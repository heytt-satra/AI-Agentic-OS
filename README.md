# JARVIS-OS

A personal AI agentic OS in Rust that sits on top of your computer and controls
the whole device through plain language. One self-contained binary, zero install,
runs on any OS (Windows now; macOS/Linux next). Talk to it in a terminal or a
token-streaming web HUD, and it acts: files, shell, apps, the screen, the browser,
the web, and your machine's own state.

## What it does

Acts on your device
- Read/write files, run shell, open and drive apps (click, type, operate a GUI).
- See the screen (vision) and watch along with a video: it captions what is on
  screen and transcribes the audio, so you can ask about a lecture or tutorial live.
- Device awareness: read/write the clipboard, system status (CPU/memory/disk/
  battery/uptime), list and focus windows, list and kill processes, save a
  screenshot, network info (IP/Wi-Fi/online), find files by name or by recency.
- Set background reminders that fire as a desktop notification when they come due.
- Search the web, browse real pages, find leads and draft outreach.
- Build software in an isolated workspace (write, compile, test, iterate).

Has a mind you can see
- Remembers across sessions with local semantic memory (embeddings run on-device).
- Learns durable facts about you, forms its own hypotheses and goals, and keeps a
  causal record of what its actions actually cause on your machine (with a
  calibration score for how right its predictions have been).
- A second brain tracks your activity (foreground window + clipboard) so you can
  ask what you worked on and when.
- The web HUD has a live "mind panel": what it is watching, what it has learned,
  its goals (confirm or drop with one click), its causal record, and pending
  nudges you can act on or dismiss. See `jarvis mind` for the same in the terminal.

Safe and private by default
- Every action passes a safety gate: automatic for safe things, asks approval for
  ones that could damage the OS, delete data, spend money, or kill a process; it
  remembers your choice. Untrusted web/file content is treated as data, never
  instructions.
- `jarvis privacy` shows exactly what is stored and what (if anything) leaves the
  device. With a local model, nothing does.

## Run

```bash
cargo run --release                 # terminal chat (first run walks you through setup)
cargo run --release -- serve        # the web HUD (opens in your browser)
cargo run --release -- setup        # pick a brain: API key OR local model
cargo run --release -- setup --local # one command: install Ollama + a model, fully private
cargo run --release -- mind         # everything it currently knows and is thinking
cargo run --release -- help         # the full command list
```

Other commands: `eval` (scored reliability suite) and `eval trend`, `cost` (token
spend), `learnings` / `goals` / `causal` / `nudges`, `reflect` / `proact` /
`pursue`, `digest`, `dataset`, `autostart`, `daemon`, `integrate`, `privacy`.

## Choose your brain (cost is up to you)

- API key: cheapest to start, works on any machine, via OpenRouter (a capable,
  non-expensive model by default). Bring your own key.
- Local model: free per use and fully private, via Ollama. `jarvis setup --local`
  installs it and points Jarvis at it in one step.

The seam is one env var (`OPENROUTER_BASE_URL`), so you can switch anytime. First
run with no config launches an interactive setup, so you never edit `.env` by hand.

## Stack

Rust, tokio, axum, reqwest + rustls (pure-Rust TLS), rusqlite (bundled SQLite),
candle (local embeddings), sysinfo, arboard, xcap. LLM via an OpenAI-compatible
endpoint (OpenRouter or a local Ollama). Everything is compiled in: no native
runtime library needed alongside the binary.

See `ARCHITECTURE.md` for how it works, `BUILD.md` for distribution and code
signing, `DESIGN.md` for the HUD design system, and `BUILD-JOURNAL.md` for the
honest running story of how it was built.
