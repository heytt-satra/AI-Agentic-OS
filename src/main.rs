// ── src/main.rs : wires the pieces together and runs the agent loop ─────────
//
// `mod provider;` tells Rust: include src/provider.rs as a module named
// `provider`. Then `use provider::...` pulls its public names into scope.

mod provider;

use anyhow::Result;
use provider::{FunctionDef, Message, Provider, Tool};

const MAX_STEPS: u32 = 5;

// Our one tool, for now. (A2 will move tools into their own module and add
// read_file / write_file / fetch_url / run_shell.)
fn run_tool(name: &str, _args_json: &str) -> String {
    match name {
        "get_current_time" => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("Unix timestamp: {now} (UTC seconds since 1970)")
        }
        other => format!("ERROR: unknown tool '{other}'"),
    }
}

fn available_tools() -> Vec<Tool> {
    vec![Tool {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "get_current_time".to_string(),
            description: "Get the current time as a Unix timestamp.".to_string(),
            parameters: serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
        },
    }]
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let provider = Provider::from_env()?; // builds the client, reads config
    println!("Provider ready: {}\n", provider.model());

    let mut messages: Vec<Message> =
        vec![Message::user("What time is it right now? Answer in one sentence.")];
    println!("User: What time is it right now?\n");

    for step in 1..=MAX_STEPS {
        let reply = provider.chat(&messages, Some(available_tools())).await?;
        messages.push(reply.message.clone());

        if reply.finish_reason == "tool_calls" {
            for call in reply.message.tool_calls.clone().unwrap_or_default() {
                println!("[step {step}] tool: {}", call.function.name);
                let result = run_tool(&call.function.name, &call.function.arguments);
                println!("[step {step}] result: {result}");
                messages.push(Message::tool_result(call.id, result));
            }
            continue;
        }

        let answer = reply.message.content.unwrap_or_else(|| "(no answer)".to_string());
        println!("\nJarvis: {answer}");
        return Ok(());
    }
    anyhow::bail!("hit MAX_STEPS without a final answer");
}
