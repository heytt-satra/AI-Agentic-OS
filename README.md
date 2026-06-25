# AI-Agentic-OS (Jarvis)

A personal, voice-capable agentic assistant in Rust that can act on your device.
Single self-contained binary, runs on any OS, no separate install.

## What it does
- Talks to you (terminal REPL or a futuristic web HUD).
- Uses tools to act: read/write files, run shell commands, open apps/files/URLs,
  search news, fetch URLs.
- Remembers across sessions with local semantic memory (embeddings run on-device).
- Acts on its own via a scheduled heartbeat, and can write a daily digest.
- Every risky action passes a permission gate (auto for safe, ask for dangerous,
  remembers your choice) with prompt-injection defenses.

## Run
```bash
cargo run --release -- serve   # the web HUD (opens in your browser)
cargo run --release            # terminal chat
cargo run --release -- once    # one heartbeat tick
cargo run --release -- digest  # daily digest
```

Set your key first: copy `.env.example` to `.env` and add an OpenRouter key.
See `TESTING.md` to try it, `ARCHITECTURE.md` for how it works, `BUILD.md` for
distribution.

## Stack
Rust, tokio, axum, reqwest+rustls, rusqlite (bundled), candle (local embeddings).
LLM via OpenRouter (DeepSeek by default; swap any model by changing one string).
