//! Minimal Qdrant REST client (no heavy SDK dependency).
//!
//! Implements only what the pipeline needs: create collection, upsert
//! points with payloads, vector similarity search, and delete-by-document.

use crate::config::Config;
use crate::error::{EchoError, EchoResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointPayload {
    pub doc_id: String,
    pub source: String,
    pub chunk_index: usize,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub score: f32,
    pub payload: PointPayload,
}

#[derive(Clone)]
pub struct Qdrant {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    collection: String,
    pub vector_size: usize,
}

impl Qdrant {
    pub fn new(cfg: &Config, vector_size: usize) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: cfg.qdrant_url.trim_end_matches('/').to_string(),
            api_key: cfg.qdrant_api_key.clone(),
            collection: cfg.qdrant_collection.clone(),
            vector_size,
        }
    }

    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.api_key.is_empty() {
            rb
        } else {
            rb.bearer_auth(&self.api_key)
        }
    }

    async fn check(&self, resp: reqwest::Response) -> EchoResult<Value> {
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| EchoError::VectorStore(format!("bad Qdrant response ({status}): {e}")))?;
        if !status.is_success() {
            return Err(EchoError::VectorStore(format!(
                "Qdrant returned {status}: {body}"
            )));
        }
        Ok(body)
    }

    /// Create the collection with cosine distance if it does not exist.
    pub async fn ensure_collection(&self) -> EchoResult<()> {
        let url = format!("{}/collections/{}", self.base_url, self.collection);
        let exists = self
            .auth(self.http.get(&url))
            .send()
            .await
            .map_err(|e| EchoError::VectorStore(e.to_string()))?
            .status()
            .is_success();

        if exists {
            tracing::info!(collection = %self.collection, "qdrant collection already exists");
            return Ok(());
        }

        let body = json!({
            "vectors": {
                "size": self.vector_size,
                "distance": "Cosine",
            },
            "hnsw_config": { "m": 32, "ef_construct": 256 }
        });
        self.check(
            self.auth(self.http.put(&url)).json(&body).send().await.map_err(|e| EchoError::VectorStore(e.to_string()))?,
        )
        .await?;
        tracing::info!(collection = %self.collection, dim = self.vector_size, "created qdrant collection");
        Ok(())
    }

    /// Upsert a batch of points (id, vector, payload).
    pub async fn upsert(
        &self,
        points: Vec<(String, Vec<f32>, PointPayload)>,
    ) -> EchoResult<usize> {
        if points.is_empty() {
            return Ok(0);
        }
        let url = format!("{}/collections/{}/points", self.base_url, self.collection);
        let body = json!({
            "points": points.iter().map(|(id, vec, p)| json!({
                "id": id,
                "vector": vec,
                "payload": p,
            })).collect::<Vec<_>>(),
        });
        self.check(
            self.auth(self.http.put(&url))
                .json(&body)
                .send()
                .await
                .map_err(|e| EchoError::VectorStore(e.to_string()))?,
        )
        .await?;
        Ok(points.len())
    }

    /// Remove every point that belongs to `doc_id` (payload filter).
    pub async fn delete_by_doc(&self, doc_id: &str) -> EchoResult<()> {
        let url = format!("{}/collections/{}/points/delete", self.base_url, self.collection);
        let body = json!({
            "filter": { "must": [{ "key": "doc_id", "match": { "value": doc_id } }] }
        });
        self.check(
            self.auth(self.http.post(&url))
                .json(&body)
                .send()
                .await
                .map_err(|e| EchoError::VectorStore(e.to_string()))?,
        )
        .await?;
        Ok(())
    }

    /// Cosine similarity search. Returns up to `top_k` hits ordered by score.
    pub async fn search(&self, vector: &[f32], top_k: usize) -> EchoResult<Vec<SearchHit>> {
        let url = format!("{}/collections/{}/points/search", self.base_url, self.collection);
        let body = json!({
            "vector": vector,
            "limit": top_k,
            "with_payload": true,
        });
        let body = self
            .check(
                self.auth(self.http.post(&url))
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| EchoError::VectorStore(e.to_string()))?,
            )
            .await?;
        let hits: Vec<SearchHit> = serde_json::from_value(body["result"].clone())
            .map_err(|e| EchoError::VectorStore(e.to_string()))?;
        Ok(hits)
    }

    pub async fn count(&self) -> EchoResult<u64> {
        let url = format!("{}/collections/{}/points/count", self.base_url, self.collection);
        let body = json!({ "exact": true });
        let body = self
            .check(
                self.auth(self.http.post(&url))
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| EchoError::VectorStore(e.to_string()))?,
            )
            .await?;
        Ok(body["result"]["count"].as_u64().unwrap_or(0))
    }
}
