//! Cross-session memory persistence (开发计划.md Phase 5: "长期记忆 跨会话保持").
//!
//! Persists [`MemoryItem`]s (plus their embeddings) to the
//! [`deepagent_persistence`] document store under the `"memory"` collection, and
//! rehydrates them into a [`SemanticMemoryStore`] on startup — so memory
//! survives across sessions and process restarts.

use deepagent_core::clock::Timestamp;
use deepagent_core::error::Result;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;

use crate::embedding::Embedder;
use crate::hybrid::HybridRetriever;
use crate::semantic::SemanticMemoryStore;
use crate::{MemoryId, MemoryItem};

/// The document-store collection memory is persisted under.
pub const MEMORY_COLLECTION: &str = "memory";

/// Persists and loads memory items via the document store.
pub struct MemoryRepository<'db> {
    store: DocumentStore<'db>,
}

impl<'db> MemoryRepository<'db> {
    /// Wrap a database handle.
    pub fn new(db: &'db Database) -> Self {
        Self {
            store: DocumentStore::new(db),
        }
    }

    /// Persist a memory item with its embedding.
    pub fn save(&self, item: &MemoryItem, embedding: &[f32], now: Timestamp) -> Result<()> {
        let body = serde_json::to_string(item)?;
        self.store.put(
            MEMORY_COLLECTION,
            &item.id.to_string(),
            &body,
            Some(embedding),
            now,
        )
    }

    /// Delete a persisted memory item.
    pub fn delete(&self, id: MemoryId) -> Result<bool> {
        self.store.delete(MEMORY_COLLECTION, &id.to_string())
    }

    /// Number of persisted memories.
    pub fn count(&self) -> Result<u64> {
        self.store.count(MEMORY_COLLECTION)
    }

    /// Load all persisted items as `(item, embedding)` pairs.
    pub fn load_all(&self) -> Result<Vec<(MemoryItem, Option<Vec<f32>>)>> {
        let docs = self.store.list(MEMORY_COLLECTION)?;
        let mut out = Vec::with_capacity(docs.len());
        for doc in docs {
            let item: MemoryItem = serde_json::from_str(&doc.body)?;
            out.push((item, doc.embedding));
        }
        Ok(out)
    }

    /// Rehydrate a [`SemanticMemoryStore`] from persisted memory. Items with a
    /// stored embedding reuse it; any missing embedding is recomputed via
    /// `embedder`.
    pub fn hydrate<E: Embedder>(&self, embedder: E) -> Result<SemanticMemoryStore<E>> {
        let mut store = SemanticMemoryStore::new(embedder);
        for (item, embedding) in self.load_all()? {
            match embedding {
                Some(emb) => {
                    store.insert_with_embedding(item, emb);
                }
                None => {
                    store.insert(item);
                }
            }
        }
        Ok(store)
    }

    /// Rehydrate a [`HybridRetriever`] (embedding + BM25 + rerank) from persisted
    /// memory. Stored embeddings are reused; the BM25 index is rebuilt from each
    /// item's markdown content. This is the retriever the Context Pipeline's L5
    /// layer queries.
    pub fn hydrate_hybrid<E: Embedder>(
        &self,
        embedder: E,
    ) -> Result<HybridRetriever<E, crate::hybrid::SignalReranker>> {
        let mut retriever = HybridRetriever::new(embedder);
        for (item, embedding) in self.load_all()? {
            match embedding {
                Some(emb) => {
                    retriever.insert_with_embedding(item, emb);
                }
                None => {
                    retriever.insert(item);
                }
            }
        }
        Ok(retriever)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashingEmbedder;
    use crate::MemoryTier;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn save_and_load_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let repo = MemoryRepository::new(&db);
        let embedder = HashingEmbedder::default();

        let item = MemoryItem::new(MemoryTier::Procedural, "user prefers Rust", 0.9, at(0));
        let emb = embedder.embed(&item.content);
        repo.save(&item, &emb, at(0)).unwrap();

        assert_eq!(repo.count().unwrap(), 1);
        let loaded = repo.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].0.content, "user prefers Rust");
        assert!(loaded[0].1.is_some());
    }

    #[test]
    fn hydrate_rebuilds_searchable_store() {
        let db = Database::open_in_memory().unwrap();
        let repo = MemoryRepository::new(&db);
        let embedder = HashingEmbedder::default();

        for content in [
            "the payment service retries on timeout",
            "the dashboard shows charts",
        ] {
            let item = MemoryItem::new(MemoryTier::Semantic, content, 0.5, at(0));
            let emb = embedder.embed(content);
            repo.save(&item, &emb, at(0)).unwrap();
        }

        // Rehydrate into a fresh semantic store and query it.
        let mut store = repo.hydrate(HashingEmbedder::default()).unwrap();
        assert_eq!(store.len(), 2);
        let hits = store.retrieve("payment timeout", None, 1, at(1000));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.content.contains("payment"));
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mem.db");

        // First session: save a memory.
        {
            let db = Database::open(&path).unwrap();
            let repo = MemoryRepository::new(&db);
            let embedder = HashingEmbedder::default();
            let item = MemoryItem::new(MemoryTier::Episodic, "completed phase 5", 0.7, at(0));
            let emb = embedder.embed(&item.content);
            repo.save(&item, &emb, at(0)).unwrap();
        }

        // Second session: a fresh DB handle still sees it.
        {
            let db = Database::open(&path).unwrap();
            let repo = MemoryRepository::new(&db);
            assert_eq!(repo.count().unwrap(), 1);
            let store = repo.hydrate(HashingEmbedder::default()).unwrap();
            assert_eq!(store.len(), 1);
        }
    }

    #[test]
    fn delete_removes_item() {
        let db = Database::open_in_memory().unwrap();
        let repo = MemoryRepository::new(&db);
        let embedder = HashingEmbedder::default();
        let item = MemoryItem::new(MemoryTier::Semantic, "temp", 0.1, at(0));
        let id = item.id;
        repo.save(&item, &embedder.embed("temp"), at(0)).unwrap();
        assert!(repo.delete(id).unwrap());
        assert_eq!(repo.count().unwrap(), 0);
    }

    #[test]
    fn hydrate_hybrid_rebuilds_retriever() {
        use crate::observation::{Observation, ObservationType};

        let db = Database::open_in_memory().unwrap();
        let repo = MemoryRepository::new(&db);
        let embedder = HashingEmbedder::default();

        // Persist markdown observations (md + embedding).
        for (title, narrative, concepts) in [
            (
                "Payment timeout fix",
                "payment service retries on timeout",
                vec!["payment"],
            ),
            ("Dashboard charts", "render charts on dashboard", vec!["ui"]),
        ] {
            let item = Observation::new(ObservationType::BugFix, title)
                .narrative(narrative)
                .concepts(concepts.into_iter().map(|s| s.to_string()))
                .into_memory_item(at(0));
            let emb = embedder.embed(&item.content);
            repo.save(&item, &emb, at(0)).unwrap();
        }

        // Rehydrate a hybrid retriever (embedding + BM25 + rerank) and query.
        let mut retriever = repo.hydrate_hybrid(HashingEmbedder::default()).unwrap();
        assert_eq!(retriever.len(), 2);
        let hits = retriever.retrieve("payment timeout handling", None, 1, at(1000));
        assert_eq!(hits.len(), 1);
        assert!(hits[0].item.content.to_lowercase().contains("payment"));

        // The L5 context block renders the retrieved markdown.
        let block = crate::hybrid::to_l5_block(&hits);
        assert!(block.contains("Relevant memory"));
        assert!(block.contains("Payment timeout fix"));
    }
}
