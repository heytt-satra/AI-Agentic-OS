# SPIKE-1 Result (2026-06-24)

**Question (from the plan):** can we drive an LLM from Rust with tool-calling, streaming, and caching — via `rig`, or do we fall back to direct HTTP?

**What we did:** built it direct, with `reqwest` + `serde`, against OpenRouter's OpenAI-compatible API, model `deepseek/deepseek-v4-flash`.

**Result — all proven LIVE:**
- Basic chat round-trip (Step C) — works.
- Tool-calling agent loop with a `MAX_STEPS` safety cap (Step E) — works; model called `get_current_time`, we executed it in Rust, model used the result.
- Token streaming over SSE (Step F, `examples/streaming.rs`) — works; needed for voice latency-masking later.
- Prompt caching — DeepSeek caches context automatically; no `cache_control` plumbing needed (the Anthropic-specific concern in the original plan does not apply on this provider).

**Verdict / decisions:**
1. **No `rig`.** Direct `reqwest` → OpenRouter is sufficient and simpler. One HTTP path.
2. **OpenRouter as the gateway.** Swap models (DeepSeek ↔ Claude ↔ GPT) by changing the model string. Resolves the plan's UC2/ENG1 SPIKE-1 checkpoint: the framework risk is gone because we don't use one.
3. **Provider layer shape (for ARCHITECTURE.md later):** a `provider` module exposing `call_llm(messages, tools) -> response` + a streaming variant. Models/keys come from env (`OPENROUTER_API_KEY`, `OPENROUTER_MODEL`).

**Cost observed:** a few dozen tokens per test call on DeepSeek V4 Flash = fractions of a cent. Dev iteration is effectively free.

**Run it:**
- `cargo run` — the tool-calling agent loop
- `cargo run --example streaming` — the streaming demo
