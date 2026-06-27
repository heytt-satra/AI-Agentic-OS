#!/usr/bin/env python3
"""
QLoRA fine-tune for Jarvis-OS on YOUR exported usage data (own-model, gap 6).

This trains a small open model to imitate the good responses Jarvis produced for
you, so it can later run locally with no API. It runs OUTSIDE the Rust binary,
on a machine with an NVIDIA GPU.

Pipeline:
  1. Use Jarvis for a while (it logs everything).
  2. Export:  jarvis dataset sft jarvis-sft.jsonl
  3. Install: pip install -r scripts/requirements-train.txt
  4. Train:   python scripts/train_lora.py --data jarvis-sft.jsonl
  5. See TRAINING.md to run the result locally via Ollama.

A 1.5B base fits a 6GB GPU (e.g. RTX 4050) in 4-bit. Use 7B if you have more VRAM.
"""
import argparse
import torch
from datasets import load_dataset
from transformers import AutoTokenizer, AutoModelForCausalLM, BitsAndBytesConfig
from peft import LoraConfig
from trl import SFTTrainer, SFTConfig


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="jarvis-sft.jsonl", help="SFT jsonl from `jarvis dataset sft`")
    ap.add_argument("--base", default="Qwen/Qwen2.5-Coder-1.5B-Instruct", help="base model to fine-tune")
    ap.add_argument("--out", default="./jarvis-lora", help="where to save the LoRA adapter")
    ap.add_argument("--epochs", type=float, default=2.0)
    args = ap.parse_args()

    ds = load_dataset("json", data_files=args.data, split="train")
    print(f"Loaded {len(ds)} training examples from {args.data}")

    tok = AutoTokenizer.from_pretrained(args.base)
    bnb = BitsAndBytesConfig(
        load_in_4bit=True,
        bnb_4bit_quant_type="nf4",
        bnb_4bit_compute_dtype=torch.bfloat16,
    )
    model = AutoModelForCausalLM.from_pretrained(args.base, quantization_config=bnb, device_map="auto")

    peft = LoraConfig(
        r=16, lora_alpha=32, lora_dropout=0.05,
        target_modules="all-linear", task_type="CAUSAL_LM",
    )
    cfg = SFTConfig(
        output_dir=args.out,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=1,
        gradient_accumulation_steps=8,
        learning_rate=2e-4,
        logging_steps=10,
        save_strategy="epoch",
        bf16=True,
        max_seq_length=2048,
        packing=False,
    )
    # SFTTrainer applies the model's chat template to the "messages" field.
    trainer = SFTTrainer(model=model, args=cfg, train_dataset=ds, peft_config=peft, processing_class=tok)
    trainer.train()
    trainer.save_model(args.out)
    print(f"Done. LoRA adapter saved to {args.out}. Next: see TRAINING.md to run it locally.")


if __name__ == "__main__":
    main()
