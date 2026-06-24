// ── Jarvis SPIKE-1, Step C: the first real call to Claude ──────────────────
//
// Goal: send one message to the Claude API and print the reply + token usage.
// Along the way you learn: structs, serde (JSON<->Rust), Result/?, env vars,
// and the reqwest HTTP client.

use anyhow::{Context, Result}; // `Result` here is anyhow's = Result<T, anyhow::Error>
use serde::{Deserialize, Serialize}; // derive macros to turn structs <-> JSON

// Constants. Change MODEL to claude-haiku-4-5 for cheaper test calls.
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const MODEL: &str = "claude-sonnet-4-6";

// ── Request shapes ─────────────────────────────────────────────────────────
// `#[derive(Serialize)]` auto-generates code to turn this struct INTO JSON.
// Field names become JSON keys verbatim (so `max_tokens` -> "max_tokens").
#[derive(Serialize)]
struct ChatRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>, // Vec<T> = a growable array of T
}

#[derive(Serialize, Deserialize, Clone)]
struct Message {
    role: String,    // "user" or "assistant"
    content: String, // for now, plain text (the API accepts a string here)
}

// ── Response shapes ────────────────────────────────────────────────────────
// `#[derive(Deserialize)]` generates code to parse JSON INTO this struct.
// We only declare the fields we care about; serde ignores the rest.
#[derive(Deserialize)]
struct ChatResponse {
    content: Vec<ContentBlock>, // Claude returns an ARRAY of content blocks
    usage: Usage,
    stop_reason: Option<String>, // Option<T> = "maybe a T, maybe nothing" (null-safe)
}

#[derive(Deserialize)]
struct ContentBlock {
    // `type` is a reserved Rust word, so we name the field `block_type`
    // and tell serde the JSON key is actually "type".
    #[serde(rename = "type")]
    block_type: String,
    // Not every block has text (tool blocks won't), so it's optional.
    text: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u32,
    output_tokens: u32,
}

// ── The actual API call ────────────────────────────────────────────────────
// `async fn` because the network call is something we await.
// Returns `Result<ChatResponse>`: either Ok(response) or Err(some error).
async fn call_claude(api_key: &str, user_text: &str) -> Result<ChatResponse> {
    let client = reqwest::Client::new();

    let body = ChatRequest {
        model: MODEL.to_string(),
        max_tokens: 1024,
        messages: vec![Message {
            role: "user".to_string(),
            content: user_text.to_string(),
        }],
    };

    // Build and send the POST. Each `?` means: if this step errors, stop and
    // return that error from call_claude. No try/catch pyramids.
    let response = client
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body) // serializes `body` to JSON and sets content-type
        .send()
        .await
        .context("HTTP request to Claude failed")?;

    // Anthropic returns errors as JSON with a non-2xx status. Surface them.
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        anyhow::bail!("Claude API returned {status}: {text}");
    }

    // Parse the JSON body text into our ChatResponse struct.
    let parsed: ChatResponse =
        serde_json::from_str(&text).context("could not parse Claude's JSON response")?;
    Ok(parsed)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Read the API key from the environment. `?` + .context() gives a clear
    // error if it's missing, instead of a cryptic panic.
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .context("set ANTHROPIC_API_KEY (get one at console.anthropic.com)")?;

    println!("Asking Claude ({MODEL})...\n");

    let resp = call_claude(&api_key, "Say hello in one short sentence, as Jarvis would.").await?;

    // Pull the text out of the first text block. `iter().find_map(...)` walks
    // the content blocks and grabs the first one that has text.
    let reply = resp
        .content
        .iter()
        .find_map(|b| if b.block_type == "text" { b.text.clone() } else { None })
        .unwrap_or_else(|| "(no text in response)".to_string());

    println!("Jarvis: {reply}\n");
    println!(
        "tokens: {} in / {} out | stop_reason: {}",
        resp.usage.input_tokens,
        resp.usage.output_tokens,
        resp.stop_reason.as_deref().unwrap_or("?")
    );

    Ok(())
}
