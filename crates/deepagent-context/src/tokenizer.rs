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

    fn source(&self) -> &'static str {
        "custom_text_estimate"
    }
}

/// A fast, dependency-free heuristic tokenizer.
///
/// Uses a blend of character- and word-based estimation that approximates
/// typical BPE behaviour for English + code (~4 chars/token). This is only
/// a cheap fragment-ranking estimate. It can substantially underestimate
/// Unicode and must not be used as a conservative provider pressure count.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeuristicTokenizer;

impl HeuristicTokenizer {
    /// Construct the heuristic tokenizer.
    pub const fn new() -> Self {
        Self
    }
}

impl TokenCounter for HeuristicTokenizer {
    fn source(&self) -> &'static str {
        "heuristic_char4_estimate"
    }

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

/// Mixed-script estimate used by Tianshu's request budgeting (`src/compact/micro.ts`).
pub(crate) struct TianshuTokenEstimate;

impl TokenCounter for TianshuTokenEstimate {
    fn count(&self, text: &str) -> usize {
        let (other, cjk) =
            text.chars()
                .fold((0usize, 0usize), |(other, cjk), ch| match ch as u32 {
                    0x4e00..=0x9fff
                    | 0x3400..=0x4dbf
                    | 0x20000..=0x2a6df
                    | 0x3040..=0x309f
                    | 0x30a0..=0x30ff
                    | 0xac00..=0xd7af => (other, cjk + 1),
                    _ => (other + 1, cjk),
                });
        other.div_ceil(4) + cjk.saturating_mul(5).div_ceil(6)
    }

    fn source(&self) -> &'static str {
        "tianshu_mixed_script_estimate"
    }
}

/// Deliberately cautious fallback when no matching prompt template is known.
/// UTF-8 byte length is not a claim about an arbitrary provider's tokenizer or
/// hidden prompt. It trades early compaction for avoiding character/4's known
/// multilingual undercount; provider usage still calibrates subsequent turns.
#[derive(Debug, Clone, Copy, Default)]
pub struct ByteTokenCounter;

impl TokenCounter for ByteTokenCounter {
    fn count(&self, text: &str) -> usize {
        text.len()
    }

    fn source(&self) -> &'static str {
        "utf8_bytes_estimate:unmatched_template"
    }
}

/// Plain-text counter at the existing TokenCounter seam. Full requests must
/// additionally use the model client's official template/serializer boundary.
#[derive(Debug, Clone, Copy)]
pub struct ModelTokenCounter(Option<deepagent_models::tokenizer::DeepSeekTokenizer>);

impl ModelTokenCounter {
    pub fn for_model(model: &str) -> Self {
        Self(deepagent_models::tokenizer::DeepSeekTokenizer::for_model(
            model,
        ))
    }
}

impl TokenCounter for ModelTokenCounter {
    fn count(&self, text: &str) -> usize {
        match self.0.map(|tokenizer| tokenizer.count_text(text)) {
            Some(Ok(count)) => count,
            Some(Err(_)) => {
                tracing::warn!("offline text encoding failed; using UTF-8 byte estimate");
                ByteTokenCounter.count(text)
            }
            None => ByteTokenCounter.count(text),
        }
    }

    fn source(&self) -> &'static str {
        // Even a matched text counter is not a full prompt measurement. Its
        // infallible trait may use byte fallback on an encoder failure.
        "model_text_estimate:byte_fallback_possible"
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
