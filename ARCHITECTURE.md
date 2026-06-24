# Jarvis-OS Architecture

Read this before extending the code. It exists so a fresh session (human or AI)
does not re-derive decisions or reintroduce bugs we already designed out.

## Module map (v0.1a)

```
src/
  main.rs       jarvis talk REPL + run_turn() agent loop (MAX_STEPS cap)
  provider.rs   LLM access behind one `Provider` type. base_url is env-driven.
  tools.rs      the agent's hands: read_file, write_file (sandboxed to workspace/),
                fetch_url, run_shell (Tier-2, human approval). Returns ToolOutcome.
  memory.rs     SQLite: `messages` (episodic) + `audit` (feedback dataset).
examples/
  streaming.rs  SSE token streaming demo (for voice latency-masking later).
```

## Strategy (the Cursor/Anysphere playbook)

1. **Bootstrap on routed APIs.** All LLM calls go through OpenRouter (OpenAI-
   compatible). DeepSeek V4 Flash by default; any model by changing the slug.
2. **Provider seam = local-model swap.** To run a local model later, set
   `OPENROUTER_BASE_URL=http://localhost:11434/v1` (Ollama) + a local model in
   `OPENROUTER_MODEL`. No code change. We do NOT train models; that's millions.
3. **Capture implicit feedback from day one.** The `audit` table logs every tool
   call with decision (auto/approved/denied) + ok. This is the dataset a future
   fine-tuned re-ranker or RL loop would learn from. It costs nothing to collect
   now and is unrecoverable if we skip it. Treat it as a first-class asset.

## DO NOT GENERATE — antipatterns we already rejected (eng review)

1. **No `Arc<Mutex<Connection>>`.** rusqlite `Connection` is `!Send`; sharing it
   behind a mutex serializes reads and a `MutexGuard` held across `.await` is the
   infamous `!Send` future compile wall. Reads → a pool; writes → one writer.
2. **No durability-critical events on a `tokio::broadcast` bus.** broadcast drops
   messages on lag (`RecvError::Lagged`) — that would silently lose audit rows.
   Audit/memory writes go through a bounded `mpsc` to the single writer.
3. **No blocking rusqlite inside an async task.** It stalls a tokio worker. The
   writer owns its `Connection` on a dedicated thread (or spawn_blocking).
4. **No check-then-spend budget across `.await`.** TOCTOU race under concurrency.
   Budget must reserve-then-commit through one owner.
5. **Don't `cargo add sqlite-vec` early.** It needs runtime extension loading
   (LoadExtensionGuard) or static compilation + the libclang/DLL chain. Recall is
   naive last-N for now; add the vector store deliberately in v0.2.

## Deferred until concurrency exists (NOT yet)

The full **event bus + dedicated single-writer thread** is correct *when there
are multiple concurrent producers* (scheduler + channels + REPL). v0.1a is a
single-threaded REPL, so building that now would be one publisher and one
subscriber: ceremony, not safety. The shape is documented here; we light it up
when the scheduler/Telegram channel land (v0.2). Until then `memory.rs` keeps one
owned `Connection` on the main task — simple and correct for one user.

## Orientation for a cold session

1. Read this file's "DO NOT GENERATE" list.
2. Read `src/main.rs:run_turn` — the agent loop is the spine.
3. Run `cargo run`, try a file task and `jarvis trace`-style inspection via the
   `audit` table.
4. Config/secrets: `.env` (see `.env.example`). Never commit `.env`.
