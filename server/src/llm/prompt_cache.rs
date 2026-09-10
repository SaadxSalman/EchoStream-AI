//! Prompt-cache-aware prompt construction.
//!
//! vLLM's Automatic Prefix Caching (and LMCache, its KV-cache spill tier)
//! operate on *identical token prefix blocks*. This module guarantees the
//! byte-for-byte stability of the leading blocks of every request:
//!
//!   block 0: SYSTEM_PROMPT            (constant, reused by every turn/session)
//!   block 1: STATIC KNOWLEDGE HEADER  (constants, per deployment)
//!   block 2: RETRIEVED DOCUMENTS      (stable ordering -> often reused when
//!                                      consecutive queries hit similar docs)
//!   block 3: DIALOGUE                 (per-turn, never cached — it changes)
//!
//! Because blocks 0-2 are emitted *before* any per-turn content, the KV
//! blocks they produce are computed exactly once and served from cache for
//! every subsequent request, collapsing prefill time (dominant share of
//! TTFT for RAG prompts) to a cache lookup.

use crate::rag::SearchHit;
use serde_json::{json, Value};

pub struct PromptBuilder {
    pub system_prompt: String,
    /// Optional static domain knowledge prepended right after the system
    /// prompt (e.g. product catalog digest). Identical across all requests.
    pub static_context: String,
}

impl PromptBuilder {
    /// Messages for a user turn. The prefix (system + static context +
    /// documents) is rendered identically for identical inputs.
    pub fn build_messages(&self, docs_block: &str, dialogue: &[Value], user_text: &str) -> Vec<Value> {
        let mut user_content = String::new();
        if !self.static_context.is_empty() {
            user_content.push_str("STATIC CONTEXT\n==============\n");
            user_content.push_str(self.static_context.trim());
            user_content.push_str("\n\n");
        }
        if !docs_block.is_empty() {
            user_content.push_str(docs_block.trim());
            user_content.push_str("\n\n");
        }
        user_content.push_str("USER QUESTION\n=============\n");
        user_content.push_str(user_text.trim());

        let mut msgs = vec![json!({ "role": "system", "content": self.system_prompt })];
        // Replay the short dialogue history *after* the cached prefix.
        for m in dialogue {
            let role = m["role"].as_str().unwrap_or("user");
            let content = m["content"].as_str().unwrap_or_default();
            if role == "user" || role == "assistant" {
                msgs.push(json!({ "role": role, "content": content }));
            }
        }
        msgs.push(json!({ "role": "user", "content": user_content }));
        msgs
    }

    /// Number of leading characters that are cache-stable (system + static
    /// context). Useful for metrics/logging the expected cache hit ratio.
    pub fn stable_prefix_len(&self) -> usize {
        self.system_prompt.len() + self.static_context.len()
    }
}

/// Extract `prompt_cache_hit_tokens` style stats that vLLM reports through
/// the OpenAI-compatible `usage` object (`prompt_tokens_details.cached_tokens`
/// or `cached_tokens`). Used for the TTFT/cache metrics surfaced in the UI.
pub fn extract_cache_stats(usage: &Value) -> (u64, u64) {
    let prompt_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let cached = usage["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| usage["cached_tokens"].as_u64())
        .unwrap_or(0);
    (prompt_tokens, cached)
}

/// Filter + order retrieval hits so that the documents block is deterministic.
pub fn stable_hits(hits: &[SearchHit], min_score: f32) -> Vec<SearchHit> {
    let mut hits: Vec<SearchHit> = hits
        .iter()
        .filter(|h| h.score >= min_score)
        .cloned()
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.payload.chunk_index.cmp(&b.payload.chunk_index))
    });
    hits
}
