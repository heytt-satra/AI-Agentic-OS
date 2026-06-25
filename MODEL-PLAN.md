# Own-Model Plan (the Cursor playbook)

The goal is not to pretrain a frontier model from scratch. That costs tens to
hundreds of millions of dollars and no one sane does it. The goal is what Cursor
actually does: own the data, run inference locally, and fine-tune small open
models that learn the owner's taste specifically.

## The ladder

**Stage 0 - Route + instrument. DONE.**
DeepSeek via OpenRouter is the brain. Embeddings already run on-device (candle +
BGE). Every tool call is logged to the `audit` table with its outcome.

**Stage 1 - Make the dataset real. SHIPPED.**
`jarvis dataset [out.jsonl]` turns the message + audit history into labeled JSONL
training examples. Each example is one episode: a user request, the final
response, the tool steps taken, and a heuristic reward derived from implicit
signals (did it answer, did tools error, did the user correct it next turn or
move on). Tool steps are attributed to the most recent user turn at or before
each tool's timestamp, so second-granular clocks do not drop actions. Output is
gitignored - it is private conversation data. Code: `src/dataset.rs`,
`memory::all_messages` / `all_audit`, and the `dataset` subcommand in `main.rs`.

Still to refine: stronger reward signals (build/test pass as strong positive,
better correction detection), and de-duplication.

**Stage 2 - Run the brain locally. NEXT.**
The seam already exists: `OPENROUTER_BASE_URL` points at any OpenAI-compatible
endpoint. Stand up Ollama (or llama.cpp) serving an open-weight model (Qwen2.5-
Coder, Llama 3.x), flip that one env var, and Jarvis runs with no third-party
API and zero code change. Measure the quality gap vs DeepSeek.

**Stage 3 - Specialize, do not generalize.**
Fine-tune a small open model (LoRA/QLoRA) for ONE high-volume, learnable
sub-task - best candidates: an "apply edit" model for code-builder mode, or a
tool-router (does this need a tool, which one). Route only that sub-task to the
local model; keep the API for hard reasoning.

**Stage 4 - Preference tuning (the "learns you" part).**
Once enough paired accept/reject data exists, run DPO/ORPO on the local model so
it aligns to the owner's taste, not a generic average. The labels come from
Stage 1's rewards.

**Stage 5 - The teacher loop.**
When the local model is unsure or fails, the frontier API handles it and that
exchange becomes new training data. Over time local handles more, API less, cost
bends down. Steady state.

## Cost / hardware reality

- Stages 0-2 are cheap or free, mostly config and testing. Ollama runs on CPU if
  there is no GPU, just slower.
- Stages 3-4 need a GPU for a few hours. A LoRA fine-tune of a 7B is roughly
  $10-50 of rented compute, free on an owned GPU. A few hundred to a few thousand
  clean examples is enough to start.
- We never pretrain. We fine-tune open weights.

## Usage

```
jarvis dataset                 # writes jarvis-dataset.jsonl
jarvis dataset mydata.jsonl    # custom path
```
