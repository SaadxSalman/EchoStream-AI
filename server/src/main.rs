//! EchoStream-AI server entrypoint: static frontend + health API + WS.

use echostream_agent::llm::{PromptBuilder, VllmClient};
use echostream_agent::rag::{Embedder, Qdrant};
use echostream_agent::state::AppState;
use echostream_agent::stt::GroqWhisper;
use echostream_agent::tts::TtsClient;
use echostream_agent::{config, ws};

use actix_files::Files;
use actix_web::web::Data;
use actix_web::{get, App, HttpResponse, HttpServer};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

#[get("/api/health")]
async fn health(data: Data<Arc<AppState>>) -> HttpResponse {
    let prompt = data.prompt_tokens_total.load(Ordering::Relaxed);
    let cached = data.cached_tokens_total.load(Ordering::Relaxed);
    HttpResponse::Ok().json(json!({
        "status": "ok",
        "uptime_secs": data.started_at.elapsed().as_secs(),
        "active_sessions": data.active_sessions.load(Ordering::Relaxed),
        "turns_total": data.turns_total.load(Ordering::Relaxed),
        "prompt_tokens_total": prompt,
        "cached_tokens_total": cached,
        "cache_hit_ratio": if prompt > 0 { cached as f64 / prompt as f64 } else { 0.0 },
        "stt_model": data.cfg.whisper_model,
        "llm_model": data.cfg.vllm_model,
        "vllm_url": data.cfg.vllm_base_url,
        "qdrant_url": data.cfg.qdrant_url,
        "qdrant_collection": data.cfg.qdrant_collection,
        "embedding_provider": data.cfg.embedding_provider,
        "tts_provider": data.cfg.tts_provider,
    }))
}

fn build_state(cfg: config::Config) -> Arc<AppState> {
    let vllm = VllmClient::new(&cfg);
    let prompts = PromptBuilder {
        system_prompt: cfg.system_prompt.clone(),
        static_context: String::new(),
    };
    let qdrant = Qdrant::new(&cfg, cfg.embedding_dim);
    Arc::new(AppState {
        whisper: GroqWhisper::new(&cfg),
        embedder: Embedder::new(&cfg),
        qdrant,
        vllm,
        prompts,
        tts: TtsClient::new(&cfg),
        cfg,
        active_sessions: std::sync::atomic::AtomicUsize::new(0),
        turns_total: std::sync::atomic::AtomicU64::new(0),
        prompt_tokens_total: std::sync::atomic::AtomicU64::new(0),
        cached_tokens_total: std::sync::atomic::AtomicU64::new(0),
        started_at: std::time::Instant::now(),
    })
}

/// Fire one tiny completion whose prompt is exactly the stable prefix
/// (system + static context) so vLLM's prefix cache / LMCache has the KV
/// blocks hot before the first real user connects.
async fn warm_prefix_cache(state: Arc<AppState>) {
    if !state.cfg.warm_prefix_cache {
        return;
    }
    // Give vLLM a moment to come up if we started together in compose.
    for attempt in 1..=10u32 {
        let messages = vec![
            json!({"role": "system", "content": state.cfg.system_prompt}),
            json!({"role": "user", "content": "Say ready."}),
        ];
        match state.vllm.warmup(messages).await {
            Ok(()) => {
                tracing::info!("vLLM prefix cache warmed (system prompt blocks resident)");
                return;
            }
            Err(e) => {
                tracing::warn!(attempt, error = %e, "prefix cache warmup failed, retrying");
                tokio::time::sleep(Duration::from_secs(6)).await;
            }
        }
    }
    tracing::error!("prefix cache warmup gave up after 10 attempts; continuing anyway");
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,echostream_agent=debug".into()),
        )
        .init();

    let cfg = config::Config::from_env().expect("valid configuration");
    tracing::info!(bind = %cfg.bind_addr, "starting EchoStream-AI server");

    let state = build_state(cfg.clone());

    // Qdrant collection may not exist yet — create lazily on first ingest;
    // here we only probe connectivity and log it.
    {
        let st = state.clone();
        tokio::spawn(async move {
            match st.qdrant.count().await {
                Ok(n) => tracing::info!(points = n, collection = %st.cfg.qdrant_collection, "qdrant reachable"),
                Err(e) => tracing::warn!(error = %e, "qdrant not reachable yet (run echostream-ingest)"),
            }
        });
    }

    tokio::spawn(warm_prefix_cache(state.clone()));

    let web_dir = cfg.web_dir.clone();
    let bind_addr = cfg.bind_addr.clone();

    HttpServer::new(move || {
        App::new()
            .app_data(Data::new(state.clone()))
            .service(health)
            .route("/ws", actix_web::web::get().to(ws::ws_index))
            .service(Files::new("/", &web_dir).index_file("index.html"))
    })
    .workers(2)
    .bind(bind_addr)?
    .run()
    .await
}
