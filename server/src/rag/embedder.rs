//! Embedding backends.
//!
//! * `openai` — any OpenAI-compatible `/embeddings` endpoint (vLLM can serve
//!   embedding models, as can OpenAI, Together, Nomic's server, etc.).
//! * `hash`   — deterministic local feature-hashing embedding. Zero external
//!   dependencies, so the full pipeline runs offline. Good enough for dev,
//!   not for production retrieval quality.

use crate::config::Config;
use crate::error::{EchoError, EchoResult};
use serde_json::json;

#[derive(Clone)]
pub enum Embedder {
    OpenAi {
        http: reqwest::Client,
        base_url: String,
        api_key: String,
        model: String,
    },
    Hash {
        dim: usize,
    },
}

impl Embedder {
    pub fn new(cfg: &Config) -> Self {
        match cfg.embedding_provider.as_str() {
            "openai" => Embedder::OpenAi {
                http: reqwest::Client::new(),
                base_url: cfg.embedding_base_url.trim_end_matches('/').to_string(),
                api_key: cfg.embedding_api_key.clone(),
                model: cfg.embedding_model.clone(),
            },
            _ => Embedder::Hash {
                dim: cfg.embedding_dim,
            },
        }
    }

    pub async fn embed(&self, text: &str) -> EchoResult<Vec<f32>> {
        match self {
            Embedder::OpenAi {
                http,
                base_url,
                api_key,
                model,
            } => {
                let resp = http
                    .post(format!("{base_url}/embeddings"))
                    .bearer_auth(api_key)
                    .json(&json!({ "model": model, "input": text }))
                    .send()
                    .await
                    .map_err(|e| EchoError::Embedding(e.to_string()))?;
                let status = resp.status();
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| EchoError::Embedding(e.to_string()))?;
                if !status.is_success() {
                    return Err(EchoError::Embedding(format!(
                        "embeddings returned {status}: {body}"
                    )));
                }
                let vec: Vec<f32> = serde_json::from_value(body["data"][0]["embedding"].clone())
                    .map_err(|e| EchoError::Embedding(e.to_string()))?;
                Ok(vec)
            }
            Embedder::Hash { dim } => Ok(hash_embed(text, *dim)),
        }
    }
}

/// Deterministic feature-hashing embedding with sub-word shingles so it is
/// reasonably stable to small rewordings.
pub fn hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; dim];
    let normalized = text.to_lowercase();
    for (i, word) in normalized.split_whitespace().collect::<Vec<_>>().iter().enumerate() {
        // Unigram.
        bump(&mut out, word, dim, 1.0);
        // Bigram with the previous word.
        if i > 0 {
            let prev = normalized.split_whitespace().nth(i - 1).unwrap();
            bump(&mut out, &format!("{prev}_{word}"), dim, 0.7);
        }
        // Character 4-grams (robust to typos).
        let chars: Vec<char> = word.chars().collect();
        for w in chars.windows(4) {
            let s: String = w.iter().collect();
            bump(&mut out, &format!("#{s}"), dim, 0.25);
        }
    }
    // L2 normalize.
    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut out {
            *v /= norm;
        }
    }
    out
}

fn bump(out: &mut [f32], feature: &str, dim: usize, weight: f32) {
    let h = fxhash(feature.as_bytes());
    let idx = (h as usize) % dim;
    let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
    out[idx] += sign * weight;
}

fn fxhash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}
