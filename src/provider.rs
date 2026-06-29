// ── src/provider.rs : the LLM provider layer ────────────────────────────────
//
// Everything about "how we talk to an LLM" lives here, behind one type:
// `Provider`. The rest of Jarvis only knows `provider.chat(messages, tools)`.
// It does NOT know or care whether that's OpenRouter, Claude, or a local
// Ollama model — because all of them speak the same OpenAI-compatible API.
//
// THAT is the answer to "can we use our own model later?": yes, by pointing
// `base_url` at http://localhost:11434/v1 (Ollama) and changing the model
// string. No other code changes.
//
// `pub` marks things visible OUTSIDE this module (main.rs can use them).
// Anything without `pub` is private to this file.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Public message/tool types (main.rs builds and reads these) ──────────────
#[derive(Serialize, Deserialize, Clone)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    // Small constructors so main.rs reads cleanly: Message::user("hi")
    pub fn user(text: impl Into<String>) -> Self {
        Message { role: "user".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn system(text: impl Into<String>) -> Self {
        Message { role: "system".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant(text: impl Into<String>) -> Self {
        Message { role: "assistant".into(), content: Some(text.into()), tool_calls: None, tool_call_id: None }
    }
    pub fn tool_result(tool_call_id: String, result: impl Into<String>) -> Self {
        Message { role: "tool".into(), content: Some(result.into()), tool_calls: None, tool_call_id: Some(tool_call_id) }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String, // JSON-encoded string of args
}

#[derive(Serialize, Clone)]
pub struct Tool {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDef,
}

#[derive(Serialize, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// What chat() hands back: the assistant's message + why it stopped + how many
// tokens it cost (0 if the API didn't report usage, e.g. the streaming path).
pub struct Reply {
    pub message: Message,
    pub finish_reason: String,
    pub tokens: u64,
}

// ── Private wire types (only used to (de)serialize the HTTP body) ────────────
#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Tool>>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}

// ── The Provider itself ─────────────────────────────────────────────────────
// Holds the HTTP client + config. Built once, reused for every call.
// Clone is cheap: reqwest::Client is an Arc internally; the strings are small.
#[derive(Clone)]
pub struct Provider {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    fast_model: Option<String>, // optional cheap model for trivial turns (Pillar 8)
}

impl Provider {
    // Constructor that reads config from the environment (.env already loaded
    // by main). Defaults to OpenRouter + DeepSeek, but every value is
    // overridable — including base_url, the local-model seam.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .context("OPENROUTER_API_KEY not set (copy .env.example to .env)")?;
        let model = std::env::var("OPENROUTER_MODEL")
            .unwrap_or_else(|_| "deepseek/deepseek-v4-flash".to_string());
        // To use a LOCAL model later: set OPENROUTER_BASE_URL=http://localhost:11434/v1
        let base_url = std::env::var("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());
        // Optional: a cheaper/faster model for trivial turns. Unset = routing off.
        let fast_model = std::env::var("OPENROUTER_MODEL_FAST").ok().filter(|s| !s.trim().is_empty());
        Ok(Provider { client: reqwest::Client::new(), api_key, model, base_url, fast_model })
    }

    // Model routing (Pillar 8): if a fast model is configured and the user's
    // message looks trivial (short, no build/code/web/file keywords), return a
    // clone that uses the cheap model. Default behavior is unchanged when
    // OPENROUTER_MODEL_FAST is not set. Conservative: anything non-trivial stays
    // on the strong model, and tool-heavy turns are never downgraded mid-flight
    // because routing is decided once on the opening user message.
    pub fn routed(&self, user_msg: &str) -> Provider {
        if let Some(fast) = &self.fast_model {
            if Self::is_trivial(user_msg) {
                let mut p = self.clone();
                p.model = fast.clone();
                return p;
            }
        }
        self.clone()
    }

    fn is_trivial(msg: &str) -> bool {
        let m = msg.trim().to_lowercase();
        if m.len() > 140 {
            return false;
        }
        const HARD: &[&str] = &[
            "build", "code", "compile", "fix", "debug", "run ", "install", "search",
            "find", "research", "browse", "scrape", "email", "lead", "write", "create",
            "open ", "click", "operate", "screenshot", "schedule", "agent", "file",
            "project", "deploy", "test",
        ];
        !HARD.iter().any(|k| m.contains(k))
    }

    // In offline mode the brain must be local; refuse to call a cloud endpoint so
    // no prompt/data can leave the device.
    fn guard_offline(&self) -> Result<()> {
        if crate::tools::offline_mode()
            && !(self.base_url.contains("localhost") || self.base_url.contains("127.0.0.1"))
        {
            anyhow::bail!(
                "OFFLINE mode is on but the model is a cloud endpoint ({}). Set up a local model (jarvis setup -> Local) so nothing leaves the device, or unset JARVIS_OFFLINE.",
                self.base_url
            );
        }
        Ok(())
    }

    // One chat round-trip. `&self` = borrows the Provider (doesn't consume it),
    // so you can call chat() as many times as you like.
    pub async fn chat(&self, messages: &[Message], tools: Option<Vec<Tool>>) -> Result<Reply> {
        self.guard_offline()?;
        let body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            max_tokens: 1024,
            tools,
            stream: false,
        };
        let url = format!("{}/chat/completions", self.base_url);
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://lensr.in")
            .header("X-Title", "Jarvis-OS")
            .json(&body)
            .send()
            .await
            .context("HTTP request failed")?;

        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("LLM API returned {status}: {text}");
        }

        let mut parsed: ChatResponse =
            serde_json::from_str(&text).context("could not parse LLM response")?;
        if parsed.choices.is_empty() {
            anyhow::bail!("no choices in response");
        }
        let tokens = parsed.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0);
        let choice = parsed.choices.swap_remove(0);
        Ok(Reply {
            message: choice.message,
            finish_reason: choice.finish_reason.unwrap_or_default(),
            tokens,
        })
    }

    // Streaming chat: forwards content tokens to `dtx` as they arrive (for the
    // live HUD), accumulates tool_calls + full content, returns the Reply when
    // the stream ends. Tool-call turns produce no content deltas.
    pub async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<Vec<Tool>>,
        dtx: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<Reply> {
        use futures_util::StreamExt;
        self.guard_offline()?;
        let body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            max_tokens: 1024,
            tools,
            stream: true,
        };
        let url = format!("{}/chat/completions", self.base_url);
        let response = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://lensr.in")
            .header("X-Title", "Jarvis-OS")
            .json(&body)
            .send()
            .await
            .context("HTTP request failed")?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM API returned {status}: {text}");
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content = String::new();
        let mut finish = String::new();
        // (id, name, args) accumulated per tool_call index across fragments
        let mut tools_acc: Vec<(String, String, String)> = Vec::new();

        while let Some(item) = stream.next().await {
            let bytes = item.context("stream chunk error")?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(nl) = buffer.find('\n') {
                let line: String = buffer.drain(..=nl).collect();
                let line = line.trim();
                let Some(data) = line.strip_prefix("data: ") else { continue };
                if data == "[DONE]" {
                    break;
                }
                let v: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let choice = &v["choices"][0];
                if let Some(fr) = choice["finish_reason"].as_str() {
                    finish = fr.to_string();
                }
                let delta = &choice["delta"];
                if let Some(c) = delta["content"].as_str() {
                    if !c.is_empty() {
                        content.push_str(c);
                        let _ = dtx.send(c.to_string());
                    }
                }
                if let Some(tcs) = delta["tool_calls"].as_array() {
                    for tc in tcs {
                        let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                        while tools_acc.len() <= idx {
                            tools_acc.push((String::new(), String::new(), String::new()));
                        }
                        if let Some(id) = tc["id"].as_str() {
                            if !id.is_empty() { tools_acc[idx].0 = id.to_string(); }
                        }
                        if let Some(n) = tc["function"]["name"].as_str() {
                            if !n.is_empty() { tools_acc[idx].1 = n.to_string(); }
                        }
                        if let Some(args) = tc["function"]["arguments"].as_str() {
                            tools_acc[idx].2.push_str(args);
                        }
                    }
                }
            }
        }

        let tool_calls = if tools_acc.is_empty() {
            None
        } else {
            Some(tools_acc.into_iter().map(|(id, name, arguments)| ToolCall {
                id,
                kind: "function".to_string(),
                function: FunctionCall { name, arguments },
            }).collect())
        };
        Ok(Reply {
            message: Message {
                role: "assistant".to_string(),
                content: if content.is_empty() { None } else { Some(content) },
                tool_calls,
                tool_call_id: None,
            },
            finish_reason: finish,
            tokens: 0, // streaming responses don't carry usage in this path
        })
    }

    // Expose the model name for logging.
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_triviality() {
        // trivial chat -> eligible for the cheap model
        assert!(Provider::is_trivial("what is 2+2?"));
        assert!(Provider::is_trivial("hello, how are you"));
        // anything tool/work-shaped stays on the strong model
        assert!(!Provider::is_trivial("build a rust cli"));
        assert!(!Provider::is_trivial("search the web for rust news"));
        assert!(!Provider::is_trivial("open notepad and click save"));
        // long messages are never trivial
        assert!(!Provider::is_trivial(&"a ".repeat(100)));
    }
}
