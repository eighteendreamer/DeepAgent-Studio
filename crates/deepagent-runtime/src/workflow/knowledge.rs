//! Knowledge retrieval abstraction for workflow nodes.
//!
//! The trait lives in `deepagent-runtime` so the workflow agent can search
//! knowledge bases without depending on `deepagent-app-core`. The concrete
//! implementation wraps `KnowledgeService` on the app-core side.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::Result;

/// A single document returned by a knowledge retrieval query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub id: String,
    pub title: String,
    pub content: String,
    pub score: f32,
}

/// Pluggable knowledge retrieval backend for workflow nodes.
#[async_trait]
pub trait KnowledgeRetriever: Send + Sync {
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<KnowledgeDocument>>;
}
