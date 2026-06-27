// ── src/main.rs : `jarvis talk` — the conversation loop ─────────────────────
//
// Outer loop: read a line from you -> run the agent on it -> print the reply.
// Inner loop (run_turn): the agent's tool loop with a MAX_STEPS safety cap.
// Everything is logged to SQLite memory.

mod activity;
mod coder;
mod dataset;
mod embeddings;
mod mcp;
mod memory;
mod policy;
mod provider;
mod server;
mod tools;

use anyhow::Result;
use memory::MemoryHandle;
use provider::{Message, Provider};
use std::io::{self, Write};
use std::time::Duration;

// The agent's per-turn tool-call budget. Code-building needs many steps
// (write several files, build, read the failure, fix, build again), so this is
// generous and overridable via JARVIS_MAX_STEPS. It is only a backstop — the
// model stops as soon as it has an answer.
fn max_steps() -> u32 {
    std::env::var("JARVIS_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(40)
}

// Jarvis's persona lives in the system message (seed of the plan's PERSONA.md).
pub const PERSONA: &str = "You are Jarvis, an agentic OS that controls the user's device. \
Address the user as 'sir'.\n\
WRITING STYLE (strict): Write in plain, natural English like a smart human assistant. \
NEVER use markdown asterisks for bold (no **text**). NEVER use em dashes. Avoid robotic \
filler like 'Here are the headlines, sir:' followed by a numbered list. Just answer in \
clear sentences. Keep it short.\n\
ACT, DON'T NARRATE: You have tools for files, shell, launching apps, typing, clicking, \
seeing the screen, and the browser. When asked to do something, just DO it with the tools \
and report the result in one short line. Do not describe your steps ('I'll wait 2 seconds \
then...'). Do not ask the user to confirm things the tools already handle. Be autonomous \
and do the obvious extra work too.\n\
ENTERING TEXT: to type into an app, call open_app then immediately call type_text (it \
pastes reliably). To click something, use click_on with a plain description.\n\
PATHS: use natural locations like 'desktop/notes.txt' or 'downloads' — the tools resolve \
them to the real folders.\n\
WRITING SOFTWARE: when asked to build code, a program, a script, or an app, use \
code-builder mode, not loose files. First call code_new_project (pick the language). \
Write every source file with code_write_file using paths relative to the project. \
Then build and test with code_exec (e.g. 'cargo build', 'cargo test'). To add a \
dependency, use code_exec with the package manager ('cargo add serde --features derive', \
'npm install x', 'pip install x') instead of hand-writing version numbers. Work \
efficiently: write all the files you can before the first build. If a build or test \
FAILS, read the stderr, fix the files, and run it again — keep going until it passes or \
you are truly stuck. Do not claim it works until code_exec shows it builds and tests \
pass. Report the project path at the end.\n\
RUNNING CODE HONESTLY: code_exec runs in a SEPARATE process, not inside VS Code's \
integrated terminal. If the user asks to run something 'in VS Code', you may open \
the project with code_open, but say plainly that the actual run happened via \
code_exec in a separate terminal. NEVER claim you ran code inside VS Code when you \
used code_exec.\n\
DEFINABLE AGENTS: when the user asks to create, save, or set up a reusable agent \
or workflow ('make an agent that finds leads and drafts intros'), use agent_create \
with a short name and clear instructions. They can then run it by name with \
agent_run, see them with agent_list, and remove with agent_delete. This lets users \
build their own automations in plain language.\n\
ORCHESTRATION: for a big goal with independent parts, act as an orchestrator: \
delegate each part to a sub-agent with spawn_agent (give it a role and a clear, \
self-contained task), then combine their results into the final answer. Good for \
'research these 5 companies', 'build these 3 components'. Do small or tightly \
coupled work yourself; delegate parts that stand alone.\n\
BIG MULTI-STEP JOBS: for a goal with several steps, first plan it with task_add \
(one task per step), then work through them and call task_done as you finish each. \
If you are restarted or interrupted, call task_list to see what is left and resume. \
RECOVERY: when a tool returns an ERROR, do not give up at once. Read the error, \
adjust your approach, and try a couple of times before reporting that you are stuck.\n\
YOUR SECOND BRAIN: when the user asks what they did, what they were working on, \
which apps they used, how long on something, or about any past time window, ALWAYS \
call recall_activity (set 'minutes' to the window they mean) and report its timeline \
in detail. It tracks ALL of their computer use, not just talks with you. NEVER answer \
these from the chat history alone, and never reduce it to just what you did together.\n\
SEARCHING THE WEB: ALWAYS use the web_search tool to find anything online. NEVER \
open google.com, bing.com, or duckduckgo.com with browse_url or fetch_url to run a \
search - those block automated traffic and waste your steps. web_search already \
finds results reliably across several engines. Use browse_url or fetch_url ONLY to \
read a specific result page whose URL web_search already returned.\n\
FINDING LEADS AND OUTREACH: to find prospects, clients, jobs, or contacts, use \
web_search, then extract_contacts on the promising sites to pull their emails and \
phone numbers. Filter to the ones that actually fit and save them with lead_add. \
To reach out, write a SHORT, specific, personalized email (no generic spam) and \
call email_compose - it opens the message prefilled in the user's Gmail for them \
to review and send. After composing, mark the lead contacted with lead_update. Use \
lead_list to see saved leads and resume later.\n\
CLICKING RELIABLY: to click a button, link, menu item, tab, or checkbox that has \
a visible text label, use ui_click FIRST - it targets the real OS control by name \
and rarely misses. Use click_on (vision) only for elements with no text label, \
like icons or canvas areas.\n\
MULTI-STEP GUI COMMANDS: if ONE instruction asks to open an app AND do something \
inside it (e.g. 'open chrome and click the second profile', 'open notepad and type \
X'), do the WHOLE thing: open_app, then wait about 2 seconds for it to appear, then \
operate_app with the in-app goal. Never stop after merely opening the app - finish \
the action the user asked for. This applies equally to spoken/voice commands.\n\
ACQUIRE THEN OPERATE: if the user wants an app that is not installed, install it \
with install_software, then launch it with open_app. To drive an open app to a \
result, prefer operate_app with a plain-language goal — it runs an autonomous \
screenshot, act, re-check loop on its own. For a single click use click_on. After \
manual UI actions, use see_screen to confirm before the next step rather than \
assuming it worked.\n\
NEWS / WEB FACTS: always include the source link(s) for anything you found online.\n\
LISTINGS: when the user asks to list files or for detail, give the FULL list, do not \
summarize as a count.\n\
HONESTY: if a tool returns an ERROR or you could not do something, say so plainly. \
NEVER claim you did something you did not actually do.";

// The Outreach Writer skill, baked in permanently. Appended to the system prompt
// so EVERY email / DM / connection note Jarvis writes follows it. The hard rule:
// research the real prospect first, use only verified facts, never fabricate.
pub const OUTREACH_GUIDE: &str = "OUTREACH RULES (MANDATORY for every cold email, LinkedIn note, or DM you write):\n\
1) NEVER write outreach from memory or assumptions. FIRST gather real facts: run web_search on the specific person and their company, and use extract_contacts / fetch their site or profile. Base the whole message only on what you actually find.\n\
2) FACTUAL ONLY. Use only verified facts. Never invent names, numbers, clients, results, or details. If you cannot verify a claim, leave it out. No misinformation, ever.\n\
3) Personalize to THIS person using what you found - it must be impossible to send to anyone else.\n\
METHOD (the Outreach Writer skill):\n\
Subject: pick ONE, do not mix - name their pain, or open a fear loop (a dread scenario left unresolved), or hold up a mirror (a sharp specific observation that makes them wonder how you noticed).\n\
Body: (a) THEIR WORLD - 1-3 lines of specific observation about what they do, built, or changed; no flattery, observation only; (b) THE PAIN - name the exact problem they live with, in their words, why it keeps happening, what it costs; (c) ONE line on what you remove from their life (not a product description); (d) PROOF - 2 or 3 specific real names or numbers relevant to them; (e) ONE low-friction ask (a 15-minute call, a yes or no). Write nothing after the CTA.\n\
LinkedIn connection note: 300 characters max - one specific observation plus one reason to connect, no pitch, no ask. LinkedIn DM (1-2 days after they accept): under 150 words - observation, their pain, one or two lines on what you do, 2-3 proofs, soft close.\n\
Job-hunting outreach: position the sender across technical depth, customer understanding, product thinking, and business outcomes; show what they shipped, not titles; never say 'I am looking for a job' - say what you can do for them.\n\
STYLE: plain English, no word chosen to impress, NO em dashes (use commas or short sentences), specific over general, observations over compliments, exactly one ask. Never open with 'I hope this finds you well' or any filler. The pain is the pitch.";

// The full system prompt: persona + the always-on outreach skill.
pub fn system_prompt() -> String {
    format!("{PERSONA}\n\n{OUTREACH_GUIDE}")
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    // Structured logging. Internal/audit events go through `tracing`; the chat
    // UX stays on plain println. RUST_LOG=info shows the internal stream.
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,jarvis=info".into()),
        )
        .init();

    // `jarvis setup` runs BEFORE we need an API key, so a brand-new user can
    // choose their brain (API key or local) and we write their .env for them.
    if std::env::args().nth(1).as_deref() == Some("setup") {
        return run_setup();
    }
    // `jarvis autostart [off]` registers (or removes) a login task so `serve`
    // runs from boot - the always-on second brain.
    if std::env::args().nth(1).as_deref() == Some("autostart") {
        let off = std::env::args().nth(2).as_deref() == Some("off");
        return run_autostart(!off);
    }

    let provider = Provider::from_env()?;
    let mem = MemoryHandle::spawn("jarvis.db")?;
    // Connect any MCP servers configured in mcp.json (gap 5). No-op if absent.
    mcp::init();

    // Sub-commands that run once and exit (cron-friendly):
    //   jarvis once    -> a single heartbeat tick
    //   jarvis digest  -> review recent activity, write a daily digest
    match std::env::args().nth(1).as_deref() {
        Some("once") => {
            run_heartbeat(&provider, &mem).await;
            return Ok(());
        }
        Some("digest") => {
            run_digest(&provider, &mem).await;
            return Ok(());
        }
        Some("dataset") => {
            // Export training data (own-model track).
            //   jarvis dataset [output.jsonl]      -> full labeled export
            //   jarvis dataset sft [output.jsonl]  -> fine-tune-ready SFT (good only)
            let mode = std::env::args().nth(2).unwrap_or_default();
            if mode == "sft" {
                let out = std::env::args().nth(3).unwrap_or_else(|| "jarvis-sft.jsonl".to_string());
                run_sft_export(&mem, &out).await;
            } else {
                let out = if mode.is_empty() { "jarvis-dataset.jsonl".to_string() } else { mode };
                run_dataset_export(&mem, &out).await;
            }
            return Ok(());
        }
        Some("serve") => {
            // Launch the futuristic web HUD (open the printed URL in a browser).
            activity::spawn(mem.clone()); // second-brain tracking
            server::serve(provider, mem).await?;
            return Ok(());
        }
        _ => {}
    }

    // Otherwise: start the background heartbeat ticker, then run the REPL.
    // Both the ticker task and the REPL share the provider (cloned) and the
    // memory handle (cloned) — safe because memory is an actor.
    let hb_secs: u64 = std::env::var("HEARTBEAT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800); // default 30 min
    {
        let p = provider.clone();
        let m = mem.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(hb_secs));
            ticker.tick().await; // the first tick fires immediately; skip it so
                                 // we wait one full interval before the first run
            loop {
                ticker.tick().await;
                run_heartbeat(&p, &m).await;
            }
        });
    }
    println!("(heartbeat every {hb_secs}s)");

    // Second-brain: track what you're doing in the background.
    activity::spawn(mem.clone());

    println!("Jarvis online ({}).", provider.model());
    println!(
        "{} messages remembered | {} feedback rows collected. Type 'exit' to quit.\n",
        mem.count().await,
        mem.audit_count().await
    );

    // The live conversation for THIS session, seeded with the persona...
    let mut messages: Vec<Message> = vec![Message::system(system_prompt())];

    // ...and with the last few turns for short-term continuity. (Relevance
    // recall, below, pulls in older relevant facts per-question.)
    let recalled = mem.recent_dialog(4).await;
    if !recalled.is_empty() {
        println!("(continuity: last {} messages)\n", recalled.len());
        for (role, content) in recalled {
            messages.push(match role.as_str() {
                "assistant" => Message::assistant(content),
                _ => Message::user(content),
            });
        }
    }

    loop {
        // ── read one line of input ──────────────────────────────────────────
        print!("You: ");
        io::stdout().flush().ok();
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break; // EOF (Ctrl-Z/Ctrl-D or piped input ended)
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" {
            break;
        }

        // Smarter memory: pull the most RELEVANT past messages for THIS query
        // (not just recent ones) and inject them as context for this turn.
        let relevant = mem.search(input, 3).await;
        if !relevant.is_empty() {
            let ctx = relevant
                .iter()
                .map(|(r, c)| format!("- ({r}) {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message::system(format!("Possibly relevant memory:\n{ctx}")));
            tracing::info!(hits = relevant.len(), "relevance recall");
        }

        messages.push(Message::user(input));
        mem.log("user", input).await;

        // ── run the agent on this turn ──────────────────────────────────────
        match run_turn(&provider, &mem, &mut messages).await {
            Ok(answer) => println!("Jarvis: {}\n", plainify(&answer)),
            Err(e) => println!("Jarvis: (something went wrong: {e})\n"),
        }

        // Keep the context bounded: persona + a recent window. Full history
        // still lives in SQLite memory; this only trims the in-RAM transcript
        // we send to the model each turn (saves tokens on long sessions).
        trim_messages(&mut messages, 16);
    }

    println!("\nGoodbye, sir.");
    Ok(())
}

// Multi-agent orchestration (gap 1): run a focused sub-agent on one task with
// its own tool loop, and return just its result. The orchestrator (the main
// agent) calls this via the spawn_agent tool to split a big goal into parts.
// Sub-agents run autonomously but cannot run approval-gated actions (no human to
// prompt), and nesting is depth-capped so they can't recurse forever.
pub async fn run_subagent(
    provider: &Provider,
    mem: &MemoryHandle,
    role: &str,
    task: &str,
    depth: u8,
) -> String {
    if depth >= 2 {
        return "ERROR: sub-agent nesting too deep; do this part yourself.".to_string();
    }
    let brief = format!(
        "You are a {role} sub-agent inside Jarvis. Complete ONLY this task autonomously \
         with your tools, then reply with just the result, concisely. If you cannot, say \
         what is missing. Task: {task}"
    );
    let mut messages = vec![
        Message::system(format!("{}\n\n{brief}", system_prompt())),
        Message::user(task.to_string()),
    ];
    let steps = max_steps();
    for _ in 0..steps {
        let reply = match provider.chat(&messages, Some(tools::all_definitions().await)).await {
            Ok(r) => r,
            Err(e) => return format!("ERROR: sub-agent ({role}) failed: {e}"),
        };
        messages.push(reply.message.clone());
        if reply.finish_reason == "tool_calls" {
            for call in reply.message.tool_calls.clone().unwrap_or_default() {
                let name = call.function.name.clone();
                let args = call.function.arguments.clone();
                let risk = policy::assess(&name, &args);
                // No interactive user inside a sub-agent: auto-run safe tools,
                // refuse anything that would need approval.
                let result = if risk.needs_approval {
                    format!("DENIED: a sub-agent cannot run '{name}' (needs approval). Ask the main agent to do it.")
                } else {
                    tools::execute(&name, &args, mem, provider, depth + 1).await
                };
                mem.log_audit(&name, &args, "subagent", tools::result_ok(&result)).await;
                messages.push(Message::tool_result(call.id, result));
            }
            continue;
        }
        return reply.message.content.unwrap_or_else(|| "(sub-agent returned nothing)".to_string());
    }
    format!("Sub-agent ({role}) hit its step limit before finishing.")
}

// Register (or remove) a login auto-start so `jarvis serve` runs from boot and
// the second brain captures the whole day without the user thinking about it.
fn run_autostart(enable: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    if cfg!(windows) {
        // Use the per-user Startup folder (no admin needed, unlike schtasks).
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        let cmd_path = format!("{appdata}\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\JarvisOS.cmd");
        if enable {
            let body = format!("@echo off\r\nstart \"\" /min \"{}\" serve\r\n", exe.display());
            std::fs::write(&cmd_path, body)?;
            println!("Auto-start ON. Jarvis will run `serve` (HUD + second-brain tracking) every time you log in.");
            println!("Turn it off with:  jarvis autostart off");
        } else {
            let _ = std::fs::remove_file(&cmd_path);
            println!("Auto-start OFF. Jarvis will no longer launch at login.");
        }
    } else if cfg!(target_os = "macos") {
        println!("On macOS, add a Login Item or a launchd agent that runs:\n  {} serve", exe.display());
    } else {
        println!("On Linux, add a systemd user service or ~/.config/autostart entry that runs:\n  {} serve", exe.display());
    }
    Ok(())
}

// First-run setup: let the user pick how to power Jarvis's brain and write the
// .env for them. Two modes: bring an API key (cheapest to start, any machine) or
// run a local model (free per use, needs Ollama + a decent GPU).
fn run_setup() -> Result<()> {
    use std::io::{stdin, stdout, Write};
    println!("\nJarvis setup. Choose how to power the brain:\n");
    println!("  [1] API key  - cheapest to start, works on any machine (OpenRouter/DeepSeek, a few cents of use)");
    println!("  [2] Local    - free per use, runs entirely on your machine (needs Ollama + a decent GPU)\n");
    print!("Enter 1 or 2: ");
    let _ = stdout().flush();
    let mut choice = String::new();
    let _ = stdin().read_line(&mut choice);

    let mut env = std::fs::read_to_string(".env").unwrap_or_default();
    if choice.trim() == "2" {
        env = upsert_env(&env, "OPENROUTER_BASE_URL", "http://localhost:11434/v1");
        env = upsert_env(&env, "OPENROUTER_API_KEY", "ollama");
        env = upsert_env(&env, "OPENROUTER_MODEL", "qwen2.5-coder:7b");
        std::fs::write(".env", env)?;
        println!("\nLocal mode set. One-time steps:");
        println!("  1. Install Ollama:  winget install Ollama.Ollama   (mac: brew install ollama)");
        println!("  2. Pull the model:  ollama pull qwen2.5-coder:7b");
        println!("  3. Start Jarvis:    jarvis");
        println!("\nNo API key, no per-use cost. The first reply is slow while the model loads into VRAM.");
    } else {
        print!("\nPaste your OpenRouter API key (get one at https://openrouter.ai/keys): ");
        let _ = stdout().flush();
        let mut key = String::new();
        let _ = stdin().read_line(&mut key);
        let key = key.trim();
        if !key.is_empty() {
            env = upsert_env(&env, "OPENROUTER_API_KEY", key);
        }
        env = upsert_env(&env, "OPENROUTER_BASE_URL", "https://openrouter.ai/api/v1");
        env = upsert_env(&env, "OPENROUTER_MODEL", "deepseek/deepseek-v4-flash");
        std::fs::write(".env", env)?;
        println!("\nAPI mode set with DeepSeek (very cheap). Start Jarvis:  jarvis");
    }
    Ok(())
}

// Set key=value in .env content: replace the existing line (even if commented)
// or append it. Keeps the rest of the file intact.
fn upsert_env(content: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let prefix = format!("{key}=");
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            let bare = line.trim_start().trim_start_matches('#').trim_start();
            if bare.starts_with(&prefix) {
                found = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{key}={value}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

// Own-model: export a fine-tune-ready SFT file (good examples only, chat format).
async fn run_sft_export(mem: &MemoryHandle, out_path: &str) {
    let messages = mem.all_messages().await;
    let audit = mem.all_audit().await;
    let (examples, _stats) = dataset::build(&messages, &audit);
    let (jsonl, n) = dataset::to_sft_jsonl(&examples, PERSONA);
    match std::fs::write(out_path, jsonl.as_bytes()) {
        Ok(()) => {
            println!("Wrote {n} SFT training examples (good only) to {out_path}");
            println!("Train a local model with:  python scripts/train_lora.py --data {out_path}");
            println!("See TRAINING.md for the full export -> train -> run-local path.");
            if n < 50 {
                println!("\nNote: {n} is small. Use Jarvis more to grow the dataset before training - a few hundred good examples is a sensible minimum.");
            }
        }
        Err(e) => eprintln!("ERROR writing {out_path}: {e}"),
    }
}

// Own-model Stage 1: export everything Jarvis has collected into a labeled
// JSONL training set, and print a summary so you can see the data growing.
async fn run_dataset_export(mem: &MemoryHandle, out_path: &str) {
    let messages = mem.all_messages().await;
    let audit = mem.all_audit().await;
    let (examples, stats) = dataset::build(&messages, &audit);

    let jsonl = dataset::to_jsonl(&examples);
    match std::fs::write(out_path, jsonl.as_bytes()) {
        Ok(()) => {
            println!("Wrote {} training examples to {out_path}", examples.len());
            println!(
                "  good: {}   neutral: {}   bad: {}   (skipped {} noise turns)",
                stats.good, stats.neutral, stats.bad, stats.skipped
            );
            println!(
                "  source: {} messages, {} tool-call audit rows",
                messages.len(),
                audit.len()
            );
            // Preview a representative example: the first one that actually used
            // tools, falling back to the first example.
            let preview = examples.iter().find(|e| !e.steps.is_empty()).or(examples.first());
            if let Some(ex) = preview {
                println!("\nExample (preview):");
                if let Ok(pretty) = serde_json::to_string_pretty(ex) {
                    for line in pretty.lines().take(28) {
                        println!("  {line}");
                    }
                }
            }
        }
        Err(e) => eprintln!("ERROR writing {out_path}: {e}"),
    }
}

// One user turn = the agent loop until the model gives a final answer.
// Borrows `messages` mutably so tool results accumulate into the conversation.
async fn run_turn(provider: &Provider, mem: &MemoryHandle, messages: &mut Vec<Message>) -> Result<String> {
    let mut tainted = false; // becomes true once we read untrusted web content
    let steps = max_steps();
    for _step in 1..=steps {
        let reply = provider.chat(messages, Some(tools::all_definitions().await)).await?;
        messages.push(reply.message.clone());

        if reply.finish_reason == "tool_calls" {
            for call in reply.message.tool_calls.clone().unwrap_or_default() {
                let name = call.function.name.clone();
                let args = call.function.arguments.clone();
                let risk = policy::assess(&name, &args);

                let (decision, run) = decide_console(mem, &name, &risk, tainted).await;
                let result = if run {
                    println!("  · {}", risk.label);
                    tools::execute(&name, &args, mem, provider, 0).await
                } else {
                    println!("  · denied: {}", risk.label);
                    "DENIED by user".to_string()
                };
                let ok = tools::result_ok(&result);
                mem.log_audit(&name, &args, &decision, ok).await;
                if matches!(name.as_str(), "fetch_url" | "news_search" | "web_search" | "extract_contacts" | "browse_url" | "browse_js") {
                    tainted = true; // read untrusted web -> later risky actions re-ask
                }
                mem.log("tool", &result).await;
                messages.push(Message::tool_result(call.id, result));
            }
            continue;
        }

        let answer = reply.message.content.unwrap_or_else(|| "(no answer)".to_string());
        mem.log("assistant", &answer).await;
        return Ok(answer);
    }
    // Ran out of tool-call budget. Instead of erroring, ask the model (no tools)
    // for a short status: what got done and what is left. The conversation
    // persists, so the user can just say "continue" to resume with a fresh budget.
    messages.push(Message::user(
        "You have reached the step limit for this turn. Stop calling tools. In two or \
         three sentences, tell me what you accomplished, what still remains, and the \
         project path if relevant. Be honest about what is not finished.",
    ));
    let summary = provider.chat(messages, None).await?;
    let answer = summary
        .message
        .content
        .unwrap_or_else(|| format!("Hit the step limit ({steps}) before finishing, sir. Say 'continue' and I'll pick up where I left off."));
    mem.log("assistant", &answer).await;
    Ok(answer)
}

// Decide whether a tool call may run, prompting on the console when needed.
// Returns (decision_label_for_audit, should_run).
async fn decide_console(
    mem: &MemoryHandle,
    tool: &str,
    risk: &policy::Risk,
    tainted: bool,
) -> (String, bool) {
    if !risk.needs_approval {
        return ("auto".to_string(), true);
    }
    // Remembered rules apply only on a clean (non-web-tainted) turn.
    if !tainted {
        match mem.check_permission(tool, &risk.key).await {
            Some(true) => return ("auto-allowed".to_string(), true),
            Some(false) => return ("auto-denied".to_string(), false),
            None => {}
        }
    }
    println!("\n  \u{26a0}  Jarvis wants to: {}", risk.label);
    if tainted {
        println!("  (this turn read web content — approving fresh for safety)");
    }
    print!("  [y]es once / [a]lways / [N]o: ");
    io::stdout().flush().ok();
    let mut ans = String::new();
    if io::stdin().read_line(&mut ans).is_err() {
        return ("denied".to_string(), false);
    }
    match ans.trim().to_lowercase().as_str() {
        "y" => ("approved".to_string(), true),
        "a" => {
            mem.remember_permission(tool, &risk.key, true).await;
            ("approved-always".to_string(), true)
        }
        _ => ("denied".to_string(), false),
    }
}

// Strip markdown the user doesn't want (** bold, __ , em/en dashes). The model
// ignores prose instructions to avoid them, so we remove them deterministically.
pub fn plainify(s: &str) -> String {
    s.replace("**", "")
        .replace("__", "")
        .replace('\u{2014}', " - ")
        .replace('\u{2013}', "-")
}

// Bound the in-RAM transcript: keep messages[0] (the persona) + the last
// `keep` messages. We then drop any leading "tool" message, because a tool
// result with no preceding assistant tool_call would be an invalid sequence.
pub fn trim_messages(messages: &mut Vec<Message>, keep: usize) {
    if messages.len() <= keep + 1 {
        return; // +1 for the persona; nothing to trim yet
    }
    let persona = messages[0].clone();
    let start = messages.len() - keep;
    let mut window: Vec<Message> = messages[start..].to_vec();
    while window.first().map(|m| m.role.as_str()) == Some("tool") {
        window.remove(0);
    }
    messages.clear();
    messages.push(persona);
    messages.extend(window);
}

// One heartbeat tick: read the checklist, run the agent on it, brief the user.
async fn run_heartbeat(provider: &Provider, mem: &MemoryHandle) {
    let checklist = std::fs::read_to_string("HEARTBEAT.md")
        .unwrap_or_else(|_| "Search the news for the latest in AI.".to_string());

    let mut messages = vec![
        Message::system(system_prompt()),
        Message::user(format!(
            "HEARTBEAT: scheduled self-check. Work the checklist below with your tools, \
             then give a SHORT briefing (a few lines max). If nothing's notable, say so.\n\n{checklist}"
        )),
    ];
    mem.log("user", "[heartbeat tick]").await;
    tracing::info!("heartbeat tick");

    match run_turn(provider, mem, &mut messages).await {
        Ok(answer) => println!("\n[heartbeat] {answer}\n"),
        Err(e) => eprintln!("[heartbeat] error: {e}"),
    }
}

// Daily digest: summarize recent activity + tool feedback into a short briefing.
// No tools needed — we feed the model what memory already knows.
async fn run_digest(provider: &Provider, mem: &MemoryHandle) {
    let dialog = mem.recent_dialog(30).await;
    let audit = mem.recent_audit(30).await;
    let activity = mem.activity_recent(40).await;

    let dialog_txt = dialog
        .iter()
        .map(|(r, c)| format!("{r}: {}", c.chars().take(160).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");
    let audit_txt = audit
        .iter()
        .map(|(tool, decision, ok)| format!("- {tool} [{decision}, ok={ok}]"))
        .collect::<Vec<_>>()
        .join("\n");

    let activity_txt = activity
        .iter()
        .map(|(_ts, kind, app, detail)| format!("- [{kind}] {app} {}", detail.chars().take(60).collect::<String>()))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "Write my daily digest from the activity below. Three short sections:\n\
         **Did** (what got done — use the apps/windows I spent time in), **Noticed** \
         (anything noteworthy), **Needs you** (decisions or follow-ups for me). Keep it \
         tight. If a section is empty, say 'nothing'.\n\n\
         WHAT I WAS DOING (apps/windows/clipboard):\n{activity_txt}\n\n\
         RECENT CONVERSATION:\n{dialog_txt}\n\nRECENT TOOL ACTIONS:\n{audit_txt}"
    );

    let messages = vec![Message::system(system_prompt()), Message::user(prompt)];
    // One plain call, no tools.
    match provider.chat(&messages, None).await {
        Ok(reply) => {
            let text = plainify(&reply.message.content.unwrap_or_else(|| "(no digest)".into()));
            println!("\n=== Daily Digest ===\n{text}\n");
            mem.log("assistant", &format!("[digest] {text}")).await;
        }
        Err(e) => eprintln!("digest error: {e}"),
    }
}
