// ── src/embeddings.rs : local, pure-Rust text embeddings (candle) ───────────
//
// Turns text into a 384-dim vector capturing MEANING. Two texts about the same
// thing land close together even if they share no words — which is exactly what
// keyword search (FTS) couldn't do (it ranked the repeated QUESTION over the
// ANSWER). Runs locally on CPU, no API, no native install: candle is pure Rust.
//
// Model: BAAI/bge-small-en-v1.5 (~130MB), downloaded once to the HF cache.

use anyhow::{anyhow, Context, Result};
use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use hf_hub::{api::sync::Api, Repo, RepoType};
use tokenizers::Tokenizer;

const MODEL_ID: &str = "BAAI/bge-small-en-v1.5";

pub struct Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl Embedder {
    // Download (first run) + load the model. Blocking; call from a worker thread.
    pub fn load() -> Result<Self> {
        let device = Device::Cpu;
        let api = Api::new().context("hf-hub api")?;
        let repo = api.repo(Repo::new(MODEL_ID.to_string(), RepoType::Model));

        let config_path = repo.get("config.json").context("download config.json")?;
        let tokenizer_path = repo.get("tokenizer.json").context("download tokenizer.json")?;
        let weights_path = repo.get("model.safetensors").context("download model.safetensors")?;

        let config: Config = serde_json::from_str(&std::fs::read_to_string(config_path)?)
            .context("parse config.json")?;
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| anyhow!("tokenizer: {e}"))?;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)? };
        let model = BertModel::load(vb, &config).context("load bert")?;

        Ok(Self { model, tokenizer, device })
    }

    // Embed one string into a normalized 384-dim vector.
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let encoded = self.tokenizer.encode(text, true).map_err(|e| anyhow!("encode: {e}"))?;
        let ids: Vec<u32> = encoded.get_ids().to_vec();
        let n_tokens = ids.len();

        // Shape [1, n_tokens]. token_type_ids are all zeros for a single text.
        let token_ids = Tensor::new(ids.as_slice(), &self.device)?.unsqueeze(0)?;
        let token_type_ids = token_ids.zeros_like()?;

        // Forward pass -> [1, n_tokens, hidden]. No attention mask needed for a
        // single unpadded sequence.
        let hidden = self.model.forward(&token_ids, &token_type_ids, None)?;

        // Mean-pool across tokens -> [1, hidden], then L2-normalize so that
        // cosine similarity == dot product later.
        let mean = (hidden.sum(1)? / n_tokens as f64)?;
        let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?;
        let normalized = mean.broadcast_div(&norm)?;

        let v: Vec<f32> = normalized.squeeze(0)?.to_vec1()?;
        Ok(v)
    }
}

// ── vector <-> bytes (for storing in a SQLite BLOB) ─────────────────────────
pub fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

// Cosine similarity of two L2-normalized vectors = their dot product.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
