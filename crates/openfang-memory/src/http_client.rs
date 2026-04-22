//! HTTP client for the memory-api gateway.
//!
//! Provides an async HTTP client that routes `remember` and `recall` operations
//! to the shared memory-api service. Uses tokio::task::spawn_blocking for
//! synchronous callers.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

/// Error type for memory API operations.
#[derive(Debug, thiserror::Error)]
pub enum MemoryApiError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Missing config: {0}")]
    Config(String),
}

/// HTTP client for the memory-api gateway service.
#[derive(Clone)]
pub struct MemoryApiClient {
    base_url: String,
    token: String,
    client: Arc<reqwest::Client>,
}

// -- Request/Response types matching memory-api endpoints --

#[derive(Serialize)]
struct StoreRequest<'a> {
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
    #[serde(rename = "agentId", skip_serializing_if = "Option::is_none")]
    agent_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    importance: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct StoreResponse {
    pub id: serde_json::Value,
    #[serde(default)]
    pub deduplicated: bool,
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<&'a str>,
}

#[derive(Deserialize, Debug)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub count: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SearchResult {
    pub id: serde_json::Value,
    pub content: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub score: f64,
    #[serde(rename = "createdAt", default)]
    pub created_at: Option<f64>,
}

#[derive(Deserialize, Debug)]
struct HealthResponse {
    pub status: String,
}

#[derive(Serialize)]
struct GraphEntityRequest {
    pub id: String,
    pub entity_type: serde_json::Value,
    pub name: String,
    pub properties: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Serialize)]
struct GraphRelationRequest {
    pub source: String,
    pub relation: serde_json::Value,
    pub target: String,
    pub properties: std::collections::HashMap<String, serde_json::Value>,
    pub confidence: f32,
}

#[derive(Serialize)]
struct GraphQueryRequest {
    pub source: Option<String>,
    pub relation: Option<serde_json::Value>,
    pub target: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct GraphQueryResponse {
    pub results: Vec<serde_json::Value>,
}

impl MemoryApiClient {
    /// Create a new memory-api HTTP client.
    ///
    /// `base_url`: The base URL of the memory-api service (e.g., "http://127.0.0.1:5500").
    /// `token_env`: The name of the environment variable holding the bearer token.
    pub fn new(base_url: &str, token_env: &str) -> Result<Self, MemoryApiError> {
        let token = if token_env.is_empty() {
            String::new()
        } else {
            std::env::var(token_env).unwrap_or_else(|_| {
                warn!(env = token_env, "Memory API token env var not set");
                String::new()
            })
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("openfang-memory/0.5")
            .build()
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;

        let base_url = base_url.trim_end_matches('/').to_string();

        Ok(Self {
            base_url,
            token,
            client: Arc::new(client),
        })
    }

    /// Check if memory-api is reachable (async).
    pub async fn health_check_async(&self) -> Result<(), MemoryApiError> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(MemoryApiError::Api {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }

        let body: HealthResponse = resp
            .json()
            .await
            .map_err(|e| MemoryApiError::Parse(e.to_string()))?;

        if body.status != "ok" {
            return Err(MemoryApiError::Api {
                status: 503,
                message: format!("memory-api status: {}", body.status),
            });
        }

        debug!("memory-api health check passed");
        Ok(())
    }

    /// Check if memory-api is reachable (blocking wrapper).
    pub fn health_check(&self) -> Result<(), MemoryApiError> {
        let client = self.clone();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(client.health_check_async())
        })
        .join()
        .map_err(|_| MemoryApiError::Http("health_check thread panicked".into()))?
    }

    /// Store a memory via POST /memory/store (async).
    ///
    /// The memory-api handles embedding generation (Jina AI) and deduplication.
    pub async fn store_async(
        &self,
        content: &str,
        category: Option<&str>,
        agent_id: Option<&str>,
        source: Option<&str>,
        importance: Option<u8>,
        tags: Option<Vec<String>>,
    ) -> Result<StoreResponse, MemoryApiError> {
        let url = format!("{}/memory/store", self.base_url);

        let body = StoreRequest {
            content,
            category,
            agent_id,
            source,
            importance,
            tags,
        };

        let mut req = self.client.post(&url).json(&body);
        if !self.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.token));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;
        let status = resp.status().as_u16();

        if status != 200 && status != 201 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(MemoryApiError::Api {
                status,
                message: body_text,
            });
        }

        let result: StoreResponse = resp
            .json()
            .await
            .map_err(|e| MemoryApiError::Parse(e.to_string()))?;

        debug!(
            id = %result.id,
            deduplicated = result.deduplicated,
            "Stored memory via HTTP"
        );

        Ok(result)
    }

    /// Store a memory (blocking wrapper).
    pub fn store(
        &self,
        content: &str,
        category: Option<&str>,
        agent_id: Option<&str>,
        source: Option<&str>,
        importance: Option<u8>,
        tags: Option<Vec<String>>,
    ) -> Result<StoreResponse, MemoryApiError> {
        let client = self.clone();
        let content = content.to_string();
        let category = category.map(|s| s.to_string());
        let agent_id = agent_id.map(|s| s.to_string());
        let source = source.map(|s| s.to_string());
        let tags = tags.clone();
        
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(client.store_async(
                    &content,
                    category.as_deref(),
                    agent_id.as_deref(),
                    source.as_deref(),
                    importance,
                    tags,
                ))
        })
        .join()
        .map_err(|_| MemoryApiError::Http("store thread panicked".into()))?
    }

    /// Search memories via POST /memory/search (async).
    ///
    /// The memory-api handles embedding the query (Jina AI) and hybrid vector+BM25 search.
    pub async fn search_async(
        &self,
        query: &str,
        limit: usize,
        category: Option<&str>,
    ) -> Result<Vec<SearchResult>, MemoryApiError> {
        let url = format!("{}/memory/search", self.base_url);

        let body = SearchRequest {
            query,
            limit: Some(limit),
            category,
        };

        let mut req = self.client.post(&url).json(&body);
        if !self.token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.token));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;
        let status = resp.status().as_u16();

        if status != 200 {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(MemoryApiError::Api {
                status,
                message: body_text,
            });
        }

        let result: SearchResponse = resp
            .json()
            .await
            .map_err(|e| MemoryApiError::Parse(e.to_string()))?;

        debug!(count = result.count, "Searched memories via HTTP");

        Ok(result.results)
    }

    /// Search memories (blocking wrapper).
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        category: Option<&str>,
    ) -> Result<Vec<SearchResult>, MemoryApiError> {
        let client = self.clone();
        let query = query.to_string();
        let category = category.map(|s| s.to_string());
        
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(client.search_async(&query, limit, category.as_deref()))
        })
        .join()
        .map_err(|_| MemoryApiError::Http("search thread panicked".into()))?
    }

    // -- Graph Operations --

    pub async fn graph_entity_add_async(
        &self,
        id: String,
        entity_type: serde_json::Value,
        name: String,
        properties: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<String, MemoryApiError> {
        let url = format!("{}/memory/graph/entity", self.base_url);
        let body = GraphEntityRequest { id, entity_type, name, properties };
        
        let resp = self.client.post(&url).json(&body).send().await
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;
        
        if !resp.status().is_success() {
            return Err(MemoryApiError::Api {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        
        let res: serde_json::Value = resp.json().await.map_err(|e| MemoryApiError::Parse(e.to_string()))?;
        Ok(res["id"].as_str().unwrap_or_default().to_string())
    }

    pub fn graph_entity_add(
        &self,
        id: String,
        entity_type: serde_json::Value,
        name: String,
        properties: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<String, MemoryApiError> {
        let client = self.clone();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(client.graph_entity_add_async(id, entity_type, name, properties))
        }).join().map_err(|_| MemoryApiError::Http("thread panic".into()))?
    }

    pub async fn graph_relation_add_async(
        &self,
        source: String,
        relation: serde_json::Value,
        target: String,
        properties: std::collections::HashMap<String, serde_json::Value>,
        confidence: f32,
    ) -> Result<String, MemoryApiError> {
        let url = format!("{}/memory/graph/relation", self.base_url);
        let body = GraphRelationRequest { source, relation, target, properties, confidence };
        
        let resp = self.client.post(&url).json(&body).send().await
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;
        
        if !resp.status().is_success() {
            return Err(MemoryApiError::Api {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        
        let res: serde_json::Value = resp.json().await.map_err(|e| MemoryApiError::Parse(e.to_string()))?;
        Ok(res["id"].as_str().unwrap_or_default().to_string())
    }

    pub fn graph_relation_add(
        &self,
        source: String,
        relation: serde_json::Value,
        target: String,
        properties: std::collections::HashMap<String, serde_json::Value>,
        confidence: f32,
    ) -> Result<String, MemoryApiError> {
        let client = self.clone();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(client.graph_relation_add_async(source, relation, target, properties, confidence))
        }).join().map_err(|_| MemoryApiError::Http("thread panic".into()))?
    }

    pub async fn graph_query_async(
        &self,
        source: Option<String>,
        relation: Option<serde_json::Value>,
        target: Option<String>,
    ) -> Result<Vec<serde_json::Value>, MemoryApiError> {
        let url = format!("{}/memory/graph/query", self.base_url);
        let body = GraphQueryRequest { source, relation, target };
        
        let resp = self.client.post(&url).json(&body).send().await
            .map_err(|e| MemoryApiError::Http(e.to_string()))?;
        
        if !resp.status().is_success() {
            return Err(MemoryApiError::Api {
                status: resp.status().as_u16(),
                message: resp.text().await.unwrap_or_default(),
            });
        }
        
        let res: GraphQueryResponse = resp.json().await.map_err(|e| MemoryApiError::Parse(e.to_string()))?;
        Ok(res.results)
    }

    pub fn graph_query(
        &self,
        source: Option<String>,
        relation: Option<serde_json::Value>,
        target: Option<String>,
    ) -> Result<Vec<serde_json::Value>, MemoryApiError> {
        let client = self.clone();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(client.graph_query_async(source, relation, target))
        }).join().map_err(|_| MemoryApiError::Http("thread panic".into()))?
    }
}
