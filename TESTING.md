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

## Testing the full device powers

Run `cargo run --release` (terminal) or `cargo run --release -- serve` (HUD).
Safe actions (read, list, search, browse) run automatically. Dangerous ones
(write, shell, delete, type, click, screenshot, run-JS) pause for approval:
terminal shows `[y]es once / [a]lways / [N]o`; the HUD shows an Allow/Deny modal.

Try these one per turn:

```
# shell + filesystem
list the files in my Downloads folder and tell me the 3 biggest
create a folder called shoots on my desktop with a README inside     # approve

# app & window control (it will launch + type)
open Notepad, then type "Lensr shoot Monday 9am"                      # approve each step

# screen vision (needs OPENROUTER_VISION_MODEL set in .env)
take a screenshot and tell me what app is in focus                   # approve

# browser automation (uses your installed Chrome/Edge)
browse https://news.ycombinator.com and list the top 3 story titles

# memory (restart between these two)
remember my favorite lens is the 35mm
# ...exit, run again...
what's my favorite lens?
```

Watch the **injection defense**: ask it to browse a page AND then do something
risky in the same turn — even if you'd "always allowed" that action before, it
will ask again (because the turn touched the web).

## Second brain (activity tracking)
When you run `cargo run` or `serve`, Jarvis tracks what you do in the background
(focused app/window + clipboard) into its memory. Ask it:
```
what was I doing in the last hour?
how much time did I spend in Premiere today?
what did I copy earlier?
```
Controls (in `.env` or environment):
- `JARVIS_TRACKING=off` — turn tracking off entirely.
- `ACTIVITY_INTERVAL_SECS=5` — how often it samples the focused window (default 5).
- `SCREENSHOT_INTERVAL_SECS=300` — also save periodic screenshots (default 0 = off).
The daily digest (`cargo run -- digest`) now uses this to summarize your day.

## Reliable typing into apps
To put text into an app, ask naturally — Jarvis now does open_app -> wait ->
paste (clipboard), which fixes the earlier garbled typing. Example:
`open notepad and write "Lensr shoot Monday 9am"` (approve each step).

## Gotchas
- Screen vision: set `OPENROUTER_VISION_MODEL` in `.env` (DeepSeek can't see).
- Browser: needs Chrome or Edge installed (you have it).
- App/window control types into whatever window is FOCUSED — click the target
  app first, or let Jarvis open it.

## What you'll see
- `· <action>` — Jarvis running a tool (or `· denied: ...`).
- `[y]es once / [a]lways / [N]o` — the approval gate.
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
