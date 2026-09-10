//! `echostream-ingest` — chunk + embed + upsert documents into Qdrant.
//!
//! Usage:
//!   echostream-ingest <file-or-directory> [--source NAME]
//!                     [--chunk-size 1200] [--chunk-overlap 150] [--recreate]
//!
//! Reads every `.txt` / `.md` / `.markdown` file (recursively for dirs),
//! splits it into overlapping chunks, embeds them with the configured
//! embedding backend, and upserts them into the configured Qdrant
//! collection. Re-ingesting the same file replaces its previous chunks.

use anyhow::{bail, Context, Result};
use echostream_agent::config::Config;
use echostream_agent::rag::qdrant::PointPayload;
use echostream_agent::rag::{chunk_text, Embedder, Qdrant};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<PathBuf> = None;
    let mut source_override: Option<String> = None;
    let mut chunk_size: usize = 1200;
    let mut chunk_overlap: usize = 150;
    let mut recreate = false;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--source" => source_override = it.next().cloned(),
            "--chunk-size" => {
                chunk_size = it.next().context("--chunk-size needs a value")?.parse()?
            }
            "--chunk-overlap" => {
                chunk_overlap = it.next().context("--chunk-overlap needs a value")?.parse()?
            }
            "--recreate" => recreate = true,
            other => {
                if other.starts_with('-') {
                    bail!("unknown flag: {other}");
                }
                input = Some(PathBuf::from(other));
            }
        }
    }
    let Some(input) = input else {
        bail!("usage: echostream-ingest <file-or-directory> [--source NAME] [--chunk-size N] [--chunk-overlap N] [--recreate]");
    };

    let cfg = Config::from_env()?;
    let embedder = Embedder::new(&cfg);
    let qdrant = Qdrant::new(&cfg, cfg.embedding_dim);
    qdrant.ensure_collection().await?;

    // Collect files.
    let mut files: Vec<PathBuf> = Vec::new();
    if input.is_dir() {
        collect_files(&input, &mut files)?;
    } else {
        files.push(input.clone());
    }
    if files.is_empty() {
        bail!("no .txt/.md files found under {}", input.display());
    }

    let started = std::time::Instant::now();
    let mut total_chunks = 0usize;

    for file in &files {
        let doc_id = file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .to_string();
        let source = source_override.clone().unwrap_or_else(|| {
            file.strip_prefix(&input)
                .or_else(|_| file.strip_prefix(&std::env::current_dir().unwrap_or_default()))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| file.to_string_lossy().to_string())
        });

        let text = std::fs::read_to_string(file)
            .with_context(|| format!("reading {}", file.display()))?;
        let chunks = chunk_text(&text, chunk_size, chunk_overlap);
        if chunks.is_empty() {
            tracing::warn!(file = %file.display(), "no content, skipping");
            continue;
        }

        if recreate {
            qdrant.delete_by_doc(&doc_id).await?;
        }

        let mut points = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let vector = embedder.embed(chunk).await?;
            let payload = PointPayload {
                doc_id: doc_id.clone(),
                source: source.clone(),
                chunk_index: i,
                text: chunk.clone(),
            };
            points.push((Uuid::new_v4().to_string(), vector, payload));
        }

        let n = qdrant.upsert(points).await?;
        total_chunks += n;
        tracing::info!(file = %file.display(), doc_id = %doc_id, chunks = n, "ingested");
    }

    let count = qdrant.count().await?;
    tracing::info!(
        points_in_collection = count,
        total_chunks = total_chunks,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "ingestion complete"
    );
    Ok(())
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if matches!(ext, "txt" | "md" | "markdown") {
                out.push(path);
            }
        }
    }
    Ok(())
}
