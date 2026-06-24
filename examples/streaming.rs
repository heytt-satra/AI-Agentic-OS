// ── Jarvis SPIKE-1, Step F: streaming tokens ────────────────────────────────
//
// Run with:  cargo run --example streaming
//
// With stream=true, OpenRouter sends Server-Sent Events (SSE): a long-lived
// response made of lines like:
//     data: {"choices":[{"delta":{"content":"Hel"}}]}
//     data: {"choices":[{"delta":{"content":"lo"}}]}
//     data: [DONE]
// We read bytes as they arrive, split into lines, and print each token piece
// the instant it lands. This is what lets voice start speaking sentence 1
// while sentence 2 is still being generated.

use anyhow::{Context, Result};
use futures_util::StreamExt; // gives the async stream a `.next()` method
use serde::{Deserialize, Serialize};
use std::io::Write; // for flushing stdout so tokens show immediately

const API_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-flash";

#[derive(Serialize)]
struct Req {
    model: String,
    messages: Vec<Msg>,
    max_tokens: u32,
    stream: bool, // the switch that turns on SSE
}

#[derive(Serialize)]
struct Msg {
    role: String,
    content: String,
}

// We only care about the incremental piece: choices[0].delta.content
#[derive(Deserialize)]
struct Chunk {
    choices: Vec<ChunkChoice>,
}
#[derive(Deserialize)]
struct ChunkChoice {
    delta: Delta,
}
#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .context("set OPENROUTER_API_KEY in .env")?;
    let model = std::env::var("OPENROUTER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

    let body = Req {
        model: model.clone(),
        max_tokens: 200,
        stream: true,
        messages: vec![Msg {
            role: "user".to_string(),
            content: "In 3 short sentences, introduce yourself as Jarvis.".to_string(),
        }],
    };

    println!("Streaming from {model}...\n");
    print!("Jarvis: ");
    std::io::stdout().flush().ok();

    let client = reqwest::Client::new();
    let resp = client
        .post(API_URL)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("HTTP-Referer", "https://lensr.in")
        .header("X-Title", "Jarvis-OS")
        .json(&body)
        .send()
        .await
        .context("request failed")?;

    // `bytes_stream()` yields chunks of raw bytes as the network delivers them.
    let mut stream = resp.bytes_stream();

    // SSE lines can be split across network chunks, so we keep a buffer and
    // only process complete lines (everything up to a '\n').
    let mut buffer = String::new();

    while let Some(item) = stream.next().await {
        let bytes = item.context("stream chunk error")?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        // Process every complete line currently in the buffer.
        while let Some(newline) = buffer.find('\n') {
            let line: String = buffer.drain(..=newline).collect(); // take line incl. '\n'
            let line = line.trim();

            // SSE data lines start with "data: ". Ignore comments/keepalives.
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                println!("\n\n[stream complete]");
                return Ok(());
            }
            // Parse the JSON chunk; print the token piece if present.
            if let Ok(chunk) = serde_json::from_str::<Chunk>(data) {
                if let Some(piece) = chunk.choices.first().and_then(|c| c.delta.content.clone()) {
                    print!("{piece}");
                    std::io::stdout().flush().ok(); // show it NOW, don't buffer
                }
            }
        }
    }

    println!();
    Ok(())
}
