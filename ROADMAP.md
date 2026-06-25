# JARVIS-OS Roadmap

Where we are and where we're going. Today Jarvis is a working agentic OS: an agent loop with a DeepSeek brain over OpenRouter, real device control (files, shell, apps, keyboard/mouse, screen vision, browser), local semantic memory, a second-brain activity tracker, a heartbeat, a daily digest, minimal-permission safety, and a token-streaming web HUD. One self-contained binary, zero install.

This roadmap covers the four directions chosen in the design consultation: code-builder, cross-platform, voice, and deeper autonomy. They are ordered by leverage, not locked sequence.

## Power 1 - Code-builder mode (plan, write, run, test code)

Goal: ask Jarvis to build software and it actually ships working code, not a snippet.

- A `project` workspace concept: a sandboxed directory per task, isolated from the rest of the disk.
- New tools: `run_tests`, `run_build`, `git_*` (init/commit/diff/status), `apply_patch` (structured edits, not blind overwrites).
- A plan-then-execute inner loop: Jarvis drafts a file plan, writes files, runs the build/tests, reads failures, and self-corrects until green or it hits the step cap.
- Language detection so it picks the right toolchain (cargo, npm, python).
- Feed every build/test result back into the Cursor-style feedback dataset (what worked, what got reverted).

First milestone: "write me a small CLI in Rust that does X" -> compiles and passes a test it wrote, end to end, unattended.

## Power 2 - Cross-platform (Windows, macOS, any Linux)

Goal: the same single binary behavior on all three, no separate install.

- Platform-abstracted device layer: app launch, known folders, clipboard, screenshot, and shell all behind one trait with per-OS implementations (Windows done; add macOS `open`/AppleScript and Linux `xdg-open`/wmctrl paths).
- Path resolution already uses `dirs` (OneDrive-aware on Windows); extend the same logic for mac/Linux home layouts.
- CI release matrix (already scaffolded in `.github/workflows/release.yml`) producing signed binaries for win/mac/linux on tag.
- Per-OS smoke test before release: launch, open an app, write a file to Desktop, screenshot, browse a URL.

First milestone: download the macOS binary, run it, and "open my notes app + screenshot" works identically.

## Power 3 - Full voice in the HUD (mic -> speech-to-text -> spoken reply)

Goal: talk to Jarvis and hear it talk back, like the films.

- Mic capture in the HUD via the browser Web Audio / MediaRecorder API (no native driver needed).
- Speech-to-text: route to a cheap STT API first (Cursor playbook); local Whisper as a later config swap, same seam as the chat model.
- Text-to-speech for replies: browser SpeechSynthesis as the zero-dependency default; upgradeable to a neural TTS API.
- Push-to-talk and wake-word ("Jarvis") as two modes; wake-word stays opt-in for privacy.
- Voice state wired to the orb: it already has idle/thinking/working/speaking; voice just drives the same states.

First milestone: hold a key, say "what's happening in tech today", hear the answer read back with sources.

## Power 4 - Deeper autonomy (multi-step tasks, retry, recovery)

Goal: hand Jarvis a goal, not a step, and trust it to finish.

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
