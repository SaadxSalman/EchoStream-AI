//! vLLM OpenAI-compatible streaming client (`/chat/completions`, SSE).

use crate::config::Config;
use crate::error::{EchoError, EchoResult};
use crate::llm::prompt_cache::extract_cache_stats;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Instant;

/// One streamed chunk produced by the model.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// First token observed for this request (TTFT signal).
    FirstToken {
        token: String,
        ttft_ms: u128,
        prompt_tokens: u64,
        cached_tokens: u64,
    },
    Token(String),
    Done {
        prompt_tokens: u64,
        cached_tokens: u64,
    },
}

pub struct VllmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    temperature: f32,
    max_tokens: u32,
}

impl VllmClient {
    pub fn new(cfg: &Config) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Accept", "text/event-stream".parse().unwrap());
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url: cfg.vllm_base_url.trim_end_matches('/').to_string(),
            api_key: cfg.vllm_api_key.clone(),
            model: cfg.vllm_model.clone(),
            temperature: cfg.temperature,
            max_tokens: cfg.max_tokens,
        }
    }

    /// Stream a chat completion. Yields `StreamEvent`s; SSE parsing is done
    /// incrementally so tokens are forwarded to the browser the moment they
    /// leave vLLM's decode loop.
    pub async fn stream_chat(
        &self,
        messages: Vec<Value>,
    ) -> EchoResult<futures_util::stream::BoxStream<'static, StreamEvent>> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "temperature": self.temperature,
            "max_tokens": self.max_tokens,
            "stream": true,
            "stream_options": { "include_usage": true },
        });

        let mut rb = self.http.post(format!("{}/chat/completions", self.base_url));
        if !self.api_key.is_empty() {
            rb = rb.bearer_auth(&self.api_key);
        }
        let resp = rb
            .json(&body)
            .send()
            .await
            .map_err(|e| EchoError::Llm(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(EchoError::Llm(format!("vLLM returned {status}: {text}")));
        }

        let started = Instant::now();
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();
        let mut first_sent = false;
        let mut usage: Value = Value::Null;
        let finished = false;

        let out = futures_util::stream::unfold(
            (stream, buffer, first_sent, usage, started, finished),
            |(mut stream, mut buffer, mut first_sent, mut usage, started, mut finished)| async move {
                if finished {
                    return None;
                }
                loop {
                    if let Some(pos) = buffer.find("\n\n") {
                        let event = buffer[..pos].to_string();
                        buffer.drain(..pos + 2);
                        for line in event.lines() {
                            let Some(data) = line.strip_prefix("data: ") else {
                                continue;
                            };
                            if data.trim() == "[DONE]" {
                                let (p, c) = extract_cache_stats(&usage);
                                finished = true;
                                return Some((
                                    StreamEvent::Done {
                                        prompt_tokens: p,
                                        cached_tokens: c,
                                    },
                                    (stream, buffer, first_sent, usage, started, finished),
                                ));
                            }
                            let Ok(v) = serde_json::from_str::<Value>(data) else {
                                continue;
                            };
                            if !v["usage"].is_null() {
                                usage = v["usage"].clone();
                            }
                            let delta = &v["choices"][0]["delta"]["content"];
                            let Some(tok) = delta.as_str() else {
                                continue;
                            };
                            if tok.is_empty() {
                                continue;
                            }
                            if !first_sent {
                                first_sent = true;
                                let (p, c) = extract_cache_stats(&usage);
                                return Some((
                                    StreamEvent::FirstToken {
                                        token: tok.to_string(),
                                        ttft_ms: started.elapsed().as_millis(),
                                        prompt_tokens: p,
                                        cached_tokens: c,
                                    },
                                    (stream, buffer, first_sent, usage, started, finished),
                                ));
                            }
                            return Some((
                                StreamEvent::Token(tok.to_string()),
                                (stream, buffer, first_sent, usage, started, finished),
                            ));
                        }
                        continue;
                    }
                    match stream.next().await {
                        Some(Ok(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));
                        }
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "vLLM stream error");
                            return None;
                        }
                        None => {
                            let (p, c) = extract_cache_stats(&usage);
                            finished = true;
                            return Some((
                                StreamEvent::Done {
                                    prompt_tokens: p,
                                    cached_tokens: c,
                                },
                                (stream, buffer, first_sent, usage, started, finished),
                            ));
                        }
                    }
                }
            },
        );

        Ok(Box::pin(out))
    }

    /// Non-streaming completion used once at startup to warm the prefix
    /// cache blocks for the system prompt (and static context) so the very
    /// first real user turn already hits warm KV blocks.
    pub async fn warmup(&self, messages: Vec<Value>) -> EchoResult<()> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "max_tokens": 1,
            "temperature": 0.0,
        });
        let mut rb = self.http.post(format!("{}/chat/completions", self.base_url));
        if !self.api_key.is_empty() {
            rb = rb.bearer_auth(&self.api_key);
        }
        let resp = rb
            .json(&body)
            .send()
            .await
            .map_err(|e| EchoError::Llm(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(EchoError::Llm(format!("warmup returned {status}: {text}")));
        }
        tracing::info!("prefix-cache warmup completion completed");
        Ok(())
    }
}
