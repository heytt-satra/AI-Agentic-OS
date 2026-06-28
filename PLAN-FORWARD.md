# JARVIS-OS Forward Plan (what's left to build)

The consolidated build plan to take JARVIS-OS from "foundations done" to
"extremely strong", ordered by the strategy: TRUST first (it's the moat), then
reliability we can measure, then the economics that make per-device viable, then
the self-improvement that compounds into a monopoly, then reach. Pulls together
REVIEW.md (flaws), MARKET-RISKS.md (threats), and STRATEGY.md (the blue ocean).

Working rules (unchanged): pure-Rust + zero-install, per-device cost, smallest
change -> individual commit -> BUILD-JOURNAL entry every time.

## Phase 0 - Finish the trust track (in progress)
Trust is the entire wedge, so this comes first.
- [x] Structured prompt-injection defense (data/instruction separation)
- [x] Verifiable offline / no-telemetry mode + `jarvis privacy` report
- [x] At-rest encryption of the activity log (pure-Rust AES-GCM)
- [ ] C: execution containment - time/resource-bounded code_exec & run_shell so a
      runaway or hostile command can't hang or exhaust the machine (next).
- [ ] Full-DB encryption option (conversations/leads) - decrypt-on-start /
      encrypt-on-stop file scheme OR field encryption; with a passphrase / OS-
      keystore key option (stronger than the on-disk key-file).
- [ ] Fine-grained capability tokens: per-domain, time-boxed permissions instead
      of the coarse auto/ask gate - so it's safe to hand real money and accounts.

## Phase 1 - Reliability we can measure
You can't harden what you can't measure.
- [ ] EVAL / TEST HARNESS (top priority): unit tests for pure logic (reward
      scoring, chunking, path-safety, injection scan, MCP parsing) + an agent-task
      eval suite with automatic pass/fail scoring, runnable in CI. Turns quality
      from vibes into a number; also feeds the own-model loop.
- [ ] Planner -> executor -> critic loop: explicit plan, then verify each task is
      ACTUALLY complete (not just "the model said so").
- [ ] Parallel execution: concurrent tool calls and concurrent sub-agents (today
      both are sequential).
- [ ] Smarter loop/drift detection (semantic, not exact tool+args match).

## Phase 2 - Economics + scale
What makes per-device viable at millions of users.
- [ ] Model routing: a cheap classifier picks the model per step (trivial ->
      small/local, hard -> strong) instead of one model for everything.
- [ ] Token + cost accounting per turn, with budget enforcement.
- [ ] RAG at scale: an ANN index (HNSW) instead of brute-force cosine; chunk
      overlap for cross-boundary recall; memory consolidation (summarize + prune
      old activity) so the DB doesn't grow unbounded.
- [ ] Persistent browser session (today browse is per-call).

## Phase 3 - The compounding moat (self-improvement)
The part that makes a personal OS get stronger with use.
- [ ] Own-model DPO / preference tuning (Stage 4): train on the good-vs-bad pairs
      we already label, then the teacher loop (API model supervises the local one).
- [ ] Self-healing tools: when a tool keeps failing, the agent uses code-builder
      ON ITSELF to write or fix a tool. An OS that extends its own capabilities.
- [ ] Scheduling engine: saved agents run on a cadence ("every morning find leads
      and draft outreach") - the leap from tool to always-on workforce.

## Phase 4 - Reach + UX
- [ ] Observability/control UI: a live timeline of every agent action with
      approve / pause / replay; cost + success dashboards; the feedback dataset
      made visible.
- [ ] Local multi-modal: on-device STT/TTS (Whisper/Piper) for true voice with no
      browser or cloud.
- [ ] First-class integrations (native or via MCP): email send/receive, calendar,
      contacts - so "handle my operations" is real.
- [ ] Agent/skill registry with a safety review, so users share automations.

## Definition of "extremely strong" (how we know we got there)
- Measurable: a published agent-eval success rate that goes up, not vibes.
- Trustworthy: provably no-telemetry, encrypted at rest, sandboxed, capability-
  scoped - safe enough to hand it money and accounts.
- Cheap at scale: runs on a local model at ~$0/call, routed for efficiency.
- Personal + compounding: it learns YOU and your switching cost rises with use.

## Recommended sequence
Finish Phase 0 (C, then full-DB encryption, then capability tokens) -> Phase 1
eval harness FIRST -> the rest of Phase 1 -> Phase 2 -> Phase 3 -> Phase 4. Each
phase is several individual, journaled commits.
