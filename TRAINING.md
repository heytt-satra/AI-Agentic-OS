# Training your own Jarvis model (own-model, gap 6)

The goal of the own-model track: stop paying per call by running a model trained
on YOUR usage, locally. The Rust binary handles data collection and export; the
actual training is a GPU job you run with the scripts here. Honest truth: this
needs an NVIDIA GPU and a few hundred good examples to be worthwhile. It is not a
button inside the app, and it should not be.

## The full path

### 1. Collect data (free, automatic)
Just use Jarvis. Every turn is logged, and outcomes are labeled (did it answer,
did a tool error, did you correct it or move on). Leave `jarvis serve` running
(or use the REPL) over days.

### 2. Export a fine-tune-ready file
```
jarvis dataset sft jarvis-sft.jsonl
```
This writes only the GOOD examples in chat-messages format. Check the count it
prints; aim for a few hundred before training.

### 3. Set up a GPU machine
```
pip install -r scripts/requirements-train.txt
# install a CUDA torch build matching your driver (see pytorch.org)
```

#### On a 6GB card (RTX 4050 laptop, etc.) — what actually fits
6GB is enough for QLoRA (4-bit) of a SMALL base. It is NOT enough for full
fine-tuning or a 7B (7B QLoRA OOMs around 6GB). The script defaults are tuned for
6GB: Qwen2.5-1.5B base, 4-bit + double-quant, LoRA r16, batch 1, grad-accum 16,
gradient checkpointing, paged 8-bit optimizer, seq len 1024.
- Fits comfortably: 1.5B (default). Often fits: 3B with `--max-seq 512`.
- If you OOM: lower `--max-seq 768` (or 512), then `--lora-r 8`. Close other GPU
  apps (browser/games) - they eat VRAM. It is a 105W laptop card, so expect a slow
  run (tens of minutes to a couple hours) and some thermal throttling; that's fine.

### 4. Train (LoRA / QLoRA)
```
python scripts/train_lora.py --data jarvis-sft.jsonl
# 6GB-tight? python scripts/train_lora.py --data jarvis-sft.jsonl --max-seq 512 --lora-r 8
```
Output: a LoRA adapter in `./jarvis-lora`. A small run is minutes to an hour.
SFT FIRST - DPO (preference tuning) needs paired good-vs-bad data and more VRAM, so
it comes later; on 6GB, SFT of a 1.5B is the realistic, useful step.

### 5. Run it locally, no API
Two options:
- Merge the adapter into the base and convert to GGUF (llama.cpp
  `convert_hf_to_gguf.py`), then `ollama create jarvis -f Modelfile` pointing at
  the GGUF.
- Or serve base+adapter with any OpenAI-compatible local server.

Then point Jarvis at it:
```
jarvis setup   # choose Local, model = your jarvis model
```
The provider seam (`OPENROUTER_BASE_URL`) makes this a config change, no code.

#### Best use on 6GB: make it the CHEAP router model
A 1.5B SFT won't beat DeepSeek on hard multi-tool reasoning - so don't replace the
strong brain with it. Instead, plug it into model routing (Pillar 8): keep the
strong model for real work and send TRIVIAL turns to your local model for $0:
```
OPENROUTER_MODEL=deepseek/deepseek-v4-flash          # strong, for real work
OPENROUTER_MODEL_FAST=jarvis                          # your local model, for trivial turns
OPENROUTER_BASE_URL=http://localhost:11434/v1         # Ollama
```
Or run fully local + private: set OPENROUTER_MODEL to your local model and
JARVIS_OFFLINE=1 so nothing leaves the device. As your dataset grows, the local
model takes over more turns and your API spend (visible via `jarvis cost`) drops.

## Where this sits in the plan
- Stage 1 (done): dataset export with reward labels.
- This (gap 6): turnkey SFT export + training script + run-local path.
- Later: DPO/preference tuning from good-vs-bad pairs, and a teacher loop where
  the API model supervises the local one. See MODEL-PLAN.md.

## Honest caveats
- A small fine-tune of a small base will be weaker than DeepSeek/Claude at hard,
  multi-tool reasoning. Its value is cost ($0/call) and privacy, and getting
  steadily better as your dataset grows.
- Quality scales with data. Hundreds-to-thousands of good examples is where this
  starts to pay off.
