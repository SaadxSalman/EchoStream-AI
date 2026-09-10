//! Shared error type.

use actix_web::{HttpResponse, ResponseError};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum EchoError {
    #[error("STT error: {0}")]
    Stt(String),

    #[error("Embedding error: {0}")]
    Embedding(String),

    #[error("Vector store error: {0}")]
    VectorStore(String),

    #[error("LLM error: {0}")]
    Llm(String),

    #[error("TTS error: {0}")]
    Tts(String),

    #[error("WebSocket error: {0}")]
    Ws(String),

    #[error("HTTP client error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl ResponseError for EchoError {
    fn error_response(&self) -> HttpResponse {
        HttpResponse::InternalServerError().json(json!({
            "error": self.to_string(),
        }))
    }
}

pub type EchoResult<T> = Result<T, EchoError>;
