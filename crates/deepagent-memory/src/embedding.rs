//! Embeddings & vector similarity (开发计划.md Phase 5 §2 "sqlite-vec").
//!
//! Semantic retrieval needs to turn text into vectors and compare them. The
//! [`Embedder`] trait abstracts the embedding model so a real provider (DeepSeek
//! / a local model) can replace the built-in [`HashingEmbedder`] without
//! touching the store. The hashing embedder is deterministic and model-free: it
//! produces a fixed-dimension bag-of-words vector, which is enough for
//! meaningful keyword-overlap-as-cosine retrieval offline and in tests.

/// Produces embedding vectors for text.
pub trait Embedder: Send + Sync {
    /// The dimensionality of vectors this embedder produces.
    fn dimensions(&self) -> usize;

    /// Embed `text` into a vector of length [`Embedder::dimensions`].
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// A deterministic, model-free embedder.
///
/// Hashes each token into one of `dims` buckets and accumulates an L2-normalized
/// term-frequency vector. Texts sharing vocabulary get high cosine similarity;
/// disjoint texts get ~0. This is not semantic in the deep-learning sense, but
/// it is a faithful, dependency-free stand-in that exercises the full vector
/// retrieval path. A model-backed embedder implements the same trait.
#[derive(Debug, Clone, Copy)]
pub struct HashingEmbedder {
    dims: usize,
}

impl Default for HashingEmbedder {
    fn default() -> Self {
        Self::new(256)
    }
}

impl HashingEmbedder {
    /// Build with a given dimensionality.
    pub fn new(dims: usize) -> Self {
        assert!(dims > 0, "embedding dimensions must be > 0");
        Self { dims }
    }
}

impl Embedder for HashingEmbedder {
    fn dimensions(&self) -> usize {
        self.dims
    }

    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0.0_f32; self.dims];
        for token in tokenize(text) {
            let bucket = (fnv1a(&token) as usize) % self.dims;
            v[bucket] += 1.0;
        }
        l2_normalize(&mut v);
        v
    }
}

/// Cosine similarity of two equal-length vectors, in `[-1.0, 1.0]`.
///
/// Returns 0.0 if either vector is zero-length or all-zero, or if the lengths
/// differ (defensive: callers should use a single embedder).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_has_correct_dimensions() {
        let e = HashingEmbedder::new(64);
        assert_eq!(e.embed("hello world").len(), 64);
        assert_eq!(e.dimensions(), 64);
    }

    #[test]
    fn identical_text_has_similarity_one() {
        let e = HashingEmbedder::default();
        let a = e.embed("fix the payment timeout bug");
        let b = e.embed("fix the payment timeout bug");
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn overlapping_text_more_similar_than_disjoint() {
        let e = HashingEmbedder::default();
        let query = e.embed("payment timeout retry");
        let related = e.embed("the payment retry logic handles timeout");
        let unrelated = e.embed("frontend button color styling");
        let s_related = cosine_similarity(&query, &related);
        let s_unrelated = cosine_similarity(&query, &unrelated);
        assert!(
            s_related > s_unrelated,
            "related={s_related} unrelated={s_unrelated}"
        );
    }

    #[test]
    fn disjoint_text_near_zero() {
        let e = HashingEmbedder::default();
        let a = e.embed("alpha beta gamma");
        let b = e.embed("xenon yttrium zirconium");
        assert!(cosine_similarity(&a, &b) < 0.2);
    }

    #[test]
    fn empty_and_mismatched_are_zero() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }
}
