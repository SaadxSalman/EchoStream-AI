//! Central configuration, loaded from environment variables / `.env`.
//!
//! Every external dependency (Groq Whisper, vLLM, Qdrant, embeddings) is
//! expressed here as a plain URL + optional key so the whole pipeline can be
//! pointed at any OpenAI-compatible endpoint.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// Bind address for the Actix-web server, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// Directory containing the static web frontend (served at `/`).
    pub web_dir: String,

    // ---------------------------------------------------------------- STT --
    /// Groq API base URL (OpenAI-compatible audio endpoint).
    pub groq_base_url: String,
    /// Groq API key.
    pub groq_api_key: String,
    /// Whisper model served by Groq, e.g. `whisper-large-v3-turbo`.
    pub whisper_model: String,

    // ---------------------------------------------------------------- LLM --
    /// vLLM OpenAI-compatible base URL, e.g. `http://vllm:8000/v1`.
    pub vllm_base_url: String,
    /// Chat model name as served by vLLM.
    pub vllm_model: String,
    /// Optional bearer token for vLLM (empty if the server is unprotected).
    pub vllm_api_key: String,
    /// Sampling temperature for generation.
    pub temperature: f32,
    /// Maximum tokens generated per answer.
    pub max_tokens: u32,
    /// Static system prompt. This is the *first* block of every request so
    /// vLLM's automatic prefix caching (and LMCache) can reuse its KV blocks
    /// across every session and turn.
    pub system_prompt: String,
    /// If true, fire a one-shot "cache warmup" completion at startup that
    /// pre-populates the prefix cache blocks for the system prompt.
    pub warm_prefix_cache: bool,

    // ---------------------------------------------------------------- RAG --
    /// Qdrant REST base URL, e.g. `http://qdrant:6333`.
    pub qdrant_url: String,
    /// Optional Qdrant API key.
    pub qdrant_api_key: String,
    /// Qdrant collection name.
    pub qdrant_collection: String,
    /// Number of retrieved chunks per query.
    pub top_k: usize,

    // -------------------------------------------------------- Embeddings ---
    /// `openai` (OpenAI-compatible /embeddings endpoint) or `hash`
    /// (deterministic local hash embedding for offline development).
    pub embedding_provider: String,
    /// OpenAI-compatible embeddings base URL.
    pub embedding_base_url: String,
    /// Embedding model name.
    pub embedding_model: String,
    /// API key for the embeddings endpoint.
    pub embedding_api_key: String,
    /// Dimension of the embedding space (required for the `hash` provider).
    pub embedding_dim: usize,

    // ---------------------------------------------------------------- TTS --
    /// `browser` (client-side speechSynthesis) or `groq` (server-side
    /// Groq PlayAI TTS streamed back as base64 PCM).
    pub tts_provider: String,
    /// Groq TTS model (only used when `tts_provider == groq`).
    pub tts_model: String,
    /// Groq TTS voice (only used when `tts_provider == groq`).
    pub tts_voice: String,

    // ---------------------------------------------------------------- Misc -
    /// Silence threshold (RMS, int16 scale) used by the server-side VAD.
    pub vad_rms_threshold: f32,
    /// HTTP timeout for outbound non-streaming calls.
    pub http_timeout: Duration,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_parse<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            web_dir: env_or("WEB_DIR", "../web"),

            groq_base_url: env_or("GROQ_BASE_URL", "https://api.groq.com/openai/v1"),
            groq_api_key: env_or("GROQ_API_KEY", ""),
            whisper_model: env_or("WHISPER_MODEL", "whisper-large-v3-turbo"),

            vllm_base_url: env_or("VLLM_BASE_URL", "http://localhost:8000/v1"),
            vllm_model: env_or("VLLM_MODEL", "meta-llama/Llama-3.1-8B-Instruct"),
            vllm_api_key: env_or("VLLM_API_KEY", ""),
            temperature: env_parse("TEMPERATURE", 0.3f32),
            max_tokens: env_parse("MAX_TOKENS", 512u32),
            system_prompt: env_or(
                "SYSTEM_PROMPT",
                "You are EchoStream, a voice-first customer support agent. \
                 Answer strictly from the provided support documents. Be concise: \
                 1-3 short spoken sentences. Never use markdown, lists or emojis. \
                 If the documents do not contain the answer, say so honestly.",
            ),
            warm_prefix_cache: env_parse("WARM_PREFIX_CACHE", true),

            qdrant_url: env_or("QDRANT_URL", "http://localhost:6333"),
            qdrant_api_key: env_or("QDRANT_API_KEY", ""),
            qdrant_collection: env_or("QDRANT_COLLECTION", "support_docs"),
            top_k: env_parse("TOP_K", 4usize),

            embedding_provider: env_or("EMBEDDING_PROVIDER", "hash"),
            embedding_base_url: env_or("EMBEDDING_BASE_URL", "https://api.openai.com/v1"),
            embedding_model: env_or("EMBEDDING_MODEL", "text-embedding-3-small"),
            embedding_api_key: env_or("EMBEDDING_API_KEY", ""),
            embedding_dim: env_parse("EMBEDDING_DIM", 512usize),

            tts_provider: env_or("TTS_PROVIDER", "browser"),
            tts_model: env_or("TTS_MODEL", "playai-tts"),
            tts_voice: env_or("TTS_VOICE", "Celeste-PlayAI"),

            vad_rms_threshold: env_parse("VAD_RMS_THRESHOLD", 350.0f32),
            http_timeout: Duration::from_secs(env_parse("HTTP_TIMEOUT_SECS", 30u64)),
        })
    }
}
