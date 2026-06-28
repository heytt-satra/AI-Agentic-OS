# JARVIS-OS Master Plan: closing every Iron-Man gap

The endgame. Each gap below has: the GOAL (what "done" feels like), why it's
HARD, our APPROACH (concrete, real techniques - pure-Rust / local-first wherever
possible to protect the privacy + zero-install moat), MILESTONES (shippable
steps), and the METRIC (how we know it's done - vibes don't count).

Order matters: reliability is first because it's the instrument that proves every
other pillar actually works. "Google/OpenAI haven't fully done it" is true - and
irrelevant. Nobody has, because it's hard, not impossible. We do it by being
narrow (one private personal OS), measurable, and relentless.

The backbone for all of it: build a real AIOS KERNEL - a scheduler, context
manager, memory manager, tool manager, and access manager - so concurrency,
scale, and safety are systematic, not ad hoc (this is the layer the market says is
the moat).

---

## Pillar 1 - Reliability (build this FIRST; everything is gated on it)
GOAL: hand it a goal and trust it finishes correctly, ~every time.
HARD: LLMs are stochastic; "done" today means "the model said so."
APPROACH:
- Agent-task EVAL RUNNER: a suite of scripted end-to-end tasks (build X, find Y,
  refuse injection Z) with AUTOMATIC pass/fail scoring. Run in CI. This is the
  instrument - a single success-rate number that must go up.
- Planner -> Executor -> CRITIC loop: decompose the goal, execute, then a critic
  step VERIFIES completion with evidence (file exists? test passes? element on
  screen?) - not the model's say-so. Re-plan on failure.
- Verification primitives: every action has a "did it actually happen" check.
- Typed errors + no silent .ok()/unwrap_or_default swallowing; structured tool
  results the loop can reason over.
- Semantic loop/drift detection (not exact-match), retry-with-diagnosis.
MILESTONES: eval runner (10 tasks) -> critic loop -> verification primitives ->
expand suite to 50 tasks -> CI gate.
METRIC: eval success rate, climbing 60% -> 90%+ with no regressions.

## Pillar 2 - Full computer-use accuracy (drive anything flawlessly)
GOAL: operate any GUI as reliably as a person.
HARD: vision pixel-guessing misses; "whatever's in front" gets hijacked.
APPROACH:
- Accessibility-tree FIRST: enumerate the full UIA element tree (name, role,
  bounding box) as a labeled list; the model picks an element by ID; we invoke its
  REAL bounds. Coordinate-free. (We have ui_click by name; this generalizes it.)
- Set-of-Marks visual grounding: overlay numbered boxes (from a11y bounds) on the
  screenshot; the model picks a number -> exact click. The best-known technique
  for the cases a11y can't name.
- Per-WINDOW targeting (drive a specific HWND, not the foreground) - kills the
  focus-hijack failure.
- Channel priority: app API > a11y invoke > Chrome CDP (web) > set-of-marks vision.
- Verify-after-act: a11y/screenshot diff confirms each step; retry/fallback if not.
MILESTONES: a11y element list tool -> set-of-marks overlay -> window targeting ->
verify-after-act in operate_app.
METRIC: GUI-task eval subset success rate.

## Pillar 3 - Full scale (kernel + memory + RAG at scale)
GOAL: stays fast and correct with millions of memories and many concurrent agents.
HARD: brute-force cosine dies at scale; one sequential loop doesn't concurrency.
APPROACH:
- ANN vector index (HNSW, pure-Rust e.g. instant-distance/hnsw_rs) -> sub-ms recall
  over millions of chunks; chunk overlap; hierarchical RAG.
- Memory MANAGER: hot/warm/cold tiers, consolidation (summarize + prune), K-LRU
  eviction - the research's virtual-memory-for-agents idea.
- Agent SCHEDULER: run agents/tools concurrently under limits (FIFO/round-robin),
  context snapshot/interrupt so a long agent can't monopolize the brain.
MILESTONES: HNSW swap -> consolidation -> scheduler + concurrent tools/sub-agents.
METRIC: recall latency at 1M chunks; throughput with N concurrent agents.

## Pillar 4 - Self-improvement (the compounding moat)
GOAL: it gets better and cheaper the more you use it - uniquely yours.
HARD: training, evaluation, and safe hot-updates are real ML+systems work.
APPROACH:
- Data flywheel (we already label accept/reject/correct) -> DPO/ORPO preference
  tuning on the local model (Stage 4).
- Teacher->student distillation: the frontier API supervises the local model;
  disagreements become training data; local handles more over time, API less.
- Self-healing tools: repeated tool failure -> the agent code-builds a fix and
  hot-loads it. An OS that writes its own syscalls.
- Auto-skill: successful multi-step solutions are saved as reusable agents.
- Closed loop: the Pillar-1 eval suite proves each training round actually helped.
MILESTONES: eval-gated DPO run -> teacher loop -> self-healing tools -> auto-skill.
METRIC: local-model eval score approaching the API's, at ~$0/call.

## Pillar 5 - Instant natural voice (continuous duplex, local)
GOAL: talk to it like a person - interrupt, overlap, instant - no cloud.
HARD: low-latency streaming STT+TTS + barge-in is a real-time systems problem.
APPROACH (all local = privacy + no latency):
- STT: streaming Whisper (candle-whisper, pure-Rust, or whisper.cpp).
- TTS: Piper (fast neural) streamed as it generates.
- Duplex: VAD (Silero/webrtc-vad) for barge-in + partial transcripts; full-duplex
  audio loop in the HUD/native.
- Wake word ("Jarvis") via a tiny local KWS model, opt-in.
MILESTONES: local STT -> local TTS -> streaming + VAD barge-in -> wake word.
METRIC: end-to-end spoken round-trip latency (target < ~1s) and barge-in working.

## Pillar 6 - Always sees & hears (continuous perception)
GOAL: ambient awareness of your screen/context, privately.
HARD: continuous capture without spam, cost, or privacy violation.
APPROACH:
- Rolling perception buffer: throttled screen + a11y snapshots with CHANGE
  detection (only record deltas); optional mic with local VAD+STT.
- Perception -> working memory pipeline feeding the agent's context.
- Privacy-first: all local, encrypted (we have at-rest crypto), one-switch off,
  offline-mode honored.
MILESTONES: change-detected screen buffer -> a11y-state buffer -> ambient audio
(opt-in) -> perception feeding proactivity.
METRIC: "what's on my screen / what changed" answered without an explicit capture.

## Pillar 7 - Anticipation / proactivity
GOAL: it acts before you ask, correctly and unobtrusively.
HARD: false positives are worse than silence; needs pattern learning + judgment.
APPROACH:
- Routine mining over the second-brain log (you draft outreach after a lead
  search; you read news at 9am) -> learned triggers.
- Proactive engine (evolve the heartbeat): triggers (time/context/event) ->
  proposed actions, surfaced for one-tap approval (never silent for risky acts).
MILESTONES: routine miner -> trigger engine -> proactive suggestions in the HUD.
METRIC: proactive-suggestion accept rate (the eval/feedback dataset judges it).

## Pillar 8 - Brilliant AND yours/cheap (threads through 3 + 4)
GOAL: top-tier quality at ~$0 marginal cost.
APPROACH: model ROUTING (cheap model for easy steps, strong for hard) + the local
DPO-tuned own model + the teacher loop. Quality rises while cost -> 0.
METRIC: cost-per-task down, eval score up, simultaneously.

---

## Sequencing (dependency-ordered)
A. RELIABILITY + the eval instrument (Pillar 1) - FIRST. Without measurement, the
   rest is guesswork.
B. COMPUTER-USE accuracy (Pillar 2) - the most-broken capability; measurable once
   A exists.
C. SCALE + kernel (Pillar 3) - the backbone concurrency/memory.
D. SELF-IMPROVEMENT (Pillar 4) + brilliant/cheap (Pillar 8) - the moat; needs A's
   evals to prove gains.
E. VOICE (Pillar 5) + PERCEPTION (Pillar 6) - the Iron-Man feel; large, local.
F. PROACTIVITY (Pillar 7) - needs perception + reliability to be safe.

Each pillar is many individual, journaled commits. We measure with Pillar 1's
evals throughout. Constraints stay: pure-Rust, zero-install, local-first, per-
device cost, commit-each + BUILD-JOURNAL every time.

## What "done" looks like
A private, on-device OS you speak to naturally, that sees your screen, drives any
app accurately, finishes real multi-step jobs at a measured 90%+ success rate,
anticipates your routines, runs on a model trained on you at ~$0/call, and that you
can prove never leaves your machine. Not the movie's omniscience - but the part
that's real, useful, and trustworthy, which is the part that wins.
