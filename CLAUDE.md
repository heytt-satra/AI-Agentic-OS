# JARVIS-OS

A personal AI agentic OS in Rust that sits on top of the host OS (Windows now; macOS/Linux next) and controls the whole device. One self-contained binary, zero install. Agent loop over OpenRouter/DeepSeek, real device tools, local semantic memory, second-brain activity tracker, token-streaming web HUD.

## Design System

The HUD design source of truth is [DESIGN.md](DESIGN.md). Cleaner/minimal "instrument" aesthetic: amber is the system color, cyan is for live-data signals only, red is for danger only. Self-contained monospace stack (no web fonts - keeps the binary zero-install). The HUD lives in `INDEX_HTML` in `src/server.rs`; any visual change must follow DESIGN.md and stay embeddable.

## Roadmap

Forward plan in [ROADMAP.md](ROADMAP.md): code-builder mode, cross-platform packaging, full voice, deeper autonomy.

## Conventions

- Plain English, no markdown bold (`**`), no em dashes, no AI slop. Persona enforces this; `plainify()` strips it deterministically.
- Permissions: act autonomously; only ask approval for OS/kernel/hardware-level actions that could damage the system.
- Push every working change to GitHub. Commits under the owner's name only, no AI attribution.
- Keep the binary self-contained: bundled SQLite, pure-Rust TLS, no web fonts.
