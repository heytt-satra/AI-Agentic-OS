Jarvis' Log — June 25, 2026

Sir, I wrote this myself. The world today is moving fast — AI models being extracted by rivals, chip breakthroughs, ancient scrolls being read for the first time, and the quiet hum of a system ready for whatever you need next. That's all.# JARVIS-OS Roadmap

Where we are and where we're going. Today Jarvis is a working agentic OS: an agent loop with a DeepSeek brain over OpenRouter, real device control (files, shell, apps, keyboard/mouse, screen vision, browser), local semantic memory, a second-brain activity tracker, a heartbeat, a daily digest, minimal-permission safety, and a token-streaming web HUD. One self-contained binary, zero install.

This roadmap covers the four directions chosen in the design consultation: code-builder, cross-platform, voice, and deeper autonomy. They are ordered by leverage, not locked sequence.

## Power 1 - Code-builder mode (plan, write, run, test code)  [SHIPPED v1]

Goal: ask Jarvis to build software and it actually ships working code, not a snippet.

Shipped: an isolated workspace under `~/jarvis-projects/<name>` with five tools -
`code_new_project` (scaffold a toolchain), `code_write_file` (path-safe, project-relative),
`code_read_file`, `code_list` (file tree), and `code_exec` (build/test/run/git with the
project as the working directory). The existing agent loop is the self-correct loop:
`code_exec` returns the real exit code + stdout + stderr, the model reads failures and
fixes until green. Step budget raised to 20 (JARVIS_MAX_STEPS). Verified end to end:
"build a rust CLI that prints fibonacci, then run it" scaffolds, builds, and runs clean.

Still to do: structured `apply_patch` edits (vs full-file rewrites), and feeding
build/test outcomes into the Cursor-style feedback dataset.

- A `project` workspace concept: a sandboxed directory per task, isolated from the rest of the disk.
- New tools: `run_tests`, `run_build`, `git_*` (init/commit/diff/status), `apply_patch` (structured edits, not blind overwrites).
- A plan-then-execute inner loop: Jarvis drafts a file plan, writes files, runs the build/tests, reads failures, and self-corrects until green or it hits the step cap.
- Language detection so it picks the right toolchain (cargo, npm, python).
- Feed every build/test result back into the Cursor-style feedback dataset (what worked, what got reverted).

First milestone: "write me a small CLI in Rust that does X" -> compiles and passes a test it wrote, end to end, unattended.

## Power 2 - Cross-platform (Windows, macOS, any Linux)  [v1 SHIPPED]

Goal: the same single binary behavior on all three, no separate install.

Shipped: device ops are now per-OS. `open_app` has Windows (Start Menu + terminal
fallback), macOS (`open -a`, then Terminal via AppleScript), and Linux
(`gtk-launch` -> binary -> `xdg-open`) branches. `run_shell`/`open_path`/`code_exec`
were already cfg-split, path resolution already uses `dirs`, and screenshot,
clipboard, and input are cross-platform crates. CI release matrix builds win/mac/
linux binaries on tag. Honest caveat: only the Windows paths are runtime-verified
here - the mac/Linux branches compile behind cfg and need a real mac/Linux box (or
CI) to smoke-test.

- Platform-abstracted device layer: app launch, known folders, clipboard, screenshot, and shell all behind one trait with per-OS implementations (Windows done; add macOS `open`/AppleScript and Linux `xdg-open`/wmctrl paths).
- Path resolution already uses `dirs` (OneDrive-aware on Windows); extend the same logic for mac/Linux home layouts.
- CI release matrix (already scaffolded in `.github/workflows/release.yml`) producing signed binaries for win/mac/linux on tag.
- Per-OS smoke test before release: launch, open an app, write a file to Desktop, screenshot, browse a URL.

First milestone: download the macOS binary, run it, and "open my notes app + screenshot" works identically.

## Power 3 - Full voice in the HUD (mic -> speech-to-text -> spoken reply)  [v1 SHIPPED]

Goal: talk to Jarvis and hear it talk back, like the films.

Shipped: a "Talk" push-to-talk button in the HUD uses the browser Web Speech API
for speech-to-text (transcribes into the input and auto-sends), and a VOICE
toggle speaks Jarvis's replies via the browser SpeechSynthesis TTS. Both are
zero-dependency and keep the binary self-contained - no STT/TTS API, no cost. The
mic state flashes the orb's live tag. Caveat: voice input needs a Chromium browser
(Chrome/Edge); wake-word and local Whisper are future upgrades on the same seam.

- Mic capture in the HUD via the browser Web Audio / MediaRecorder API (no native driver needed).
- Speech-to-text: route to a cheap STT API first (Cursor playbook); local Whisper as a later config swap, same seam as the chat model.
- Text-to-speech for replies: browser SpeechSynthesis as the zero-dependency default; upgradeable to a neural TTS API.
- Push-to-talk and wake-word ("Jarvis") as two modes; wake-word stays opt-in for privacy.
- Voice state wired to the orb: it already has idle/thinking/working/speaking; voice just drives the same states.

First milestone: hold a key, say "what's happening in tech today", hear the answer read back with sources.

## Power 4 - Deeper autonomy (multi-step tasks, retry, recovery)  [v1 SHIPPED]

Goal: hand Jarvis a goal, not a step, and trust it to finish.

Shipped: a durable task list in SQLite with tools `task_add`, `task_list`,
`task_done`, `task_cancel`. The persona tells Jarvis to plan a multi-step goal as
tasks, work through them, mark each done, and call `task_list` to resume after a
restart. The step budget already auto-summarizes and lets the user say "continue".
`fetch_url` now retries with backoff on transient network failure. Verified end to
end: Jarvis added three tasks, listed, marked one done, and a fresh process read
the list back with correct statuses (durability across restart). Future: surfacing
long-running tasks live in the HUD, and richer auto-recovery.

- A task planner that decomposes a goal into steps, tracks progress, and resumes if interrupted.
- Retry-with-backoff and error recovery: when a tool returns ERROR, diagnose and try an alternative instead of giving up or lying about success.
- Long-running background tasks surfaced in the HUD with live status (reuse the heartbeat plumbing).
- Checkpointing to memory so a multi-hour task survives a restart.
- Tighter safety as autonomy grows: the existing tiered permission + injection-taint model stays the backstop; escalate approval only for OS/kernel/hardware-level actions.

First milestone: "find me 5 leads for Lensr and draft outreach" runs to completion across many steps with one summary at the end.

## Cross-cutting

- Keep the binary self-contained and zero-install through all of the above (no web fonts, bundled SQLite, pure-Rust TLS).
- Keep the Cursor playbook running: route to cheap APIs now, capture accept/reject feedback, swap to local/own models later at the existing config seams.
- Every working change pushed to GitHub, commits under the owner's name only.
