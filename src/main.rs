// ── Jarvis SPIKE-1, Step C: first real LLM call (via OpenRouter) ────────────
//
// We talk to OpenRouter, a gateway that speaks the OpenAI-compatible API to
// many models. We use DeepSeek V4 Flash (cheap). The SAME code can later hit
// Claude/GPT/Gemini by changing one model string.
//
// Teaches: structs, serde (JSON<->Rust), Result/?, env vars, reqwest, and the
// OpenAI chat-completions request/response shape.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash";

// ── Request shapes ─────────────────────────────────────────────────────────
#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
}

// One struct serves both request and response messages here, so it derives
// BOTH Serialize (to send) and Deserialize (to receive).
#[derive(Serialize, Deserialize, Clone)]
struct Message {
    role: String, // "system" | "user" | "assistant"
    // In responses, content can be null (e.g. tool calls), so it's Optional.
    content: Option<String>,
}

// ── Response shapes (OpenAI format) ────────────────────────────────────────
// Note the difference from Anthropic: replies live under choices[].message,
// and token counts are prompt_tokens / completion_tokens.
#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
}

// ── The API call ───────────────────────────────────────────────────────────
async fn call_llm(api_key: &str, model: &str, user_text: &str) -> Result<ChatResponse> {
    let client = reqwest::Client::new();

    let body = ChatRequest {
        model: model.to_string(),
        max_tokens: 1024,
        messages: vec![Message {
            role: "user".to_string(),
            content: Some(user_text.to_string()),
        }],
    };

    let response = client
        .post(API_URL)
        .header("Authorization", format!("Bearer {api_key}")) // OpenAI-style auth
        .header("HTTP-Referer", "https://lensr.in") // optional: OpenRouter attribution
        .header("X-Title", "Jarvis-OS") // optional: shows in your OpenRouter dashboard
        .json(&body)
        .send()
        .await
        .context("HTTP request to OpenRouter failed")?;

    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("OpenRouter returned {status}: {text}");
    }

    let parsed: ChatResponse =
        serde_json::from_str(&text).context("could not parse the JSON response")?;
    Ok(parsed)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env into the environment (no-op if the file is absent).
    let _ = dotenvy::dotenv();

    // Read the key. If missing, print a friendly, actionable message — this is
    // the "any user can insert their key and it works" experience.
    let api_key = match std::env::var("OPENROUTER_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            eprintln!("No OPENROUTER_API_KEY found.");
            eprintln!("Fix: copy .env.example to .env and paste your key from https://openrouter.ai/keys");
            std::process::exit(1);
        }
    };

    // Model is overridable via env; otherwise the cheap DeepSeek default.
    let model = std::env::var("OPENROUTER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    println!("Asking {model} via OpenRouter...\n");

    let resp = call_llm(&api_key, &model, "Say hello in one short sentence, as Jarvis would.").await?;

    // OpenAI format: take the first choice's message content.
    let reply = resp
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .unwrap_or_else(|| "(no content in response)".to_string());

    let finish = resp
        .choices
        .first()
        .and_then(|c| c.finish_reason.clone())
        .unwrap_or_else(|| "?".to_string());

    println!("Jarvis: {reply}\n");
    println!(
        "tokens: {} in / {} out | finish_reason: {finish}",
        resp.usage.prompt_tokens, resp.usage.completion_tokens
    );

    Ok(())
}
