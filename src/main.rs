// ── src/main.rs : `jarvis talk` — the conversation loop ─────────────────────
//
// Outer loop: read a line from you -> run the agent on it -> print the reply.
// Inner loop (run_turn): the agent's tool loop with a MAX_STEPS safety cap.
// Everything is logged to SQLite memory.

mod activity;
mod coder;
mod embeddings;
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
        .unwrap_or(20)
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
Then build and test with code_exec (e.g. 'cargo build', 'cargo test'). If a build or \
test FAILS, read the stderr, fix the files, and run it again — keep going until it \
passes or you are truly stuck. Do not claim it works until code_exec shows it builds \
and tests pass. Report the project path at the end.\n\
NEWS / WEB FACTS: always include the source link(s) for anything you found online.\n\
LISTINGS: when the user asks to list files or for detail, give the FULL list, do not \
summarize as a count.\n\
HONESTY: if a tool returns an ERROR or you could not do something, say so plainly. \
NEVER claim you did something you did not actually do.";

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

    let provider = Provider::from_env()?;
    let mem = MemoryHandle::spawn("jarvis.db")?;

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
    let mut messages: Vec<Message> = vec![Message::system(PERSONA)];

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

// One user turn = the agent loop until the model gives a final answer.
// Borrows `messages` mutably so tool results accumulate into the conversation.
async fn run_turn(provider: &Provider, mem: &MemoryHandle, messages: &mut Vec<Message>) -> Result<String> {
    let mut tainted = false; // becomes true once we read untrusted web content
    let steps = max_steps();
    for _step in 1..=steps {
        let reply = provider.chat(messages, Some(tools::definitions())).await?;
        messages.push(reply.message.clone());

        if reply.finish_reason == "tool_calls" {
            for call in reply.message.tool_calls.clone().unwrap_or_default() {
                let name = call.function.name.clone();
                let args = call.function.arguments.clone();
                let risk = policy::assess(&name, &args);

                let (decision, run) = decide_console(mem, &name, &risk, tainted).await;
                let result = if run {
                    println!("  · {}", risk.label);
                    tools::execute(&name, &args, mem).await
                } else {
                    println!("  · denied: {}", risk.label);
                    "DENIED by user".to_string()
                };
                let ok = tools::result_ok(&result);
                mem.log_audit(&name, &args, &decision, ok).await;
                if matches!(name.as_str(), "fetch_url" | "news_search" | "browse_url" | "browse_js") {
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
    anyhow::bail!("hit MAX_STEPS ({steps}) without finishing")
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
        Message::system(PERSONA),
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

    let messages = vec![Message::system(PERSONA), Message::user(prompt)];
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
