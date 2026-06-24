// ── Jarvis SPIKE-1, Step E: the agent loop (tool-calling) ───────────────────
//
// The model can't run code or know live facts. So it ASKS us to run a tool,
// we run it, hand back the result, and it answers. That cycle is the agent.
//
// Flow:
//   user question + tool list
//     -> model: "call get_current_time"   (finish_reason = "tool_calls")
//     -> WE run get_current_time in Rust
//     -> we send the result back
//     -> model: final natural-language answer   (finish_reason = "stop")
//
// We wrap it in a loop with a STEP CAP so a misbehaving model can't loop forever
// (a core safety rule from the plan).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash";
const MAX_STEPS: u32 = 5; // safety: never loop the agent more than this

// ── Message: now richer, because assistant/tool turns carry more than text ──
// `skip_serializing_if` omits a field from the JSON when it's None, so a simple
// user message doesn't send empty tool_calls/tool_call_id keys.
#[derive(Serialize, Deserialize, Clone)]
struct Message {
    role: String, // "system" | "user" | "assistant" | "tool"
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>, // present when the assistant wants tools
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>, // present on a "tool" result message
}

#[derive(Serialize, Deserialize, Clone)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: String, // "function"
    function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
struct FunctionCall {
    name: String,
    arguments: String, // a JSON-encoded STRING of the args, per the OpenAI spec
}

// ── Tool definitions we advertise to the model ──────────────────────────────
#[derive(Serialize)]
struct Tool {
    #[serde(rename = "type")]
    kind: String, // "function"
    function: FunctionDef,
}

#[derive(Serialize)]
struct FunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value, // a JSON-Schema object describing the args
}

// ── Request / response ──────────────────────────────────────────────────────
#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}

// ── One HTTP round-trip ─────────────────────────────────────────────────────
async fn call_llm(
    api_key: &str,
    model: &str,
    messages: &[Message],
    tools: Option<Vec<Tool>>,
) -> Result<Choice> {
    let client = reqwest::Client::new();
    let body = ChatRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        max_tokens: 1024,
        tools,
    };
    let response = client
        .post(API_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("HTTP-Referer", "https://lensr.in")
        .header("X-Title", "Jarvis-OS")
        .json(&body)
        .send()
        .await
        .context("HTTP request failed")?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("OpenRouter returned {status}: {text}");
    }
    let mut parsed: ChatResponse =
        serde_json::from_str(&text).context("could not parse response")?;
    // Take ownership of the first choice (swap_remove avoids a clone).
    if parsed.choices.is_empty() {
        anyhow::bail!("no choices in response");
    }
    Ok(parsed.choices.swap_remove(0))
}

// ── Our actual tool implementation ──────────────────────────────────────────
// This is plain Rust. The model never runs it; WE do, when asked.
fn run_tool(name: &str, _args_json: &str) -> String {
    match name {
        "get_current_time" => {
            // SystemTime -> seconds since 1970, formatted minimally.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("Unix timestamp: {now} (UTC seconds since 1970)")
        }
        other => format!("ERROR: unknown tool '{other}'"),
    }
}

// Describe our tool to the model as a JSON-Schema function.
fn available_tools() -> Vec<Tool> {
    vec![Tool {
        kind: "function".to_string(),
        function: FunctionDef {
            name: "get_current_time".to_string(),
            description: "Get the current time as a Unix timestamp. Use this whenever the user asks what time it is.".to_string(),
            // This tool takes no arguments: an empty object schema.
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    }]
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("No OPENROUTER_API_KEY found. Copy .env.example to .env and add your key.");
            std::process::exit(1);
        }
    };
    let model = std::env::var("OPENROUTER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    // The running conversation. We append to it as the agent loop progresses.
    let mut messages: Vec<Message> = vec![Message {
        role: "user".to_string(),
        content: Some("What time is it right now? Answer in one sentence.".to_string()),
        tool_calls: None,
        tool_call_id: None,
    }];

    println!("User: What time is it right now?\n");

    // ── THE AGENT LOOP ──────────────────────────────────────────────────────
    for step in 1..=MAX_STEPS {
        let choice = call_llm(&api_key, &model, &messages, Some(available_tools())).await?;
        let finish = choice.finish_reason.clone().unwrap_or_default();

        // Append the assistant's turn to history (it may contain tool_calls).
        messages.push(choice.message.clone());

        if finish == "tool_calls" {
            // The model wants one or more tools run. Execute each, append a
            // matching "tool" result message keyed by the tool_call_id.
            let calls = choice.message.tool_calls.clone().unwrap_or_default();
            for call in calls {
                println!("[step {step}] model called tool: {}", call.function.name);
                let result = run_tool(&call.function.name, &call.function.arguments);
                println!("[step {step}] tool result: {result}");
                messages.push(Message {
                    role: "tool".to_string(),
                    content: Some(result),
                    tool_calls: None,
                    tool_call_id: Some(call.id),
                });
            }
            // loop again so the model can use the results
            continue;
        }

        // finish_reason == "stop": the model gave its final answer.
        let answer = choice.message.content.unwrap_or_else(|| "(no answer)".to_string());
        println!("\nJarvis: {answer}");
        return Ok(());
    }

    anyhow::bail!("hit MAX_STEPS ({MAX_STEPS}) without a final answer");
}
