//! Shared application state.

use crate::config::Config;
use crate::llm::{PromptBuilder, VllmClient};
use crate::rag::{Embedder, Qdrant};
use crate::stt::GroqWhisper;
use crate::tts::TtsClient;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;

pub struct AppState {
    pub cfg: Config,
    pub whisper: GroqWhisper,
    pub embedder: Embedder,
    pub qdrant: Qdrant,
    pub vllm: VllmClient,
    pub prompts: PromptBuilder,
    pub tts: TtsClient,

    // Live metrics (surfaced on /api/health).
    pub active_sessions: AtomicUsize,
    pub turns_total: AtomicU64,
    pub prompt_tokens_total: AtomicU64,
    pub cached_tokens_total: AtomicU64,
    pub started_at: std::time::Instant,
}
