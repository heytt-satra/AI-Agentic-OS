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
- [x] C: execution containment - timeout+kill for code_exec & run_shell (run_bounded)
- [ ] Full-DB encryption option (conversations/leads) - decrypt-on-start /
      encrypt-on-stop file scheme OR field encryption; with a passphrase / OS-
      keystore key option (stronger than the on-disk key-file).
- [ ] Fine-grained capability tokens: per-domain, time-boxed permissions instead
      of the coarse auto/ask gate - so it's safe to hand real money and accounts.

## Phase 1 - Reliability we can measure
You can't harden what you can't measure.
- [x] EVAL / TEST HARNESS (foundation): 15 cargo unit tests over the security-
      critical pure logic (crypto round-trip, path-traversal, injection scan,
      network classification, reward labeling, contact extraction). `cargo test`
      green. STILL TO DO: an agent-task eval RUNNER (scored end-to-end) + CI wiring.
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
- [x] Scheduling engine: saved agents run on a cadence (schedule_add/list/remove +
      a ticker in `serve`) - with autostart, the always-on workforce.

## NEXT WAVE (prioritized, ready to build)
Built this session: activity encryption, execution containment, eval/test harness,
scheduling. The remaining items, ordered by value/effort and grouped by how they
get built:

Clean + buildable now (each ~one commit):
1. Cost/token accounting - capture `usage` from replies into a `usage` table;
   `jarvis cost` report + surface in privacy. (Streaming-usage is the fiddly part.)
2. Model routing - opt-in OPENROUTER_MODEL_FAST; route trivial turns to the cheap
   model via a Provider.routed() clone. Low risk (default unchanged).
3. Semantic loop detection - upgrade the exact tool+args loop guard to fuzzy.
4. Parallel sub-agents - run multiple spawn_agent calls concurrently (tokio join)
   instead of sequentially.
5. Persistent browser session - keep one headless-Chrome alive across browse calls.

Larger, deliberate (multi-commit, but in-binary):
6. Full-DB encryption (conversations/leads) with passphrase/keystore key.
7. Capability tokens - fine-grained, time-boxed permissions.
8. ANN vector index (HNSW) + chunk overlap + memory consolidation for RAG at scale.
9. Self-healing tools - detect repeated tool failure, code-build a fix, hot-add.
10. Agent-task eval RUNNER + CI wiring (completes Phase 1 #1).

External / needs hardware or a big frontend (own deliberate projects, not a marathon):
11. Own-model DPO training (GPU) - pipeline exists (TRAINING.md); this is the run.
12. Local STT/TTS (Whisper/Piper) - on-device voice without the browser.
13. Observability/control UI - live action timeline with approve/pause/replay.
14. Native integrations (email/calendar/contacts), agent/skill registry.

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
