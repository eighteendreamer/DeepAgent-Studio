//! Token counting abstraction.
//!
//! The budget system needs to estimate token counts for prompt fragments. We
//! abstract this behind [`TokenCounter`] so a precise model-specific tokenizer
//! (e.g. a BPE implementation) can replace the heuristic without touching the
//! budgeting logic.

/// Anything that can estimate the token cost of a piece of text.
pub trait TokenCounter: Send + Sync {
    /// Estimated number of tokens in `text`.
    fn count(&self, text: &str) -> usize;
}

/// A fast, dependency-free heuristic tokenizer.
///
/// Uses a blend of character- and word-based estimation that approximates
/// typical BPE behaviour for English + code (~4 chars/token). This is good
/// enough for budgeting decisions; it is intentionally conservative (rounds
/// up) so we never *under*-estimate and blow the context window.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicTokenizer;

impl HeuristicTokenizer {
    /// Construct the heuristic tokenizer.
    pub const fn new() -> Self {
        Self
    }
}

impl TokenCounter for HeuristicTokenizer {
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        // Approximate by characters/4, but never fewer than the word count,
        // and at least 1 for non-empty input.
        let chars = text.chars().count();
        let by_chars = chars.div_ceil(4);
        let by_words = text.split_whitespace().count();
        by_chars.max(by_words).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(HeuristicTokenizer::new().count(""), 0);
    }

    #[test]
    fn nonempty_is_at_least_one() {
        assert_eq!(HeuristicTokenizer::new().count("a"), 1);
    }

    #[test]
    fn scales_with_length() {
        let t = HeuristicTokenizer::new();
        let short = t.count("hello world");
        let long = t.count(&"hello world ".repeat(100));
        assert!(long > short);
    }

    #[test]
    fn word_count_floor() {
        let t = HeuristicTokenizer::new();
        // Many short words: word count dominates the char/4 estimate.
        let text = "a a a a a a a a";
        assert!(t.count(text) >= 8);
    }
}
