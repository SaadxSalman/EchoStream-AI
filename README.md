# EchoStream-AI 🎙️⚡

**Real-Time Voice-Interactive RAG Agent with Sub-Second Latency and Streaming KV Caching**

A voice-first conversational RAG pipeline built for customer support and technical
troubleshooting. You talk, and within a fraction of a second the system:

1. **streams your microphone audio** to a Rust (Actix-web) backend over WebSockets,
2. **transcribes** it with **Groq Whisper** (`whisper-large-v3-turbo`),
3. **retrieves** the most relevant knowledge-base chunks from **Qdrant** (HNSW, cosine),
4. **streams tokens** from a **vLLM**-served LLM whose **KV-cache blocks for the system
   prompt and static context are pre-computed and reused** (vLLM Automatic Prefix
   Caching + **LMCache**), and
5. **speaks the answer back** while tokens are still arriving (sentence-level
   streaming TTS).

The result is a natural, interruptible, phone-call-like support experience where the
dominant latency sources — speech-to-text round-trip and RAG prompt prefill — are
attacked simultaneously.

---

## Table of Contents

- [1. Why this project exists](#1-why-this-project-exists)
- [2. Feature highlights](#2-feature-highlights)
- [3. Architecture](#3-architecture)
- [4. The KV-cache / TTFT deep dive](#4-the-kv-cache--ttft-deep-dive)
- [5. Tech stack](#5-tech-stack)
- [6. Repository layout](#6-repository-layout)
- [7. Getting started](#7-getting-started)
- [8. Configuration reference](#8-configuration-reference)
- [9. WebSocket protocol reference](#9-websocket-protocol-reference)
- [10. The voice pipeline, stage by stage](#10-the-voice-pipeline-stage-by-stage)
- [11. Metrics and observability](#11-metrics-and-observability)
- [12. Performance tuning playbook](#12-performance-tuning-playbook)
- [13. Testing](#13-testing)
- [14. Security considerations](#14-security-considerations)
- [15. Troubleshooting](#15-troubleshooting)
- [16. Limitations and roadmap](#16-limitations-and-roadmap)
- [17. License and credits](#17-license-and-credits)

---

## 1. Why this project exists

Text chatbots are easy. **Voice agents are a latency war.**

A human conversation starts feeling "broken" when the gap between you finishing a
sentence and the other side starting to answer exceeds roughly **700–1000 ms**. A
naive voice-RAG stack blows way past that:

| Stage | Naive implementation | Typical cost |
|---|---|---|
| Capture + upload | Upload whole recordings | 500–2000 ms |
| STT | Whisper-large on CPU | 1000–5000 ms |
| Retrieval | Full-scan vector search | 20–200 ms |
| LLM prefill | Re-encode a 2–4k token RAG prompt *every turn* | 300–1500 ms |
| TTS | Wait for the *full* answer, then synthesize | +2000–5000 ms |
| **Total** | | **4–10 seconds** 💀 |

EchoStream-AI closes that gap with engineering at every stage:

- **Streaming capture** — the browser sends 16 kHz PCM16 frames every ~256 ms over a
  single WebSocket; there is no "upload" step at all.
- **Energy-based VAD on both sides** — silence is never uploaded, never transcribed;
  the utterance is finalized the moment you stop talking (~700 ms trailing silence).
- **Whisper as an API** — Groq's LPU inference runs `whisper-large-v3-turbo` in
  ~150–300 ms for a typical utterance.
- **Prefix-stable prompts + KV caching** — the system prompt and static context are
  byte-identical across *all* requests, so vLLM computes their KV blocks exactly once
  and LMCache keeps them hot. Prefill collapses to a cache lookup.
- **Token-level streaming all the way to the speaker** — TTS starts on the *first
  completed sentence*, not the end of the answer.

## 2. Feature highlights

- 🦀 **Rust backend** — Actix-web 4 + `actix-ws`, fully async on Tokio; one binary
  serves the WebSocket API, the health/metrics API, and the static frontend.
- 🎧 **AudioWorklet capture** — runs off the main thread, mono-mixed, downsampled
  with linear interpolation to 16 kHz and quantized to PCM16 in the browser.
- 🗣️ **Groq Whisper STT** — OpenAI-compatible `/audio/transcriptions` with
  `whisper-large-v3-turbo`; audio is wrapped into a WAV container server-side.
- 🔎 **Qdrant vector search** — REST client with zero heavy SDK dependencies; HNSW
  (m=32, ef_construct=256), cosine distance, payload-filtered deletes for re-ingest.
- 🧠 **vLLM streaming** — SSE parsing of `/v1/chat/completions` with
  `stream_options.include_usage`, incremental token forwarding, and
  time-to-first-token (TTFT) measurement.
- ⚡ **Streaming KV caching** — cache-stable prompt prefixing, startup prefix-cache
  warmup, deterministic retrieval ordering, and per-turn cache-hit telemetry
  (`cached_tokens / prompt_tokens`) surfaced in the UI.
- 🔊 **Two TTS modes** — zero-cost browser `speechSynthesis` with sentence-level
  streaming playback (default), or server-side **Groq PlayAI TTS** delivered as
  base64 WAV over the socket.
- 🧰 **Zero-lock-in embeddings** — use any OpenAI-compatible `/embeddings` endpoint,
  or the built-in deterministic **hash embedding** for fully offline dev (no API
  keys needed to run the whole pipeline end-to-end).
- 📊 **Live observability** — per-turn TTFT, cache-hit ratio, turn latency, sources
  used (in the UI), plus aggregate counters on `/api/health`.
- 📚 **CLI ingestion tool** — `echostream-ingest` chunks (overlap-aware,
  paragraph-preferring), embeds, and upserts `.md`/`.txt` docs; re-ingesting a
  document replaces its old chunks via payload-filtered delete.

---

## 3. Architecture

```
                                      BROWSER (web/)
  ┌────────────────────────────────────────────────────────────────────────┐
  │  Mic ─► AudioWorklet ─► downsample 48k→16k ─► PCM16 ─► client VAD ─┐    │
  │                                                                   │    │
  │  ┌──────────────────────────────────────────────────────────┐ ◄───┘    │
  │  │ WebSocket (binary: audio frames / text: JSON events)     │          │
  │  └──────────────────────────────────────────────────────────┘          │
  │  ◄── tokens (streamed) ── ◄── transcript ── ◄── sources ── ◄── metrics  │
  │  TTS: sentence-chunked speechSynthesis  OR  base64 WAV (Groq)          │
  └────────────────────────────────────────────────────────────────────────┘
                                       │
                                       ▼
                                      RUST SERVER (server/)
  ┌────────────────────────────────────────────────────────────────────────┐
  │ ws.rs        session loop, server-side VAD, utterance finalization     │
  │     │                                                                   │
  │     ▼  per-turn task                                                    │
  │ audio.rs      PCM16 → WAV container, RMS energy                         │
  │ stt/          ──► Groq Whisper  (whisper-large-v3-turbo)   ~150-300 ms  │
  │ rag/          ──► embedder (openai-compatible or local hash)             │
  │               ──► Qdrant search (top-k, cosine, HNSW)      ~5-30 ms     │
  │ llm/          ──► prompt_cache.rs: stable prefix assembly                │
  │               ──► vllm_client.rs: SSE token stream         TTFT ~100 ms │
  │ tts/          ──► Groq PlayAI TTS (optional, base64 WAV)                 │
  └────────────────────────────────────────────────────────────────────────┘
            │                        │                       │
            ▼                        ▼                       ▼
      ┌───────────┐          ┌──────────────┐        ┌──────────────┐
      │  Groq API │          │    vLLM      │        │    Qdrant    │
      │ (STT/TTS) │          │ + LMCache    │        │  (HNSW ANN)  │
      └───────────┘          │ prefix cache │        └──────────────┘
                             └──────────────┘
```

### Latency budget (happy path, typical numbers)

| # | Stage | Where | Budget |
|---|-------|-------|--------|
| 1 | Capture + client VAD | browser worklet | ~256 ms chunking (overlapped with speech) |
| 2 | Trailing-silence detect | server VAD | ~700 ms after last word |
| 3 | WAV wrap + STT round-trip | Rust → Groq | 150–300 ms |
| 4 | Embed + Qdrant search | Rust → Qdrant | 5–30 ms |
| 5 | Prompt assembly | Rust, in-memory | <1 ms |
| 6 | Prefill (cache-hit prefix) | vLLM + LMCache | ~30–120 ms **with cache**, 300+ ms cold |
| 7 | First token + first sentence | vLLM stream | ~100–200 ms per sentence |
| 8 | Speech playback starts | browser TTS | ~instant once sentence lands |

**Perceived response latency ≈ stage 2 + 3 + 4 + 6 + half of 7 ≈ 1.0–1.4 s**, and the
first spoken words land while later sentences are still being generated.

---

## 4. The KV-cache / TTFT deep dive

This is the heart of the project. In an LLM, *prefill* — processing the prompt and
producing the key/value (KV) tensors for every token — is the dominant per-request
cost for RAG workloads, because a RAG prompt is long and mostly **repeated** across
turns. Two mechanisms exploit that repetition:

### 4.1 vLLM Automatic Prefix Caching (APC)

vLLM (started with `--enable-prefix-caching`, wired up in `docker-compose.yml`)
hashes the prompt in **block-sized chunks** (typically 16 tokens). If a new request
begins with blocks whose hashes are already resident, vLLM **reuses their KV tensors
instead of recomputing them**. A shared system prompt is therefore computed once and
served from cache for every subsequent request of every user.

### 4.2 LMCache — a KV-cache tier beyond one engine

vLLM can additionally offload/share KV blocks through **LMCache** by setting a
kv-transfer config (see the `VLLM_KV_CACHE_TRANSFER_CONFIG` env var in the compose
file):

```
{"kv_connector":"LMCacheConnectorV1","kv_role":"kv_producer_and_consumer"}
```

LMCache keeps evicted/infrequent KV blocks in CPU RAM (and optionally Redis/local
disk), so the prefix survives engine restarts, multi-instance deployments, and
even different machines — a proper *cache hierarchy* for KV tensors.

### 4.3 What EchoStream-AI does to make caching actually work

Caches only help if the prompts are **byte-identical up front**. The Rust code is
engineered around that invariant:

1. **`llm/prompt_cache.rs` — block-stable prompt layout.** Messages are ordered:
   `[system prompt] → [dialogue history] → [static context + retrieved docs + user
   question]`. The system prompt (constant) is the very first token block. Dialogue
   history — which changes per turn — is deliberately placed *after* everything
   cache-stable in the *system* slot so it never invalidates the prefix.
2. **`rag::stable_hits()` — deterministic retrieval ordering.** Hits are filtered by
   a minimum score and sorted by `(score desc, chunk_index asc)`. Consecutive
   questions about the same topic retrieve the same documents in the same order →
   the documents block is byte-identical → its KV blocks are reused too.
3. **`rag::format_context_block()` — stable formatting.** Fixed headers
   (`SUPPORT DOCUMENTS ====`), fixed `[n] (source: …, score: …)` layout, no
   timestamps or nondeterminism.
4. **Startup prefix warmup (`main.rs::warm_prefix_cache`).** When
   `WARM_PREFIX_CACHE=true`, the server fires a 1-token completion whose prompt is
   exactly the system prefix, retrying for up to ~1 minute, so the KV blocks are
   resident *before the first user speaks*. The first real turn of the day is
   cache-hot too.
5. **Per-turn cache telemetry.** vLLM reports usage via
   `usage.prompt_tokens_details.cached_tokens` (OpenAI-compatible). The server
   extracts it (`prompt_cache::extract_cache_stats`), sends a `ttft` event, and the
   UI shows the **cache-hit ratio** live: you should see the ratio jump towards
   60–90% from the second turn onwards.

### 4.4 How to verify caching is working

```bash
# Watch per-turn metrics in the UI (Cache hit tile), or server logs:
#   turn ... cached_tokens=742 prompt_tokens=980  → 76% cache hit

curl http://localhost:8080/api/health | jq .cache_hit_ratio
```

If `cache_hit_ratio` stays near 0: check that vLLM was started with
`--enable-prefix-caching`, that the model/tokenizer did not change between turns,
and that `SYSTEM_PROMPT` did not change (any edit shifts every block).

---

## 5. Tech stack

| Layer | Technology | Why it was chosen |
|---|---|---|
| Backend | **Rust + Actix-web 4 + actix-ws** | Predictable p99 latencies, tiny memory footprint, fearless concurrency for many simultaneous WS sessions; actix-ws gives us a clean async WS session API on the same HTTP server that serves the frontend. |
| Async runtime | **Tokio** (via actix-web `rt`) | Ecosystem standard; used for the per-turn spawn, streams, timers and channels. |
| HTTP/SSE client | **reqwest** (json, stream, multipart features) | Streams the vLLM SSE byte-stream incrementally and uploads multipart WAV to Groq without temp files. |
| STT | **Groq Whisper** (`whisper-large-v3-turbo`) | Groq LPU inference makes large-v3-quality transcription fast enough for conversation; the turbo variant halves the cost with near-identical WER. |
| Vector store | **Qdrant** | Best-in-class filtered ANN (HNSW), trivial REST API (we ship a minimal Rust client, no heavy SDK), payload filters used for idempotent re-ingestion. |
| LLM engine | **vLLM** (OpenAI-compatible server) | PagedAttention, continuous batching, **Automatic Prefix Caching** and chunked prefill — the foundation of the low-TTFT story. |
| KV-cache tier | **LMCache** (via `LMCacheConnectorV1`) | Spills/reuses KV blocks beyond a single engine process: CPU RAM, disk, or Redis — the cache survives restarts and spans replicas. |
| LLM | **Llama-3.1-8B-Instruct** (default) | Strong instruction following at chat speeds; swap via `VLLM_MODEL` for anything vLLM can serve. |
| TTS | Browser **speechSynthesis** (default) / **Groq PlayAI TTS** | Zero-cost zero-latency voice in the browser; PlayAI voices when you want studio quality, delivered as base64 WAV over the same socket. |
| Frontend | Vanilla JS + **AudioWorklet** | AudioWorklet processes audio off the main thread with no dependencies; WebSocket-first design; no build step — served straight from disk. |
| Infra | **Docker Compose** | One command brings up Qdrant + vLLM(GPU) + the Rust server. |

---

## 6. Repository layout

```
EchoStream-AI/
├── README.md                        ← you are here
├── docker-compose.yml               ← qdrant + vllm(GPU, LMCache) + echostream
├── .env.example                     ← every configurable knob, documented
├── .gitignore
├── docs/
│   └── sample_knowledge/            ← demo knowledge base to ingest first
│       ├── acme_support_kb.md
│       └── returns_and_billing.md
├── server/                          ← the Rust workspace
│   ├── Cargo.toml                   ← two binaries: echostream-server, echostream-ingest
│   ├── Dockerfile                   ← multi-stage build (rust:1.83 → debian-slim)
│   └── src/
│       ├── lib.rs                   ← shared library root
│       ├── main.rs                  ← server entrypoint: routes, warmup, health
│       ├── bin/ingest.rs            ← CLI: chunk → embed → upsert → Qdrant
│       ├── config.rs                ← all env-driven configuration
│       ├── state.rs                 ← AppState: clients + atomic metrics
│       ├── error.rs                 ← EchoError enum + Actix ResponseError
│       ├── audio.rs                 ← PCM16→WAV wrapper, RMS energy (+ unit tests)
│       ├── ws.rs                    ← WS session loop, VAD, per-turn pipeline
│       ├── stt/
│       │   ├── mod.rs
│       │   └── groq_whisper.rs      ← Groq /audio/transcriptions client
│       ├── rag/
│       │   ├── mod.rs
│       │   ├── chunker.rs           ← overlap-aware chunking + stable context block
│       │   ├── embedder.rs          ← OpenAI-compatible or local hash embeddings
│       │   └── qdrant.rs            ← minimal Qdrant REST client
│       ├── llm/
│       │   ├── mod.rs
│       │   ├── prompt_cache.rs      ← cache-stable prompt assembly + hit stats
│       │   └── vllm_client.rs       ← SSE streaming client + cache warmup call
│       └── tts/
│           └── mod.rs               ← Groq PlayAI TTS (optional server-side voice)
└── web/                             ← static frontend served by the Rust binary
    ├── index.html
    ├── styles.css
    ├── app.js                       ← WS client, VAD, streaming UI + TTS
    └── worklets/
        └── recorder-worklet.js      ← AudioWorkletProcessor (mic capture)
```

---

## 7. Getting started

### 7.1 Prerequisites

| Path | Requirement |
|---|---|
| Full stack (compose) | Docker + NVIDIA Container Toolkit, an NVIDIA GPU with >= 16 GB VRAM (8B model at fp16), a Groq API key, an HF token for gated models |
| Dev without GPU | Rust 1.75+, Docker (Qdrant only), a Groq API key, any OpenAI-compatible LLM endpoint (or point `VLLM_BASE_URL` at any provider) |
| Browser | Chrome / Edge / Firefox 2022+ (AudioWorklet + WebSocket required); served over `http://localhost` so the mic works without TLS |

### 7.2 Quickstart with Docker Compose (full stack)

```bash
cp .env.example .env    # a ready-made (git-ignored) .env already ships with the repo; just paste your keys
# Edit .env: set GROQ_API_KEY (and HF_TOKEN if the model is gated)

docker compose up -d qdrant vllm       # vector DB + vLLM (downloads model on first run)
docker compose up -d echostream        # build + start the Rust server

# Ingest the demo knowledge base (runs on your host against the compose Qdrant):
cd server && cargo run --release --bin echostream-ingest -- ../docs/sample_knowledge
```

Then open **http://localhost:8080**, click **Start listening**, and ask:
> "My Wi-Fi drops every hour, what do I do?"

### 7.3 Manual setup (dev-friendly, no GPU required)

```bash
# 1) Qdrant
docker run -d --name qdrant -p 6333:6333 -v qdrant_data:/qdrant/storage qdrant/qdrant:v1.12.4

# 2) An OpenAI-compatible LLM endpoint. Options:
#    a) vLLM on a GPU box:
docker run --gpus all -p 8000:8000 vllm/vllm-openai:v0.6.6 \
  --model meta-llama/Llama-3.1-8B-Instruct --enable-prefix-caching \
  --enable-chunked-prefill --max-model-len 8192
#    b) any other OpenAI-compatible provider (set VLLM_BASE_URL + VLLM_API_KEY + VLLM_MODEL)

# 3) Configure + build + run the Rust server
cp .env.example server/.env
cd server && cargo run --release --bin echostream-server
#    -> serves http://localhost:8080 (frontend + /ws + /api/health)

# 4) Ingest documents (idempotent per document)
cargo run --release --bin echostream-ingest -- ../docs/sample_knowledge
```

### 7.4 Embeddings without any API key

The default `EMBEDDING_PROVIDER=hash` uses a deterministic feature-hashing embedder
implemented in `rag/embedder.rs` (unigrams + bigrams + char 4-grams, L2-normalized).
It is deliberately dependency-free so the entire pipeline runs end-to-end offline.
For production retrieval quality switch to a real embedding model:

```env
EMBEDDING_PROVIDER=openai
EMBEDDING_BASE_URL=https://api.openai.com/v1          # or vLLM / TEI / Nomic
EMBEDDING_MODEL=text-embedding-3-small
EMBEDDING_API_KEY=sk-...
EMBEDDING_DIM=1536
```

> Changing the embedding provider or dimension changes the vector space — wipe the
> Qdrant collection (or use a new `QDRANT_COLLECTION`) and re-ingest.

---

## 8. Configuration reference

All configuration is environment-driven (`server/.env` or the process environment).
Every variable has a sensible default so `cargo run` works out of the box (with the
hash embedder and browser TTS).

### Server

| Variable | Default | Description |
|---|---|---|
| `BIND_ADDR` | `0.0.0.0:8080` | Listen address for HTTP + WS. |
| `WEB_DIR` | `../web` | Static frontend directory served at `/`. |
| `HTTP_TIMEOUT_SECS` | `30` | Timeout for non-streaming outbound HTTP calls. |

### Speech-to-text (Groq)

| Variable | Default | Description |
|---|---|---|
| `GROQ_API_KEY` | *(empty)* | **Required for STT.** Get one at console.groq.com. |
| `GROQ_BASE_URL` | `https://api.groq.com/openai/v1` | OpenAI-compatible base URL. |
| `WHISPER_MODEL` | `whisper-large-v3-turbo` | Any Groq Whisper model id. |

### LLM / vLLM

| Variable | Default | Description |
|---|---|---|
| `VLLM_BASE_URL` | `http://localhost:8000/v1` | OpenAI-compatible base URL of vLLM. |
| `VLLM_MODEL` | `meta-llama/Llama-3.1-8B-Instruct` | Model name as served. |
| `VLLM_API_KEY` | *(empty)* | Bearer token if your endpoint requires one. |
| `TEMPERATURE` | `0.3` | Sampling temperature. |
| `MAX_TOKENS` | `512` | Generation cap per turn. |
| `SYSTEM_PROMPT` | support-agent persona | The cache-critical first block — keep it **stable** across deployments. |
| `WARM_PREFIX_CACHE` | `true` | Fire a 1-token warmup completion at startup (retries ~1 min). |

### Retrieval (Qdrant)

| Variable | Default | Description |
|---|---|---|
| `QDRANT_URL` | `http://localhost:6333` | Qdrant REST endpoint. |
| `QDRANT_API_KEY` | *(empty)* | Bearer key for Qdrant Cloud / secured instances. |
| `QDRANT_COLLECTION` | `support_docs` | Collection name (created on first ingest). |
| `TOP_K` | `4` | Chunks retrieved per query. |

### Embeddings

| Variable | Default | Description |
|---|---|---|
| `EMBEDDING_PROVIDER` | `hash` | `hash` (local, offline) or `openai` (any OpenAI-compatible endpoint). |
| `EMBEDDING_BASE_URL` | `https://api.openai.com/v1` | Used when provider is `openai`. |
| `EMBEDDING_MODEL` | `text-embedding-3-small` | Used when provider is `openai`. |
| `EMBEDDING_API_KEY` | *(empty)* | Used when provider is `openai`. |
| `EMBEDDING_DIM` | `512` | Vector size; **must match** the model when using `openai`. |

### Text-to-speech

| Variable | Default | Description |
|---|---|---|
| `TTS_PROVIDER` | `browser` | `browser` = Web Speech API in the client; `groq` = server-side PlayAI TTS. |
| `TTS_MODEL` | `playai-tts` | Groq TTS model (groq mode only). |
| `TTS_VOICE` | `Celeste-PlayAI` | Groq voice (groq mode only). |

### Voice activity

| Variable | Default | Description |
|---|---|---|
| `VAD_RMS_THRESHOLD` | `350` | Server-side RMS threshold (int16 scale) to detect speech/silence. |

---

## 9. WebSocket protocol reference

One connection at `ws(s)://host/ws` carries the whole session. **Binary frames are
audio; text frames are JSON.**

### 9.1 Client -> Server

| Frame | Payload | Purpose |
|---|---|---|
| text | `{"type":"start"}` | Reset the VAD/utterance state; server replies `status: listening`. |
| binary | raw PCM16 LE, mono, 16 kHz | Microphone audio; any size (client sends 4096-sample ≈ 256 ms frames). |
| text | `{"type":"flush"}` | Force-finalize the buffered utterance now (push-to-talk release, Send-now button). |
| text | `{"type":"stop"}` | End the session; server closes the socket. |

### 9.2 Server -> Client

| `type` | Fields | Meaning |
|---|---|---|
| `hello` | `stt_model, llm_model, tts_provider, vad_rms_threshold` | Sent on connect; capabilities snapshot. |
| `status` | `stage` = `listening \| stt \| retrieval \| llm \| tts` | Pipeline stage transitions (drives the stage pill). |
| `transcript` | `text` | Final Whisper transcript for the turn. |
| `sources` | `sources[{source, score, snippet}]` | Retrieved chunks above the score floor. |
| `ttft` | `ms, prompt_tokens, cached_tokens` | Time to first token + KV-cache hit stats for the turn. |
| `token` | `value` | One streamed LLM token fragment. |
| `tts` | `format:"wav", data:<base64>` | Complete answer synthesized server-side (groq TTS mode). |
| `answer_done` | `answer, elapsed_ms, ttft_ms, prompt_tokens, cached_tokens, cache_hit_ratio, sources_used` | Turn summary; drives the metrics tiles. |
| `error` | `message` | Any pipeline failure; the session stays usable. |

### 9.3 Example turn (annotated)

```jsonc
// t=0      client -> server: binary (…512 ms of speech…)
// t=+700ms server VAD hears trailing silence, finalizes the utterance
{"type":"status","stage":"stt"}
// t=+950ms Groq Whisper came back
{"type":"transcript","text":"my wifi drops every hour"}
{"type":"status","stage":"retrieval"}
{"type":"sources","sources":[{"source":"acme_support_kb.md","score":0.61,"snippet":"If Wi-Fi drops every 60 minutes …"}]}
{"type":"status","stage":"llm"}
// t=+1180ms first token decoded (prefill hit the cache)
{"type":"ttft","ms":235,"prompt_tokens":980,"cached_tokens":742}
{"type":"token","value":"Set"} {"type":"token","value":" your"} …
// browser TTS speaks the first completed sentence while later tokens stream
{"type":"answer_done","answer":"Set your channel to manual …","elapsed_ms":1432,"ttft_ms":235,"cache_hit_ratio":0.76}
{"type":"status","stage":"listening"}
```

---

## 10. The voice pipeline, stage by stage

### 10.1 Capture (browser, `worklets/recorder-worklet.js` + `app.js`)

- An `AudioWorkletProcessor` batches 2048 context-rate frames (~42 ms at 48 kHz),
  mono-mixes all channels, and posts them to the main thread — off the UI thread,
  with no GC pressure from the media pipeline.
- The main thread downsamples to 16 kHz via linear interpolation and converts to
  **PCM16** (the exact format Whisper expects server-side; no re-encode anywhere).
- Frames accumulate into 4096-sample (256 ms) WebSocket binary frames.
- **Client VAD**: an RMS gate (float32 threshold 0.012) keeps the socket silent
  while you are silent; frames keep flowing for 250 ms after the last voice to
  avoid clipping word tails. The gate also closes while TTS playback is active so
  the agent does not hear itself.

### 10.2 Session loop (server, `ws.rs`)

A `tokio::select!` loop multiplexes inbound WS frames with a completion channel
from the active turn task:

- Binary frames extend the current utterance buffer and are fed through the
  **server-side RMS VAD** (`VAD_RMS_THRESHOLD`, int16 scale).
- An utterance is finalized when: 3 consecutive silent chunks follow speech
  (~700 ms), the 12 s cap is hit, or the client sends `flush`.
- While a turn is executing, `busy=true` and further audio is dropped (the client
  is playing the answer anyway); the completion message flips back to listening.
- Short dialogue memory (last 4 user/assistant pairs) is kept per session and
  replayed after the cache-stable prefix (see §4.3).

### 10.3 STT (server, `stt/groq_whisper.rs`)

The PCM buffer is wrapped into a **WAV container** (`audio::pcm16_to_wav`, a
44-byte canonical header) and uploaded as multipart to Groq
`/audio/transcriptions` with `model=whisper-large-v3-turbo`, `temperature=0`.
Latency, payload size and transcript length are logged (`pipeline::stt` target).
Empty transcripts are a no-op (the session returns to listening) — no wasted LLM
or retrieval calls.

### 10.4 Retrieval (server, `rag/`)

1. `Embedder::embed()` — one embedding for the utterance.
2. `Qdrant::search()` — POST `/collections/{c}/points/search` with `top_k` and
   `with_payload: true` (cosine, HNSW m=32 / ef_construct=256 at collection
   creation).
3. `stable_hits()` filters below `MIN_RETRIEVAL_SCORE = 0.2` and orders
   deterministically (see §4.3).
4. `format_context_block()` renders the `SUPPORT DOCUMENTS` block with numbered,
   cited chunks.

### 10.5 Cache-aware prompt + streaming LLM (`llm/`)

`PromptBuilder::build_messages()` produces:

```
[0] system    : SYSTEM_PROMPT (constant; first KV blocks, always cache-hit)
[1..n]        : prior dialogue turns (short, per-session)
[n+1] user    : STATIC CONTEXT? + SUPPORT DOCUMENTS? + USER QUESTION
```

`VllmClient::stream_chat()` posts `stream:true, stream_options:{include_usage:true}`
and parses the SSE byte-stream incrementally with `bytes_stream()` + a string
buffer — every token event is forwarded to the browser **immediately** (no
buffering of the full response). The first decoded token produces the `ttft`
event with cache stats; the final `usage` event updates them.

### 10.6 TTS playback

- **Browser mode (default)**: `app.js` accumulates tokens and speaks each
  **completed sentence** (`[.!?…]` boundary) via `speechSynthesis`, so playback
  starts while generation continues. Utterances queue naturally through the
  Web Speech API.
- **Groq mode** (`TTS_PROVIDER=groq`): after the answer completes,
  `tts::TtsClient::synthesize()` renders the full answer through
  `/audio/speech` (`playai-tts`, voice `Celeste-PlayAI`) and the WAV is sent as a
  base64 `tts` event, decoded and played by the client.

### 10.7 Echo control

`echoCancellation: true` + muting the mic gate during playback (`state.playing`)
+ server-side `busy` drop window together prevent the classic feedback loop where
the agent hears its own voice and answers itself.

---

## 11. Metrics and observability

### In the UI

Five live tiles: **Last TTFT**, **Cache hit** (`cached/prompt tokens`), **Turn
total** (STT start → answer done), **Sources used**, **Avg TTFT** (session mean).
Plus a pipeline stage pill (`listening → transcribing → retrieving → streaming →
speaking`) and a WS status pill.

### On the server

`GET /api/health` returns aggregate counters:

```json
{
  "status": "ok",
  "uptime_secs": 342,
  "active_sessions": 2,
  "turns_total": 27,
  "prompt_tokens_total": 26460,
  "cached_tokens_total": 19840,
  "cache_hit_ratio": 0.75,
  "stt_model": "whisper-large-v3-turbo",
  "llm_model": "meta-llama/Llama-3.1-8B-Instruct",
  "qdrant_collection": "support_docs",
  "embedding_provider": "hash",
  "tts_provider": "browser"
}
```

Structured `tracing` logs cover every stage: STT round-trip timing
(`pipeline::stt`), turn failures, warmup progress, Qdrant reachability. Set
`RUST_LOG=debug` for verbose session tracing.

---

## 12. Performance tuning playbook

| Knob | Where | Effect |
|---|---|---|
| `--enable-prefix-caching` | vLLM flags | Non-negotiable; enables KV block reuse. |
| `--enable-chunked-prefill` | vLLM flags | Lets long RAG prefills overlap with other requests' decode; big p99 win under load. |
| `--gpu-memory-utilization 0.90` | vLLM flags | More room for KV blocks → longer cache residency. |
| `--max-model-len 8192` | vLLM flags | Keep tight; smaller context = more KV blocks available for the prefix cache. |
| LMCache kv-transfer config | compose env | Enables the CPU-RAM/disk KV tier; survives restarts, shares across replicas. |
| `WHISPER_MODEL` | env | `whisper-large-v3-turbo` ≈ large-v3 WER at ~2× the speed; `whisper-small` if you trade accuracy for latency. |
| `SILENCE_MS_TO_FLUSH` (client), `SILENCE_CHUNKS_TO_FLUSH` (server) | code constants | Trailing-silence budget; lower = snappier but riskier mid-sentence cuts. |
| `TOP_K` + chunk size (`--chunk-size` at ingest) | env / CLI | Fewer, bigger chunks → shorter prompt → faster prefill and more stable retrieval ordering. |
| `MAX_TOKENS` | env | Short answers = less decode time; the system prompt already asks for 1–3 spoken sentences. |
| `EMBEDDING_PROVIDER=openai` | env | Hash embeddings are for dev only; a real embedding model sharply improves retrieval precision. |
| `.workers(2)` in `main.rs` | code | Actix workers; each session is one async task — 2 workers comfortably handle dozens of sessions. |

---

## 13. Testing

### Rust unit tests

```bash
cd server
cargo test            # WAV container + RMS math tests (audio.rs)
cargo check --all-targets
```

### Frontend syntax check

```bash
node --check web/app.js
node --check web/worklets/recorder-worklet.js
```

### End-to-end manual test script

1. `cargo run --release --bin echostream-ingest -- ../docs/sample_knowledge`
2. `cargo run --release --bin echostream-server`
3. Open http://localhost:8080 — expect `WS connected`, `hello` models pill.
4. Click **Start listening**, allow mic, say: *"My Wi-Fi drops every hour"*.
5. Expected: transcript appears → sources list shows `acme_support_kb.md` →
   answer streams with the Cache-hit tile > 0 from the second turn on →
   browser speaks the answer sentence-by-sentence.
6. Turn the mic off, click **Send now** after recording without speaking —
   empty transcript must be ignored gracefully.
7. Stop vLLM, ask a question → the `error` event appears in the conversation
   and the session returns to listening (no crash).

---

## 14. Security considerations

- **Secrets** live only in `.env` (git-ignored). `GROQ_API_KEY` and embedding keys
  never reach the browser — the browser talks only to the Rust server.
- **Perimeter**: in production, terminate TLS at a reverse proxy (nginx/Caddy) —
  the UI auto-selects `wss://` when the page is served over HTTPS. Mic access
  requires a secure context (`localhost` is exempt).
- **Rate limiting / auth** are intentionally out of scope for this reference
  implementation; add at the proxy (per-IP WS limits) and/or an auth token check
  inside `ws_index` before `actix_ws::handle`.
- **Prompt injection**: retrieved documents are untrusted text. The default
  system prompt constrains the model to the documents; for hostile corpora add
  delimiters/injection-hardening and consider a rewriter stage.
- **Qdrant / vLLM** should not be exposed publicly; keep them on the compose
  internal network (the compose file only publishes 8080, 6333, 8000 — tighten
  as needed).
- **Audio data** is transmitted raw PCM to your server and to Groq — disclose
  that in your privacy policy for real deployments.

---

## 15. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `WS disconnected` pill | server not running / wrong port | `cargo run --release --bin echostream-server`; check `BIND_ADDR`. |
| Mic button errors | permission denied or non-secure context | Allow mic; serve via `localhost` or HTTPS. |
| `error: Groq STT returned 401` | missing/invalid `GROQ_API_KEY` | Set the key in `.env`, restart. |
| `vLLM returned 404` | wrong `VLLM_BASE_URL` (must end in `/v1`) or model name | Match the URL/model to the `vllm serve` invocation. |
| `Qdrant returned 404` on search | collection missing — you skipped ingest | Run `echostream-ingest` against your docs. |
| Cache-hit stays 0% | vLLM started without `--enable-prefix-caching`, or `SYSTEM_PROMPT`/model changed between turns | Enable the flag; keep the system prompt immutable per deployment. |
| TTFT high (>1 s) but cache hits | cold start after restart | Keep `WARM_PREFIX_CACHE=true`; LMCache retains blocks across restarts when its cache dir persists. |
| Agent hears itself | echo cancellation off / gate disabled | Keep `echoCancellation: true` and the mic gate enabled; use headphones. |
| Answers cut mid-sentence | aggressive VAD thresholds | Raise `SILENCE_MS_TO_FLUSH` / `SILENCE_CHUNKS_TO_FLUSH`. |
| Words clipped at start | gate closes between words | The client keeps a 250 ms post-voice tail; raise it in `app.js` if needed. |
| Garbled audio / no transcript | sample-rate mismatch | Client must send 16 kHz mono PCM16; the worklet downsamples automatically — do not change `TARGET_RATE` alone. |

---

## 16. Limitations and roadmap

**Known limitations**

- Client VAD is a simple RMS gate — no barge-in while server TTS is playing
  (browser TTS mode queues utterances instead of canceling them).
- Whisper is called per finalized utterance (no partial/streaming STT), so
  on-screen transcription is per-turn rather than live word-by-word.
- Dialogue memory is in-process only; a server restart clears sessions
  (documents persist in Qdrant).
- The hash embedder is a development convenience, not production retrieval.
- Single-node session state; no horizontal session routing yet (LMCache
  offloads the KV tier, but WS sessions are sticky to one server).

**Roadmap ideas**

- Streaming STT (Groq real-time / local faster-whisper) for word-level latency.
- Semantic VAD (Silero / webrtcvad) instead of energy gating.
- Token-level TTS streaming via Groq PlayAI chunks or Kokoro-on-device.
- Barge-in with echo-aware cancellation of in-flight `speechSynthesis`.
- Hybrid retrieval (BM25 + vectors) and reranking for the knowledge base.
- Multi-tenant collections + per-tenant `cache_salt` to partition KV caches.
- Prometheus metrics endpoint; OpenTelemetry spans across the pipeline.
- Session persistence (Redis) and sticky-free horizontal scaling.

---

## 17. License and credits

Released under the **MIT License** (add your LICENSE file when publishing).

Built on the shoulders of giants:

- [Actix Web](https://actix.rs) and [actix-ws](https://docs.rs/actix-ws) — the async web backbone.
- [Groq](https://groq.com) — Whisper STT and PlayAI TTS at LPU speed.
- [vLLM](https://docs.vllm.ai) — PagedAttention, prefix caching, chunked prefill.
- [LMCache](https://lmcache.ai) — the KV-cache tier beyond one engine.
- [Qdrant](https://qdrant.tech) — fast, filtered vector search.
- [Meta Llama](https://llama.meta.com) — the default assistant model.

> *EchoStream-AI — because support should answer as fast as you can ask.*
