//! WebSocket session: binary PCM16 audio in, structured JSON events out.
//!
//! Protocol (client -> server):
//!   text  {"type":"start"}            begin a voice session
//!   text  {"type":"flush"}            force-finalize the current utterance
//!   text  {"type":"stop"}             end the session
//!   binary PCM16 LE mono @ 16 kHz     streamed microphone audio
//!
//! Protocol (server -> client):
//!   {"type":"hello", ...}             capabilities on connect
//!   {"type":"status","stage":...}     pipeline stage transitions
//!   {"type":"transcript","text":...}  final Whisper transcript of the turn
//!   {"type":"sources","sources":[..]} retrieved documents for the turn
//!   {"type":"ttft","ms":...}          time-to-first-token + KV cache stats
//!   {"type":"token","value":...}      one streamed LLM token
//!   {"type":"tts","data":...}         base64 WAV when server-side TTS is on
//!   {"type":"answer_done", ...}       turn summary with latency metrics
//!   {"type":"error","message":...}

use crate::audio::{pcm16_to_wav, pcm16_rms};
use crate::error::EchoError;
use crate::llm::StreamEvent;
use crate::rag::format_context_block;
use crate::state::AppState;
use actix_ws::{Message, MessageStream, Session};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

const SAMPLE_RATE: u32 = 16000;
/// Hard cap on a single utterance buffer (ms) before auto-flush.
const MAX_UTTERANCE_MS: u64 = 12_000;
/// Consecutive silent chunks that end an utterance (server-side VAD).
const SILENCE_CHUNKS_TO_FLUSH: u32 = 3;
/// Minimum cosine similarity for a retrieved chunk to be included.
const MIN_RETRIEVAL_SCORE: f32 = 0.2;

pub async fn ws_index(
    req: actix_web::HttpRequest,
    stream: actix_web::web::Payload,
    data: actix_web::web::Data<Arc<AppState>>,
) -> Result<actix_web::HttpResponse, actix_web::Error> {
    let (resp, session, msg_stream) = actix_ws::handle(&req, stream)?;
    let state = data.get_ref().clone();
    actix_web::rt::spawn(run_session(state, session, msg_stream));
    Ok(resp)
}

async fn send(session: &Session, event: Value) {
    if session.text(event.to_string()).await.is_err() {
        tracing::debug!("client disconnected during send");
    }
}

/// Energy-based VAD over the buffered utterance.
struct VadState {
    buffer: Vec<u8>,
    speaking: bool,
    silent_chunks: u32,
}

impl VadState {
    fn new() -> Self {
        Self { buffer: Vec::new(), speaking: false, silent_chunks: 0 }
    }

    fn buffered_ms(&self) -> u64 {
        (self.buffer.len() / 2) as u64 * 1000 / SAMPLE_RATE as u64
    }

    fn should_flush(&self) -> bool {
        self.buffered_ms() >= MAX_UTTERANCE_MS
            || (self.speaking && self.silent_chunks >= SILENCE_CHUNKS_TO_FLUSH)
    }

    fn take_if_any(&mut self) -> Option<Vec<u8>> {
        if self.buffer.len() < 4000 {
            self.buffer.clear();
            self.speaking = false;
            self.silent_chunks = 0;
            return None; // < 125 ms of audio: stray noise, ignore
        }
        Some(std::mem::take(&mut self.buffer))
    }

    fn push(&mut self, pcm: &[u8], threshold: f32) {
        self.buffer.extend_from_slice(pcm);
        let rms = pcm16_rms(pcm);
        if rms > threshold {
            self.speaking = true;
            self.silent_chunks = 0;
        } else if self.speaking {
            self.silent_chunks += 1;
        }
    }
}

/// Main per-connection loop: multiplexes inbound WebSocket frames with
/// turn-completion notifications from spawned pipeline tasks.
async fn run_session(state: Arc<AppState>, session: Session, mut stream: MessageStream) {
    state.active_sessions.fetch_add(1, Ordering::Relaxed);
    let dialogue = Arc::new(tokio::sync::Mutex::new(Vec::<Value>::new()));
    let mut vad = VadState::new();
    let mut busy = false;
    let (done_tx, mut done_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

    send(
        &session,
        json!({
            "type": "hello",
            "stt_model": state.cfg.whisper_model,
            "llm_model": state.cfg.vllm_model,
            "tts_provider": state.cfg.tts_provider,
            "vad_rms_threshold": state.cfg.vad_rms_threshold,
        }),
    )
    .await;

    loop {
        tokio::select! {
            msg = stream.next() => match msg {
                Some(Ok(Message::Binary(bytes))) => {
                    if !busy {
                        vad.push(&bytes, state.cfg.vad_rms_threshold);
                        if vad.should_flush() {
                            if let Some(pcm) = vad.take_if_any() {
                                busy = true;
                                tokio::spawn(run_turn(
                                    state.clone(),
                                    session.clone(),
                                    pcm,
                                    dialogue.clone(),
                                    done_tx.clone(),
                                ));
                            }
                        }
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    let Ok(cmd) = serde_json::from_str::<Value>(&text) else { continue };
                    match cmd["type"].as_str() {
                        Some("start") => {
                            vad = VadState::new();
                            send(&session, json!({"type":"status","stage":"listening"})).await;
                        }
                        Some("flush") => {
                            if !busy {
                                if let Some(pcm) = vad.take_if_any() {
                                    busy = true;
                                    tokio::spawn(run_turn(
                                        state.clone(),
                                        session.clone(),
                                        pcm,
                                        dialogue.clone(),
                                        done_tx.clone(),
                                    ));
                                }
                            }
                        }
                        Some("stop") | Some("close") => {
                            let _ = session.close(None).await;
                            break;
                        }
                        _ => {}
                    }
                }
                Some(Ok(Message::Ping(bytes))) => {
                    let _ = session.pong(&bytes).await;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "ws receive error");
                    break;
                }
                _ => {}
            },

            Some(_) = done_rx.recv() => {
                // The turn task finished; resume ingesting microphone audio.
                busy = false;
                send(&session, json!({"type":"status","stage":"listening"})).await;
            }
        }
    }

    state.active_sessions.fetch_sub(1, Ordering::Relaxed);
}

/// Wrapper that runs one full voice turn and always signals completion.
async fn run_turn(
    state: Arc<AppState>,
    session: Session,
    pcm: Vec<u8>,
    dialogue: Arc<tokio::sync::Mutex<Vec<Value>>>,
    done_tx: tokio::sync::mpsc::UnboundedSender<()>,
) {
    let result = execute_turn(&state, &session, &pcm, &dialogue).await;
    let _ = done_tx.send(());
    if let Err(e) = result {
        tracing::error!(error = %e, "turn failed");
        send(&session, json!({"type": "error", "message": e.to_string()})).await;
    }
}

/// The full voice RAG pipeline for one utterance:
/// STT -> embed -> retrieve -> cache-aware prompt -> streamed LLM -> TTS.
async fn execute_turn(
    state: &Arc<AppState>,
    session: &Session,
    pcm: &[u8],
    dialogue: &Arc<tokio::sync::Mutex<Vec<Value>>>,
) -> Result<Option<(String, String)>, EchoError> {
    let turn_started = Instant::now();
    let wav = pcm16_to_wav(pcm, SAMPLE_RATE, 1);

    // ---------------------------------------------------------------- STT --
    send(session, json!({"type": "status", "stage": "stt"})).await;
    let user_text = state.whisper.transcribe(&wav).await?;
    if user_text.is_empty() {
        return Ok(None);
    }
    send(session, json!({"type": "transcript", "text": user_text})).await;

    // ------------------------------------------------------------ Retrieve -
    send(session, json!({"type": "status", "stage": "retrieval"})).await;
    let query_vec = state.embedder.embed(&user_text).await?;
    let raw_hits = state.qdrant.search(&query_vec, state.cfg.top_k).await?;
    let hits = crate::llm::stable_hits(&raw_hits, MIN_RETRIEVAL_SCORE);

    send(
        session,
        json!({
            "type": "sources",
            "sources": hits.iter().map(|h| json!({
                "source": h.payload.source,
                "score": h.score,
                "snippet": h.payload.text.chars().take(160).collect::<String>(),
            })).collect::<Vec<_>>(),
        }),
    )
    .await;

    let docs_block = if hits.is_empty() {
        String::new()
    } else {
        format_context_block(&hits)
    };

    // ---------------------------------------------------------------- LLM --
    send(session, json!({"type": "status", "stage": "llm"})).await;
    let dialogue_snapshot = dialogue.lock().await.clone();
    let messages =
        state
            .prompts
            .build_messages(&docs_block, &dialogue_snapshot, &user_text);
    let mut answer = String::new();
    let (mut prompt_tokens, mut cached_tokens, mut ttft_ms) = (0u64, 0u64, 0u128);

    let stream = state.vllm.stream_chat(messages).await?;
    tokio::pin!(stream);
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::FirstToken { token, ttft_ms: t, prompt_tokens: p, cached_tokens: c } => {
                ttft_ms = t;
                prompt_tokens = p;
                cached_tokens = c;
                answer.push_str(&token);
                send(session, json!({
                    "type": "ttft",
                    "ms": t,
                    "prompt_tokens": p,
                    "cached_tokens": c,
                }))
                .await;
                send(session, json!({"type": "token", "value": token})).await;
            }
            StreamEvent::Token(tok) => {
                answer.push_str(&tok);
                send(session, json!({"type": "token", "value": tok})).await;
            }
            StreamEvent::Done { prompt_tokens: p, cached_tokens: c } => {
                if p > 0 {
                    prompt_tokens = p;
                }
                if c > 0 {
                    cached_tokens = c;
                }
            }
        }
    }

    let answer = answer.trim().to_string();

    // ---------------------------------------------------------------- TTS --
    if state.tts.enabled() && !answer.is_empty() {
        send(session, json!({"type": "status", "stage": "tts"})).await;
        match state.tts.synthesize(&answer).await {
            Ok(wav_bytes) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);
                send(session, json!({"type": "tts", "format": "wav", "data": b64})).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "server TTS failed, falling back to browser voice");
            }
        }
    }

    // ------------------------------------------------------------- Metrics -
    let cache_ratio = if prompt_tokens > 0 {
        cached_tokens as f64 / prompt_tokens as f64
    } else {
        0.0
    };
    state.turns_total.fetch_add(1, Ordering::Relaxed);
    state
        .prompt_tokens_total
        .fetch_add(prompt_tokens, Ordering::Relaxed);
    state
        .cached_tokens_total
        .fetch_add(cached_tokens, Ordering::Relaxed);

    send(
        session,
        json!({
            "type": "answer_done",
            "answer": answer,
            "elapsed_ms": turn_started.elapsed().as_millis() as u64,
            "ttft_ms": ttft_ms as u64,
            "prompt_tokens": prompt_tokens,
            "cached_tokens": cached_tokens,
            "cache_hit_ratio": cache_ratio,
            "sources_used": hits.len(),
        }),
    )
    .await;

    // Persist the turn into the short dialogue memory (after the prompt was
    // built, so the cached prefix for *next* turn only shifts by this turn).
    {
        let mut dlg = dialogue.lock().await;
        dlg.push(json!({"role": "user", "content": user_text}));
        dlg.push(json!({"role": "assistant", "content": answer}));
        let max_entries = 8;
        if dlg.len() > max_entries {
            let drop = dlg.len() - max_entries;
            dlg.drain(..drop);
        }
    }

    Ok(Some((user_text, answer)))
}
