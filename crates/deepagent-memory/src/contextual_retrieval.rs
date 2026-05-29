//! The complete Anthropic *Contextual Retrieval* pipeline, end to end:
//!
//! ```text
//! Documents
//!   -> Chunking            (chunking::chunk_markdown)
//!   -> Contextualize Chunk (contextualize::Contextualizer)
//!   -> Embedding           (embedding::Embedder, on the contextualized text)
//!   -> BM25 Index          (bm25::Bm25Index, on the contextualized text)
//!   -> Hybrid Retrieval    (RRF fusion of dense + sparse)
//!   -> Rerank              (hybrid::Reranker)
//!   -> LLM                 (RetrievedChunk::to_prompt_block for injection)
//! ```
//!
//! Unlike [`crate::hybrid::HybridRetriever`] (which indexes whole memory items),
//! this retriever indexes at the **chunk** level and indexes the
//! **contextualized** text (context prefix + chunk), which is the distinguishing
//! feature of Anthropic's technique. It is what powers retrieval over larger
//! documents (READMEs, design docs, transcripts) for the Context Pipeline's L5.

use std::collections::HashMap;

use crate::bm25::Bm25Index;
use crate::chunking::{chunk_markdown, ChunkConfig};
use crate::contextualize::{contextualize_chunks, ContextualizedChunk, Contextualizer};
use crate::embedding::{cosine_similarity, Embedder};
use crate::fusion::reciprocal_rank_fusion;
use crate::hybrid::HybridConfig;

/// A globally-unique chunk id within a [`ContextualRetriever`].
pub type ChunkId = usize;

/// An indexed chunk plus the metadata needed to render and rank it.
#[derive(Debug, Clone)]
struct StoredChunk {
    doc_id: String,
    doc_title: String,
    contextualized: ContextualizedChunk,
    embedding: Vec<f32>,
}

/// A retrieved chunk with its final score.
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    /// Which document the chunk came from.
    pub doc_id: String,
    /// The document's human title.
    pub doc_title: String,
    /// The chunk's heading path within the document.
    pub heading_path: Vec<String>,
    /// The original (non-contextualized) chunk text.
    pub text: String,
    /// The generated situating context.
    pub context: String,
    /// Final rerank score.
    pub score: f32,
}

impl RetrievedChunk {
    /// Render this chunk for injection into the LLM prompt (the final "LLM"
    /// stage): cites the source and includes the situating context.
    pub fn to_prompt_block(&self) -> String {
        let loc = if self.heading_path.is_empty() {
            self.doc_title.clone()
        } else {
            format!("{} > {}", self.doc_title, self.heading_path.join(" > "))
        };
        format!("[source: {loc}]\n{}", self.text)
    }
}

/// Reranks retrieved chunks. Production uses a cross-encoder; the default
/// [`ScoreReranker`] simply orders by fused retrieval score.
pub trait ChunkReranker {
    /// Re-order `candidates` (each carrying a fused score in `[0,1]`), best
    /// first.
    fn rerank(&self, query: &str, candidates: Vec<(RetrievedChunk, f32)>) -> Vec<RetrievedChunk>;
}

/// Default reranker: orders by the fused retrieval score. A cross-encoder
/// reranker (query × chunk relevance model) implements this same trait.
#[derive(Debug, Clone, Default)]
pub struct ScoreReranker;

impl ChunkReranker for ScoreReranker {
    fn rerank(&self, _query: &str, candidates: Vec<(RetrievedChunk, f32)>) -> Vec<RetrievedChunk> {
        let mut out: Vec<RetrievedChunk> = candidates
            .into_iter()
            .map(|(mut c, fused)| {
                c.score = fused;
                c
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out
    }
}

/// The end-to-end contextual retriever.
pub struct ContextualRetriever<E: Embedder, C: Contextualizer, R: ChunkReranker> {
    embedder: E,
    contextualizer: C,
    reranker: R,
    chunk_config: ChunkConfig,
    hybrid_config: HybridConfig,
    chunks: HashMap<ChunkId, StoredChunk>,
    bm25: Bm25Index<ChunkId>,
    next_id: ChunkId,
}

impl<E: Embedder, C: Contextualizer> ContextualRetriever<E, C, ScoreReranker> {
    /// Build with the default score reranker.
    pub fn new(embedder: E, contextualizer: C) -> Self {
        Self::with_config(
            embedder,
            contextualizer,
            ScoreReranker,
            ChunkConfig::default(),
            HybridConfig::default(),
        )
    }
}

impl<E: Embedder, C: Contextualizer, R: ChunkReranker> ContextualRetriever<E, C, R> {
    /// Build with a custom reranker and configs.
    pub fn with_config(
        embedder: E,
        contextualizer: C,
        reranker: R,
        chunk_config: ChunkConfig,
        hybrid_config: HybridConfig,
    ) -> Self {
        Self {
            embedder,
            contextualizer,
            reranker,
            chunk_config,
            hybrid_config,
            chunks: HashMap::new(),
            bm25: Bm25Index::default(),
            next_id: 0,
        }
    }

    /// Number of indexed chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Ingest a document through the full pipeline: chunk -> contextualize ->
    /// embed -> index (dense + BM25). Returns the number of chunks indexed.
    pub fn add_document(&mut self, doc_id: &str, doc_title: &str, markdown: &str) -> usize {
        // Stage: Chunking.
        let chunks = chunk_markdown(markdown, &self.chunk_config);
        // Stage: Contextualize Chunk.
        let contextualized =
            contextualize_chunks(&self.contextualizer, doc_title, markdown, chunks);

        let mut added = 0;
        for cc in contextualized {
            let index_text = cc.contextualized_text();
            // Stage: Embedding (on the contextualized text).
            let embedding = self.embedder.embed(&index_text);
            // Stage: BM25 Index (on the contextualized text).
            let id = self.next_id;
            self.next_id += 1;
            self.bm25.add(id, &index_text);
            self.chunks.insert(
                id,
                StoredChunk {
                    doc_id: doc_id.to_string(),
                    doc_title: doc_title.to_string(),
                    contextualized: cc,
                    embedding,
                },
            );
            added += 1;
        }
        added
    }

    /// Retrieve the top-`k` chunks for `query` via hybrid retrieval + rerank.
    pub fn retrieve(&self, query: &str, k: usize) -> Vec<RetrievedChunk> {
        if k == 0 || self.chunks.is_empty() || query.trim().is_empty() {
            return Vec::new();
        }
        let pool = self.hybrid_config.candidate_pool;

        // Stage: Hybrid Retrieval — dense.
        let query_vec = self.embedder.embed(query);
        let mut dense: Vec<(ChunkId, f32)> = self
            .chunks
            .iter()
            .filter_map(|(id, c)| {
                let sim = cosine_similarity(&query_vec, &c.embedding);
                if sim > 1e-4 {
                    Some((*id, sim))
                } else {
                    None
                }
            })
            .collect();
        dense.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        dense.truncate(pool);
        let dense_ids: Vec<ChunkId> = dense.into_iter().map(|(id, _)| id).collect();

        // Stage: Hybrid Retrieval — sparse (BM25).
        let sparse_ids: Vec<ChunkId> = self
            .bm25
            .search(query, pool)
            .into_iter()
            .map(|h| h.id)
            .collect();

        // Stage: Hybrid Retrieval — fuse via RRF.
        let fused =
            reciprocal_rank_fusion(&dense_ids, &sparse_ids, self.hybrid_config.rrf_k, |a, b| {
                a.cmp(b)
            });
        if fused.is_empty() {
            return Vec::new();
        }
        let max_fused = fused.first().map(|(_, s)| *s).unwrap_or(1.0).max(1e-6);

        let candidates: Vec<(RetrievedChunk, f32)> = fused
            .into_iter()
            .filter_map(|(id, s)| {
                self.chunks.get(&id).map(|c| {
                    (
                        RetrievedChunk {
                            doc_id: c.doc_id.clone(),
                            doc_title: c.doc_title.clone(),
                            heading_path: c.contextualized.chunk.heading_path.clone(),
                            text: c.contextualized.chunk.text.clone(),
                            context: c.contextualized.context.clone(),
                            score: 0.0,
                        },
                        s / max_fused,
                    )
                })
            })
            .collect();

        // Stage: Rerank.
        let mut ranked = self.reranker.rerank(query, candidates);
        ranked.truncate(k);
        ranked
    }

    /// Render retrieved chunks as the Context Pipeline's **L5 (Semantic
    /// Retrieval)** block — the final "LLM" stage (prompt injection).
    pub fn retrieve_l5_block(&self, query: &str, k: usize) -> String {
        let hits = self.retrieve(query, k);
        if hits.is_empty() {
            return String::new();
        }
        let mut out = String::from("# Relevant context (retrieved)\n");
        for h in &hits {
            out.push_str(&format!("\n{}\n", h.to_prompt_block()));
        }
        out.trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contextualize::HeadingContextualizer;
    use crate::embedding::HashingEmbedder;

    fn retriever() -> ContextualRetriever<HashingEmbedder, HeadingContextualizer, ScoreReranker> {
        ContextualRetriever::new(HashingEmbedder::default(), HeadingContextualizer)
    }

    const DOC: &str = "# Payment Service\n\n\
        The payment service charges customers via Stripe.\n\n\
        ## Retries\n\n\
        On a network timeout the service retries the charge with exponential backoff. \
        The retry budget is controlled by the RETRY_BUDGET environment variable.\n\n\
        ## Webhooks\n\n\
        Stripe webhooks notify us of asynchronous payment events like disputes.";

    #[test]
    fn ingests_and_chunks_document() {
        let mut r = retriever();
        let n = r.add_document("payments.md", "Payment Service", DOC);
        assert!(n >= 1);
        assert_eq!(r.chunk_count(), n);
    }

    #[test]
    fn retrieves_relevant_chunk_with_context() {
        let mut r = retriever();
        r.add_document("payments.md", "Payment Service", DOC);
        let hits = r.retrieve("network timeout retries backoff", 1);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.to_lowercase().contains("retr"));
        // Contextualization attached document/heading context.
        assert!(hits[0].context.contains("Payment Service"));
    }

    #[test]
    fn bm25_finds_exact_env_var_across_chunks() {
        let mut r = retriever();
        r.add_document("payments.md", "Payment Service", DOC);
        let hits = r.retrieve("RETRY_BUDGET", 1);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].text.contains("RETRY_BUDGET"));
    }

    #[test]
    fn prompt_block_cites_source() {
        let mut r = retriever();
        r.add_document("payments.md", "Payment Service", DOC);
        let block = r.retrieve_l5_block("webhook disputes", 1);
        assert!(block.contains("Relevant context"));
        assert!(block.contains("[source: Payment Service"));
    }

    #[test]
    fn empty_query_and_index_safe() {
        let r = retriever();
        assert!(r.retrieve("anything", 5).is_empty());
        let mut r2 = retriever();
        r2.add_document("d", "D", DOC);
        assert!(r2.retrieve("", 5).is_empty());
    }

    #[test]
    fn multiple_documents_are_searchable() {
        let mut r = retriever();
        r.add_document(
            "a.md",
            "Auth",
            "# Auth\n\nLogin uses JWT tokens signed with RS256.",
        );
        r.add_document("p.md", "Payments", DOC);
        let auth = r.retrieve("JWT token signing", 1);
        assert_eq!(auth[0].doc_id, "a.md");
        let pay = r.retrieve("stripe webhook", 1);
        assert_eq!(pay[0].doc_id, "p.md");
    }
}
