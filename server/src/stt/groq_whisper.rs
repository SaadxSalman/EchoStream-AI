//! Groq Whisper speech-to-text client.
//!
//! Audio is delivered as a buffered WAV file (PCM16, 16 kHz mono) through
//! Groq's OpenAI-compatible `/audio/transcriptions` endpoint.

use crate::config::Config;
use crate::error::{EchoError, EchoResult};
use serde_json::Value;
use std::time::Instant;

pub struct GroqWhisper {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl GroqWhisper {
    pub fn new(cfg: &Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: cfg.groq_base_url.trim_end_matches('/').to_string(),
            api_key: cfg.groq_api_key.clone(),
            model: cfg.whisper_model.clone(),
        }
    }

    /// Transcribe a buffered WAV payload. Returns the plain transcript text.
    pub async fn transcribe(&self, wav: &[u8]) -> EchoResult<String> {
        if wav.is_empty() {
            return Err(EchoError::Stt("empty audio buffer".into()));
        }
        let started = Instant::now();

        let part = reqwest::multipart::Part::bytes(wav.to_vec())
            .file_name("chunk.wav")
            .mime_str("audio/wav")
            .map_err(|e| EchoError::Stt(e.to_string()))?;

        let form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json")
            .text("temperature", "0")
            .part("file", part);

        let resp = self
            .http
            .post(format!("{}/audio/transcriptions", self.base_url))
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| EchoError::Stt(e.to_string()))?;

        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| EchoError::Stt(format!("bad STT response ({status}): {e}")))?;

        if !status.is_success() {
            return Err(EchoError::Stt(format!("Groq STT returned {status}: {body}")));
        }

        let text = body["text"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();

        tracing::info!(
            target: "pipeline::stt",
            bytes = wav.len(),
            ms = started.elapsed().as_millis() as u64,
            chars = text.chars().count(),
            "transcribed"
        );

        Ok(text)
    }
}
