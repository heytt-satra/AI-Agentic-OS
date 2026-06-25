// ── src/main.rs : `jarvis talk` — the conversation loop ─────────────────────
//
// Outer loop: read a line from you -> run the agent on it -> print the reply.
// Inner loop (run_turn): the agent's tool loop with a MAX_STEPS safety cap.
// Everything is logged to SQLite memory.

mod memory;
mod provider;
mod tools;

use anyhow::Result;
use memory::MemoryHandle;
use provider::{Message, Provider};
use std::io::{self, Write};

const MAX_STEPS: u32 = 8;

// Jarvis's persona lives in the system message (seed of the plan's PERSONA.md).
const PERSONA: &str = "You are Jarvis, a concise, dry, capable personal assistant. \
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

    println!("Jarvis online ({}).", provider.model());
    println!(
        "{} messages remembered | {} feedback rows collected. Type 'exit' to quit.\n",
        mem.count().await,
        mem.audit_count().await
    );

    // The live conversation for THIS session, seeded with the persona...
    let mut messages: Vec<Message> = vec![Message::system(PERSONA)];

    // ...and with recent dialog from PAST sessions, so Jarvis has continuity.
    // (Naive last-N recall; v0.2 makes this semantic.)
    let recalled = mem.recent_dialog(8).await;
    if !recalled.is_empty() {
        println!("(recalling {} messages from past sessions)\n", recalled.len());
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

        messages.push(Message::user(input));
        mem.log("user", input).await;

        // ── run the agent on this turn ──────────────────────────────────────
        match run_turn(&provider, &mem, &mut messages).await {
            Ok(answer) => println!("Jarvis: {answer}\n"),
            Err(e) => println!("Jarvis: (something went wrong: {e})\n"),
        }
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
