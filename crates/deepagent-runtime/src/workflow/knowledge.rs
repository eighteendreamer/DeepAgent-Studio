//! Knowledge retrieval abstraction for workflow nodes.
//!
//! The trait lives in `deepagent-runtime` so the workflow agent can search
//! knowledge bases without depending on `deepagent-app-core`. The concrete
//! implementation wraps `KnowledgeService` on the app-core side.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::{CoreError, Result};
use serde_json::{Map, Value};

/// Local v1 vault selection. Empty/missing legacy fields mean mixed; arbitrary
/// base identifiers and dynamic selectors are not supported or silently widened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeScope {
    Project,
    Global,
    Mixed,
}

impl KnowledgeScope {
    pub fn from_config(config: &Map<String, Value>) -> Result<Self> {
        match config.get("knowledgeBaseId") {
            None => Ok(Self::Mixed),
            Some(Value::String(s)) => match s.as_str() {
                "" | "mixed" => Ok(Self::Mixed), "project" => Ok(Self::Project), "global" => Ok(Self::Global),
                _ => Err(CoreError::invalid("knowledgeBaseId must be project, global or mixed (empty legacy value means mixed)")),
            },
            Some(_) => Err(CoreError::invalid("knowledgeBaseId must be a string")),
        }
    }
}

/// Typed selection passed through the sole workflow retrieval seam.
pub struct KnowledgeQuery<'a> {
    pub query: &'a str,
    pub limit: usize,
    pub scope: KnowledgeScope,
}

/// Results and the logical active-entry SHA-256 revision of the bound snapshot.
pub struct KnowledgeSearch {
    pub revision: String,
    pub documents: Vec<KnowledgeDocument>,
}

/// A single document returned by a knowledge retrieval query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub id: String,
    pub title: String,
    pub content: String,
    pub score: f32,
    pub scope: KnowledgeScope,
    pub updated_at: i64,
}

/// Pluggable knowledge retrieval backend for workflow nodes.
#[async_trait]
pub trait KnowledgeRetriever: Send + Sync {
    async fn search(&self, query: KnowledgeQuery<'_>) -> Result<KnowledgeSearch>;
}
