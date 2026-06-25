// ── src/main.rs : `jarvis talk` — the conversation loop ─────────────────────
//
// Outer loop: read a line from you -> run the agent on it -> print the reply.
// Inner loop (run_turn): the agent's tool loop with a MAX_STEPS safety cap.
// Everything is logged to SQLite memory.

mod embeddings;
mod memory;
mod provider;
mod server;
mod tools;

use anyhow::Result;
use memory::MemoryHandle;
use provider::{Message, Provider};
use std::io::{self, Write};
use std::time::Duration;

const MAX_STEPS: u32 = 8;

// Jarvis's persona lives in the system message (seed of the plan's PERSONA.md).
pub const PERSONA: &str = "You are Jarvis, a concise, dry, capable personal assistant. \
Address the user as 'sir'. Keep spoken answers short. You have tools to read/write \
files in a workspace, fetch URLs, and run shell commands (which require the user's \
approval). Use them when useful rather than guessing.";

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
            Ok(answer) => println!("Jarvis: {answer}\n"),
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
    for step in 1..=MAX_STEPS {
        let reply = provider.chat(messages, Some(tools::definitions())).await?;
        messages.push(reply.message.clone());

        if reply.finish_reason == "tool_calls" {
            for call in reply.message.tool_calls.clone().unwrap_or_default() {
                println!("  · using {}", call.function.name);
                let outcome = tools::execute(&call.function.name, &call.function.arguments).await;

                // Record the feedback signal (the Cursor-style dataset).
                mem.log_audit(&call.function.name, &call.function.arguments, &outcome.decision, outcome.ok).await;
                tracing::info!(
                    tool = %call.function.name,
                    decision = %outcome.decision,
                    ok = outcome.ok,
                    "tool call (step {step})"
                );

                mem.log("tool", &outcome.result).await;
                messages.push(Message::tool_result(call.id, outcome.result));
            }
            continue;
        }

        let answer = reply.message.content.unwrap_or_else(|| "(no answer)".to_string());
        mem.log("assistant", &answer).await;
        return Ok(answer);
    }
    anyhow::bail!("hit MAX_STEPS ({MAX_STEPS}) without finishing")
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

    let prompt = format!(
        "Write my daily digest from the activity below. Three short sections:\n\
         **Did** (what got done), **Noticed** (anything noteworthy), **Needs you** \
         (decisions or follow-ups for me). Keep it tight. If a section is empty, say 'nothing'.\n\n\
         RECENT CONVERSATION:\n{dialog_txt}\n\nRECENT TOOL ACTIONS:\n{audit_txt}"
    );

    let messages = vec![Message::system(PERSONA), Message::user(prompt)];
    // One plain call, no tools.
    match provider.chat(&messages, None).await {
        Ok(reply) => {
            let text = reply.message.content.unwrap_or_else(|| "(no digest)".into());
            println!("\n=== Daily Digest ===\n{text}\n");
            mem.log("assistant", &format!("[digest] {text}")).await;
        }
        Err(e) => eprintln!("digest error: {e}"),
    }
}
