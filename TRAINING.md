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
A 6GB GPU (e.g. RTX 4050) trains a 1.5B base in 4-bit. More VRAM -> use a 7B base.
```
pip install -r scripts/requirements-train.txt
# install a CUDA torch build matching your driver (see pytorch.org)
```

### 4. Train (LoRA / QLoRA)
```
python scripts/train_lora.py --data jarvis-sft.jsonl --base Qwen/Qwen2.5-Coder-1.5B-Instruct
```
Output: a LoRA adapter in `./jarvis-lora`. A small run is minutes to an hour.

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
