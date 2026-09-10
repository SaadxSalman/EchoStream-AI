//! Server-side TTS via Groq PlayAI (`/audio/speech`), returning raw bytes of
//! a WAV file which the browser can decode and play immediately. When
//! `TTS_PROVIDER=browser` the client uses the Web Speech API instead and this
//! module stays idle.

use crate::config::Config;
use crate::error::{EchoError, EchoResult};

pub struct TtsClient {
    http: reqwest::Client,
    enabled: bool,
    base_url: String,
    api_key: String,
    model: String,
    voice: String,
}

impl TtsClient {
    pub fn new(cfg: &Config) -> Self {
        Self {
            http: reqwest::Client::new(),
            enabled: cfg.tts_provider.eq_ignore_ascii_case("groq"),
            base_url: cfg.groq_base_url.trim_end_matches('/').to_string(),
            api_key: cfg.groq_api_key.clone(),
            model: cfg.tts_model.clone(),
            voice: cfg.tts_voice.clone(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Synthesize `text` to a WAV container. Groq PlayAI returns WAV for
    /// `playai-tts` models.
    pub async fn synthesize(&self, text: &str) -> EchoResult<Vec<u8>> {
        if !self.enabled {
            return Err(EchoError::Tts("server TTS disabled".into()));
        }
        // PlayAI TTS caps input at ~10k chars; clamp defensively.
        let text: String = text.chars().take(9000).collect();
        let resp = self
            .http
            .post(format!("{}/audio/speech", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "model": self.model,
                "voice": self.voice,
                "input": text,
                "response_format": "wav",
            }))
            .send()
            .await
            .map_err(|e| EchoError::Tts(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let t = resp.text().await.unwrap_or_default();
            return Err(EchoError::Tts(format!("Groq TTS returned {status}: {t}")));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| EchoError::Tts(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}
