# Design System — JARVIS-OS

## Product Context
- **What this is:** A personal AI agentic OS (Rust) that sits on top of Windows/macOS/Linux and controls the whole device, with a browser-served HUD and a terminal REPL.
- **Who it's for:** The owner (a solo founder), as a daily command surface.
- **Space/industry:** Personal AI agents / agentic OS (peers: OpenClaw, Claude Desktop).
- **Project type:** Local web HUD (single self-contained page served by the Rust binary) + CLI.

## Aesthetic Direction
- **Direction:** Restrained futurism / industrial instrument. An calm cockpit, not a movie set.
- **Decoration level:** minimal — a faint grid and bracket-framed viewport; no scanlines, no glow spam.
- **Mood:** The moment it loads you feel you're at a real control surface: quiet, precise, alive. Confident, not noisy.
- **Memorable thing:** "It feels like an instrument that's actually running my machine."

## Typography
- **Display/Wordmark:** the monospace stack, letter-spaced wide (0.5em) — techy without a web font.
- **Body/Chat:** monospace stack.
- **Data/Labels:** monospace stack, smaller, letter-spaced, uppercase for telemetry.
- **Stack (self-contained, NO web fonts — keeps the binary zero-install):**
  `'SF Mono','JetBrains Mono','Cascadia Code','Cascadia Mono',Consolas,ui-monospace,monospace`
- **Scale:** wordmark 15px / chat 14px / telemetry 10.5px / state-label 11px. Line-height 1.55.

## Color
- **Approach:** restrained. Amber is the system color; one cool accent for live data; red only for danger.
- **Primary (system/amber):** `#FFB000` — wordmark, orb, prompts, active chrome.
- **Accent (live data/cyan):** `#39D3C0` — used ONLY for live/active signals (tool tags, inner orb ring, "online"). Never decorative.
- **Danger/Approval:** `#FF5C5C` — approval modal emphasis, errors, destructive warnings.
- **Neutrals:** bg `#04060A`, surface `#0A0F14`, hairline `rgba(255,176,0,.14)`, text `#CDD6DF`, muted `#5D6B77`.
- **Dark mode:** this IS the (only) mode — designed dark-native.

## Spacing
- **Base unit:** 8px.
- **Density:** comfortable (generous viewport whitespace around the orb; calm).
- **Scale:** 4 / 8 / 12 / 16 / 24 / 32 / 48.

## Layout
- **Approach:** centered single column, instrument-framed.
- **Grid:** one column; content max-width ~900px, centered.
- **Chrome:** thin bracket corners on the viewport; top telemetry bar; orb centered above the conversation; command line pinned at the bottom.
- **Border radius:** mostly 0 (sharp, instrument-like); 2px on the input only.

## Motion
- **Approach:** minimal-functional. The orb breathes slowly when idle, speeds/pulses by state; messages fade in; no boot theatrics.
- **Easing:** ease-out for enter, ease-in-out for state.
- **Duration:** micro 120ms / state 400ms. Orb runs continuously but calm.

## Anti-slop guardrails (enforced)
No purple/blue glow as the identity, no 3-column icon grids, no centered marketing copy, no gradient buttons, no bubble radii, no web-font dependency, no scanline overload. Amber monochrome + one cyan accent + red-for-danger only.

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-06-25 | Restrained-futurism instrument aesthetic, amber + cyan-accent + red-danger, self-contained monospace | /design-consultation; user chose the "cleaner/minimal" variant of the arc-reactor HUD; must stay embeddable + zero-install |
