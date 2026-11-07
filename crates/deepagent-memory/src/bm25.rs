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
        }
    }

    /// Add a document. `text` is tokenized internally.
    pub fn add(&mut self, id: Id, text: &str) {
        let tokens = tokenize(text);
        let len = tokens.len() as u32;
        let mut term_freqs: HashMap<String, u32> = HashMap::new();
        for t in tokens {
            *term_freqs.entry(t).or_insert(0) += 1;
        }
        self.docs.push(IndexedDoc {
            id,
            term_freqs,
            len,
        });
    }

    /// Score every document against `query` and return the top `k` by BM25,
    /// descending. Documents with score <= 0 are excluded.
    #[cfg(test)]
    pub fn search(&self, query: &str, k: usize) -> Vec<Bm25Hit<Id>> {
        self.search_filtered(query, k, |_| true)
    }

    /// Restrict the scoring corpus before computing statistics or truncating
    /// candidates. Ineligible documents cannot affect recall or ranking.
    pub fn search_filtered(
        &self,
        query: &str,
        k: usize,
        eligible: impl Fn(&Id) -> bool,
    ) -> Vec<Bm25Hit<Id>> {
        let docs: Vec<_> = self.docs.iter().filter(|doc| eligible(&doc.id)).collect();
        if docs.is_empty() || k == 0 {
            return Vec::new();
        }
        let query_terms = tokenize(query);
        let avg = docs.iter().map(|doc| doc.len as u64).sum::<u64>() as f32 / docs.len() as f32;
        let idfs: Vec<_> = query_terms
            .iter()
            .map(|term| {
                let df = docs
                    .iter()
                    .filter(|doc| doc.term_freqs.contains_key(term))
                    .count() as f32;
                ((docs.len() as f32 - df + 0.5) / (df + 0.5) + 1.0).ln()
            })
            .collect();
        let k1 = self.params.k1;
        let b = self.params.b;

        let mut hits: Vec<Bm25Hit<Id>> = docs
            .iter()
            .filter_map(|doc| {
                let mut score = 0.0_f32;
                for (term, idf) in query_terms.iter().zip(&idfs) {
                    let tf = *doc.term_freqs.get(term).unwrap_or(&0) as f32;
                    if tf == 0.0 {
                        continue;
                    }
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

    #[test]
    fn h176_filtered_corpus_matches_independent_index_before_top_k() {
        let mut all = Bm25Index::default();
        let mut selected = Bm25Index::default();
        for (id, text) in [
            (1, "needle with several extra words"),
            (2, "needle needle secondary"),
        ] {
            all.add(id, text);
            selected.add(id, text);
        }
        for id in 3..100 {
            all.add(id, "needle");
        }
        assert_eq!(
            all.search_filtered("needle", 1, |id| *id <= 2),
            selected.search("needle", 1)
        );
        assert!(all.search_filtered("needle", 1, |_| false).is_empty());
        assert_eq!(
            all.search_filtered("needle", 200, |_| true),
            all.search("needle", 200)
        );
    }

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
