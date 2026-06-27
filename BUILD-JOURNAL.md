# JARVIS-OS Build Journal

The honest, running story of building an AI Agentic Operating System: what we
set out to do, the problems we hit, what testing revealed, what we lacked, how
we reasoned our way to a fix, and what shipped. Updated on every commit.

North star: the LLM is the kernel, agents are the apps, tools are the syscalls,
natural language is the interface. The computer becomes a reasoning digital
workforce that runs on your own device. We are building the hard 30% that the
market has not cracked, because that is what disrupts it.

---

## Part 1 - The journey so far (the ~65% already built)

### Foundation: the LLM as kernel
We built an agent loop (`run_turn`) where the model plans, calls tools, reads
results, and repeats until done, capped by a step budget. The brain is pluggable
behind `provider.rs` (OpenRouter/DeepSeek now, local Ollama by changing one env
var). This is the kernel of the AIOS.

### Tools as syscalls (our strongest layer)
We gave the kernel hands: files, shell, app launch, keyboard/mouse, screen
vision, a real headless browser, web search, code-builder, install, leads/email,
durable tasks. The sandbox was deliberately removed; safety lives in `policy.rs`
+ an approval flow, gating only OS-damaging or destructive actions.

### Memory as the file system
Local semantic memory via candle embeddings (BGE) + SQLite, with an FTS keyword
fallback. A "second brain" activity tracker logs every foreground app and
clipboard copy. This is our RAG-as-filesystem, though today it spans
conversations and activity, not arbitrary documents (a known gap, item 3).

### Notable problems and how we solved them
- **DeepSeek ignored style rules (`**` markdown, em dashes).** Instructions did
  not work. Fix: deterministic `plainify()` post-processing, and later running
  outgoing emails through it too. Lesson: for a model that ignores style
  instructions, enforce in code, not prompts.
- **Files landed in the wrong place; a fake "Desktop" appeared.** The real
  Desktop was OneDrive-redirected. Fix: resolve natural paths through the OS
  known-folder API (`dirs`).
- **Stale-binary trap.** `cargo run` (debug) and `--release` are different
  binaries, and a running process holds its old code. We kept "fixing" things
  that did not change because we tested a stale build. Lesson: rebuild the right
  profile and kill the running instance before every release test. Now habitual.
- **code_exec could not find cargo.** A non-interactive shell lacked the user
  PATH. Fix: `toolchain_path()` prepends `~/.cargo/bin` and friends.
- **Step-limit failures on multi-file builds.** Raised the budget to 40 and,
  instead of erroring at the cap, the agent now returns an honest status and the
  user can say "continue".
- **Search kept getting blocked.** Single-engine DuckDuckGo scraping rate-limited
  our IP. Fix: a fallback chain (DuckDuckGo -> Mojeek -> Bing) with per-engine
  parsers, plus a persona rule forcing the model to use `web_search` instead of
  improvising into blocked Google/Bing pages. Verified working with DDG burned.
- **install_software hung 30+ minutes (WhatsApp).** winget waited on source
  ambiguity. Fix: pin `--source winget` + a 4-minute timeout on a blocking
  thread so a stuck installer can never freeze the agent.
- **Activity recall only summarized Jarvis chats.** The data was captured but
  recall summarized the conversation instead of querying the log. Fix:
  timeframe-based recall (`activity_since`) rendering a real timeline with clock
  times (chrono) and per-app totals, plus a persona rule to always use it.
  Remaining truth: tracking only runs while Jarvis runs, so we added
  `jarvis autostart` to keep it always-on.
- **"Ran it in VSCode" was a false claim.** It ran in a separate terminal. Fix:
  a `code_open` tool + a persona honesty rule never to claim VSCode execution
  when using code_exec.

### Interaction + autonomy already in place
Natural-language REPL and a voice HUD (push-to-talk STT + spoken TTS, browser
APIs, zero-dependency). An agentic loop that self-corrects (code-builder fixes
its own build errors), retries transient failures, and tracks durable tasks that
survive restarts. A baked-in Outreach Writer skill that researches a prospect
before writing, uses only verified facts, and never uses em dashes.

### Cost architecture (the business moat)
Everything runs per-device: the user brings an API key or runs a local model
(`jarvis setup` chooses). No central API bill that scales with millions of users.
Local-first is also the answer to the privacy backlash against cloud AI
monitoring.

---

## Part 2 - The remaining hard 30% (what we build next, in order)

1. **Multi-agent orchestration** - an orchestrator that decomposes a goal and
   delegates to specialist sub-agents. Turns one assistant into a workforce.
2. **Reliable computer-use** - drive any GUI accurately by vision. The field's
   hardest open problem.
3. **Document RAG** - ingest and semantically search the user's real files/PDFs/
   codebases, not just conversations.
4. **User-definable agents/workflows** - let the user create, save, and schedule
   their own agents in plain language. The democratization piece.
5. **MCP / open standards** - speak the Model Context Protocol to plug into the
   ecosystem of tools and servers.
6. **Own model** - train on the dataset we already export (Stage 1 done).
7. **Reliability + safety at scale** - long-horizon stability and
   prompt-injection hardening, so it is safe to hand real money and real access.

---

## Part 3 - Ongoing build log

### 2026-06-27 - Journal created; starting item 1 (multi-agent orchestration)
Set up this journal and committed the assessment that we are ~65% to the AIOS
architecture, strongest on real device control + tools + local-first, weakest on
multi-agent orchestration and reliable computer-use. Decision: build all seven
gaps in order, smallest changes committed individually, journal updated each
commit.

### 2026-06-27 - Item 1 SHIPPED: multi-agent orchestration
**Goal:** turn the single agent into an orchestrator that delegates focused
sub-tasks to specialist sub-agents - the "digital workforce" in the AIOS essay.

**Thinking / design:** a sub-agent is just another agent loop with a focused
system prompt and its own step budget, that returns only its result. The
orchestrator (main agent) calls it through a new `spawn_agent(role, task)` tool.

**Problems we reasoned through:**
- The tool dispatcher `tools::execute` didn't have the `Provider` (needed to run
  a sub-agent's model calls). Decision: thread `provider` and a `depth` counter
  into `execute`, rather than special-casing the agent loop in two places (REPL +
  HUD). Cleaner and reusable.
- Async recursion: `execute -> run_subagent -> execute` is a self-referential
  async cycle that won't compile (infinite future size). Fix: `Box::pin` the
  `run_subagent` call inside the spawn_agent arm to break the cycle.
- Safety + termination: a sub-agent has no human to approve risky actions, and
  could recurse forever. Decisions: sub-agents auto-run only non-approval tools
  and refuse anything gated; nesting is capped at depth 2.

**How it works:** `run_subagent(provider, mem, role, task, depth)` in main.rs
runs the focused loop; `spawn_agent` in tools.rs invokes it; persona teaches the
orchestrator to delegate independent parts and synthesize.

**Test:** "delegate a researcher (find a physicist + a fact) and a coder (write
and run a squares script), then combine." Result: both sub-agents ran - the coder
actually scaffolded and executed `squares.py` (output 1,4,9,16,25), the
researcher returned a Feynman fact - and the orchestrator merged them. Passed.

**Still to do later:** true PARALLEL sub-agents (our tool loop runs calls
sequentially), and richer role-specific tool subsets.
