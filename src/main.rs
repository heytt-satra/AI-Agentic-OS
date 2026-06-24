// ── src/main.rs : wires the pieces together and runs the agent loop ─────────

mod memory;
mod provider;
mod tools;

use anyhow::Result;
use memory::Memory;
use provider::{Message, Provider};

const MAX_STEPS: u32 = 8;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let provider = Provider::from_env()?;
    let mem = Memory::open("jarvis.db")?;
    println!("Provider ready: {}", provider.model());
    // Proof of persistence: this number grows every time you run the program.
    println!("Memory: {} messages remembered from past runs\n", mem.count()?);

    // A task that forces two different tools: write, then read back.
    let task = "Create a file called shoot.txt in the workspace containing the line \
                'Lensr shoot Monday 9am'. Then read it back and confirm the exact contents.";
    let mut messages: Vec<Message> = vec![Message::user(task)];
    mem.log("user", task)?; // remember what was asked
    println!("User: {task}\n");

    for step in 1..=MAX_STEPS {
        let reply = provider.chat(&messages, Some(tools::definitions())).await?;
        messages.push(reply.message.clone());

        if reply.finish_reason == "tool_calls" {
            for call in reply.message.tool_calls.clone().unwrap_or_default() {
                println!("[step {step}] tool: {}({})", call.function.name, call.function.arguments);
                // .await because execute() is async (fetch_url awaits the network)
                let result = tools::execute(&call.function.name, &call.function.arguments).await;
                println!("[step {step}] result: {}", first_line(&result));
                messages.push(Message::tool_result(call.id, result));
            }
            continue;
        }

        let answer = reply.message.content.unwrap_or_else(|| "(no answer)".to_string());
        mem.log("assistant", &answer)?; // remember what Jarvis said
        println!("\nJarvis: {answer}");
        println!("\n[memory now holds {} messages]", mem.count()?);
        return Ok(());
    }
    anyhow::bail!("hit MAX_STEPS without a final answer");
}

// Tiny helper so long tool results don't flood the console log.
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}
