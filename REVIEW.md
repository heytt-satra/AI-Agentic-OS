# JARVIS-OS Reverse-Engineering Review

An honest teardown of the current build: what it is, where it is weak, what to
strengthen, and what to add to make it a category-defining product. Written
against the actual code, not the marketing. Nothing here is hidden to look good.

## 1. What it actually is

An LLM-as-kernel agent loop (`run_turn` / the HUD loop) over a pluggable provider
(`provider.rs`, OpenRouter/DeepSeek or local). Around it: a large built-in tool
set (`tools.rs`), a SQLite actor for memory/leads/tasks/agents/docs (`memory.rs`),
local embeddings (`embeddings.rs`), a second-brain tracker (`activity.rs`), a
policy/approval gate (`policy.rs`), a web HUD (`server.rs`), multi-agent
sub-agents (`run_subagent`), an MCP client (`mcp.rs`), document RAG, and a
training-data pipeline (`dataset.rs`). It runs on the real device. Good bones.

## 2. The biggest flaws (ranked by how much they matter)

1. **No automated tests or evals.** Everything is verified by hand, once. There
   is not a single `cargo test`, no agent-task eval harness, no regression net. For
   a product people trust with their machine, this is the number-one risk: any
   change can silently break a capability and nobody knows until a user hits it.

2. **Security is thin for the power it has.** Tools run with the user's full
   privileges, no sandbox. The prompt-injection defense is keyword heuristics
   (`looks_like_injection`) - trivially bypassed by paraphrase or non-English.
   Sub-agents bypass approval gates entirely. MCP servers are fully trusted once
   configured. The SQLite DB (activity log, clipboard history, leads, contacts) is
   plaintext on disk - a privacy/exfiltration liability.

3. **The agent loop is shallow.** It is a single, sequential tool loop with no
   real planner, no self-verification that a task is actually complete, and no
   reflection. Tool calls run one at a time (no parallelism), so multi-part jobs
   are slow, and "done" is whatever the model says, not a checked fact.

4. **Reliability guards are crude.** The loop guard is exact (tool+args) match -
   a model that paraphrases its repetition slips through. There is no drift
   detection on long-horizon tasks, and recovery is just "retry a couple times".

5. **Computer-use is still vision-fragile.** `ui_click` (a11y) is reliable for
   named controls, but `operate_app` falls back to pixel-guessing for anything
   unlabeled, and there is no a11y element-list grounding inside the loop.

6. **Scaling cliffs in memory/RAG.** Semantic search is a brute-force cosine over
   every vector each query - fine at thousands, dead at millions (no ANN index).
   Chunks have no overlap (recall misses across boundaries) and a hard per-file
   cap. The browser session is per-call (no persistence).

7. **No cost/observability layer.** No token accounting, no per-call cost, no
   budget enforcement, no success-rate metrics. You cannot see what the system
   costs or how well it is doing.

8. **Single model for everything.** Easy and hard tasks hit the same model. No
   routing (cheap model for trivial steps, strong model for reasoning) - wasteful
   and slower than it needs to be.

9. **Silent error-swallowing.** Many `.ok()` / `unwrap_or_default()` hide failures
   instead of surfacing them, which makes debugging and honesty harder.

10. **Cross-platform unverified.** mac/Linux code paths compile behind cfg but
    have never run; the MCP hub can block if a server never responds (no read
    timeout); globals (`OnceLock`) make some state hard to reset.

## 3. Strengthen what already exists

- **Add a test + eval harness (highest priority).** Unit tests for the pure logic
  (dataset reward, chunking, path safety, injection scan, mcp parsing), and an
  AGENT EVAL suite: a set of scripted tasks with automatic pass/fail scoring, run
  in CI. This is what turns "vibes" into measurable quality and lets you improve
  safely. It also feeds the own-model loop.
- **Harden security:** sandbox risky tools (run generated code / shell in a
  restricted process or container), encrypt the SQLite DB at rest, give MCP
  servers a trust tier, and replace keyword injection detection with structured
  defenses (clearly separate data vs instruction channels, "spotlighting", and an
  allowlist of what tainted turns may do).
- **Deepen the loop:** a planner -> executor -> critic structure; explicit
  task-completion verification; parallel tool calls and parallel sub-agents;
  semantic (not exact) loop/drift detection.
- **Scale memory:** an ANN index (HNSW) for vectors, chunk overlap, memory
  consolidation (summarize and prune old activity), and a persistent browser.
- **Economics:** token + cost accounting per turn, budget enforcement, and model
  routing (a cheap classifier picks the model per step).
- **Computer-use:** feed the a11y element list into operate_app so it picks a real
  element instead of guessing pixels; add scroll/double-click/drag.

## 4. Things to ADD (category-defining)

- **Scheduling engine** so saved agents run on a cadence ("every morning find
  leads and draft outreach") - the leap from tool to autonomous workforce.
- **Self-healing tools:** when a tool keeps failing, the agent uses code-builder
  on ITSELF to write or fix a tool. An OS that extends its own capabilities.
- **Real planning/reflection** with a persistent task graph that survives restarts
  and shows progress, not just a flat to-do list.
- **Multi-modal, local:** on-device STT/TTS (Whisper/Piper) for true voice without
  a browser or cloud; optional camera/audio context.
- **First-class integrations** (native or via MCP): email send/receive, calendar,
  contacts, Slack, so "handle my operations" is real.
- **Observability + control UI:** a live timeline of every agent action with
  approve / pause / replay, cost and success dashboards, and the feedback dataset
  made visible.
- **Preference tuning loop (own-model Stage 4):** wire DPO from the good-vs-bad
  pairs we already label, plus a teacher loop where the API model supervises the
  local one - the path to a model that is genuinely yours and cheap.
- **Fine-grained capability tokens:** per-domain, time-boxed permissions instead
  of the coarse auto/ask gate, so you can safely hand it money and accounts.
- **Agent/skill registry** with a safety review, so users share automations.

## 5. If we do only five things next (to make it extremely strong)

1. **Automated eval/test harness** - you cannot harden what you cannot measure.
2. **Security: sandbox + DB encryption + structured injection defense** - required
   before it touches money or credentials at scale.
3. **Planner/critic + parallelism** - reliability and speed on real multi-step work.
4. **Model routing + cost accounting** - the economics that make per-device viable.
5. **Scheduling** - turns the seven shipped capabilities into an always-on workforce.

Everything above is depth on a foundation that already exists. The skeleton and
the muscles are built; this is the work that makes it bulletproof.
