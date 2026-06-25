# Testing Jarvis

Open a terminal in this folder (`cd C:\Users\heytt\jarvis-os`). A fresh terminal
already has `cargo` on PATH.

## Run modes

| Command | What it does |
|---|---|
| `cargo run --release -- serve` | **Futuristic web HUD** — opens the amber UI in your browser. Best experience. |
| `cargo run` | Talk to Jarvis in the terminal (REPL). `exit` to quit. |
| `cargo run -- once` | One heartbeat tick (reads HEARTBEAT.md, briefs you), then exits. |
| `cargo run -- digest` | Print a daily digest of recent activity, then exits. |
| `cargo run --example streaming` | Watch tokens stream live. |

## Speed note (important)
Always use **`--release`** for real use — the local embedding model runs 10-30x
faster optimized. `cargo run` (debug) is for quick code checks only. The HUD in
particular should be run as `cargo run --release -- serve`.

## Quick capability test (type into `cargo run`, one line at a time)

```
what's happening in AI news right now?         # news_search (J2)
write a file called test.txt that says hello    # write_file (sandboxed to workspace/)
read test.txt back to me                        # read_file
run the shell command: echo it works            # approval gate -> type y (or n to deny)
remember my favorite food is biryani            # stores a fact
exit
```

Then run `cargo run` again (new session) and ask with DIFFERENT words:

```
what do I like to eat?                          # semantic recall -> biryani
where do I work?                                 # semantic recall -> Lensr
exit
```

This proves memory survives restarts and recalls by MEANING, not keywords.

## What you'll see
- `· using <tool>` — Jarvis chose a tool.
- `⚠ Approve? [y/N]` — the safety gate for shell commands.
- `[memory] semantic embeddings ready` — local embedding model loaded.

## Config / data
- Keys: `.env` (already set). Template: `.env.example`.
- Memory DB: `jarvis.db` (delete it to start fresh).
- Files Jarvis writes: `workspace/`.
- Heartbeat cadence: env `HEARTBEAT_SECS` (default 1800). Checklist: `HEARTBEAT.md`.
- Internal logs: set `RUST_LOG=info` to see tool/recall events.

## Troubleshooting
- "No OPENROUTER_API_KEY": copy `.env.example` to `.env`, add your key.
- Offline: semantic recall falls back to keyword (FTS) search automatically.
- `cargo` not found: open a new terminal, or run `$env:PATH = "$HOME\.cargo\bin;$env:PATH"`.
