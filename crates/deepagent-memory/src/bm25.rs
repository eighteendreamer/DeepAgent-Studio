//! BM25 sparse lexical search (the "BM25" in Anthropic's md + embedding + BM25 +
//! rerank stack; claude-mem implements the equivalent via SQLite FTS5 bm25()).
//!
//! This is a self-contained Okapi BM25 index — no SQLite FTS5 dependency, so it
//! works identically on every platform (claude-mem notes FTS5 is unavailable on
//! some Bun/Windows builds). BM25 scores documents by term frequency saturated
//! by document length, which complements dense embeddings: BM25 nails exact
//! keyword / identifier matches that embeddings can blur, while embeddings catch
//! paraphrases BM25 misses. Fusing both (see [`crate::hybrid`]) is the point.

use std::collections::HashMap;

/// Classic Okapi BM25 parameters.
#[derive(Debug, Clone, Copy)]
pub struct Bm25Params {
    /// Term-frequency saturation (typical 1.2–2.0).
    pub k1: f32,
    /// Length-normalization strength (typical 0.75).
    pub b: f32,
}

impl Default for Bm25Params {
    fn default() -> Self {
        Self { k1: 1.5, b: 0.75 }
    }
}

/// A document registered in the index: an opaque id plus its tokenized terms.
#[derive(Debug, Clone)]
struct IndexedDoc<Id> {
    id: Id,
    term_freqs: HashMap<String, u32>,
    len: u32,
}

/// A BM25 hit: the document id and its BM25 score (higher = more relevant).
#[derive(Debug, Clone, PartialEq)]
pub struct Bm25Hit<Id> {
    /// The matched document id.
    pub id: Id,
    /// BM25 relevance score.
    pub score: f32,
}

/// An in-memory BM25 index over documents identified by `Id`.
#[derive(Debug, Clone)]
pub struct Bm25Index<Id: Clone + Eq> {
    params: Bm25Params,
    docs: Vec<IndexedDoc<Id>>,
    /// term -> number of documents containing it (for IDF).
    doc_freq: HashMap<String, u32>,
    total_len: u64,
}

impl<Id: Clone + Eq> Default for Bm25Index<Id> {
    fn default() -> Self {
        Self::new(Bm25Params::default())
    }
}

impl<Id: Clone + Eq> Bm25Index<Id> {
    /// New empty index.
    pub fn new(params: Bm25Params) -> Self {
        Self {
            params,
            docs: Vec::new(),
            doc_freq: HashMap::new(),
            total_len: 0,
        }
    }

    /// Number of indexed documents.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Add a document. `text` is tokenized internally.
    pub fn add(&mut self, id: Id, text: &str) {
        let tokens = tokenize(text);
        let len = tokens.len() as u32;
        let mut term_freqs: HashMap<String, u32> = HashMap::new();
        for t in tokens {
            *term_freqs.entry(t).or_insert(0) += 1;
        }
        // Update document frequency (count each distinct term once per doc).
        for term in term_freqs.keys() {
            *self.doc_freq.entry(term.clone()).or_insert(0) += 1;
        }
        self.total_len += len as u64;
        self.docs.push(IndexedDoc {
            id,
            term_freqs,
            len,
        });
    }

    /// Average document length (in tokens).
    fn avg_doc_len(&self) -> f32 {
        if self.docs.is_empty() {
            0.0
        } else {
            self.total_len as f32 / self.docs.len() as f32
        }
    }

    /// Inverse document frequency for a term (Robertson-Sparck-Jones, the
    /// non-negative variant used by SQLite FTS5's bm25()).
    fn idf(&self, term: &str) -> f32 {
        let n = self.docs.len() as f32;
        let df = *self.doc_freq.get(term).unwrap_or(&0) as f32;
        // ln(1 + (N - df + 0.5) / (df + 0.5)) — always >= 0.
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// Score every document against `query` and return the top `k` by BM25,
    /// descending. Documents with score <= 0 are excluded.
    pub fn search(&self, query: &str, k: usize) -> Vec<Bm25Hit<Id>> {
        if self.docs.is_empty() || k == 0 {
            return Vec::new();
        }
        let query_terms = tokenize(query);
        let avg = self.avg_doc_len();
        let k1 = self.params.k1;
        let b = self.params.b;

        let mut hits: Vec<Bm25Hit<Id>> = self
            .docs
            .iter()
            .filter_map(|doc| {
                let mut score = 0.0_f32;
                for term in &query_terms {
                    let tf = *doc.term_freqs.get(term).unwrap_or(&0) as f32;
                    if tf == 0.0 {
                        continue;
                    }
                    let idf = self.idf(term);
                    let denom = tf + k1 * (1.0 - b + b * (doc.len as f32 / avg.max(1.0)));
                    score += idf * (tf * (k1 + 1.0)) / denom;
                }
                if score > 0.0 {
                    Some(Bm25Hit {
                        id: doc.id.clone(),
                        score,
                    })
                } else {
                    None
                }
            })
            .collect();

        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        hits
    }
}

/// Tokenize text into lowercased alphanumeric terms (shared with embeddings so
/// the two retrieval paths see the same vocabulary).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Bm25Index<u32> {
        let mut idx = Bm25Index::default();
        idx.add(1, "the payment service retries on timeout");
        idx.add(2, "the dashboard renders charts and graphs");
        idx.add(3, "payment timeout configuration and retry budget");
        idx
    }

    #[test]
    fn ranks_keyword_matches_first() {
        let idx = index();
        let hits = idx.search("payment timeout", 10);
        assert!(!hits.is_empty());
        // Docs 1 and 3 mention payment+timeout; doc 2 does not.
        let ids: Vec<u32> = hits.iter().map(|h| h.id).collect();
        assert!(ids.contains(&1));
        assert!(ids.contains(&3));
        assert!(!ids.contains(&2));
    }

    #[test]
    fn rarer_term_scores_higher_via_idf() {
        let mut idx = Bm25Index::default();
        // "the" appears everywhere (low idf); "zebra" is rare (high idf).
        idx.add(1, "the the the the zebra");
        idx.add(2, "the the the the the");
        idx.add(3, "the the the the the");
        let zebra = idx.search("zebra", 10);
        assert_eq!(zebra[0].id, 1);
    }

    #[test]
    fn empty_query_or_index_returns_nothing() {
        let idx = index();
        assert!(idx.search("", 10).is_empty());
        let empty: Bm25Index<u32> = Bm25Index::default();
        assert!(empty.search("payment", 10).is_empty());
    }

    #[test]
    fn top_k_limits() {
        let idx = index();
        let hits = idx.search("payment timeout retry dashboard charts", 1);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn no_match_returns_empty() {
        let idx = index();
        assert!(idx.search("quantum chromodynamics", 10).is_empty());
    }
}
