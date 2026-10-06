//! The vocabulary of `EventPayload::ContextCompacted::strategy`.
//!
//! One list, because the label is re-read by consumers: the desktop maps it to
//! Chinese (`components/chat-timeline/SessionMarkerRow.tsx`) and its build gate
//! asserts that map covers this list. A strategy invented at a call site would
//! otherwise reach the user as a raw token, and nothing would fail to say so.

/// Compaction reused the session note as the summary; no model call was made.
pub const SESSION_MEMORY: &str = "session_memory";
/// A model produced the summary for the compacted prefix.
pub const MODEL: &str = "model";
/// The user ran `/compact`.
pub const MANUAL: &str = "manual";
/// A boundary written while resuming a session.
pub const RESUME: &str = "resume";
/// The speculative pre-pass's summary was adopted by the second pass.
pub const PREFIRE_PASS2: &str = "prefire_pass2";
/// Compaction ran before the request because the estimate crossed the threshold.
pub const PROACTIVE_THRESHOLD: &str = "proactive_threshold";
/// The overflow path asked the model for a summary mid-run.
pub const REACTIVE_MODEL: &str = "reactive_model";
/// Historical tool results were cleared in place; no summary was written.
pub const MICRO_COMPACT: &str = "micro_compact";
/// Internal bookkeeping for a history snip. Deliberately not surfaced as a
/// timeline row, so it is the one value the UI label map need not cover.
pub const HISTORY_SNIP: &str = "history_snip";

/// Every value a producer may write.
pub const ALL: &[&str] = &[
    SESSION_MEMORY,
    MODEL,
    MANUAL,
    RESUME,
    PREFIRE_PASS2,
    PROACTIVE_THRESHOLD,
    REACTIVE_MODEL,
    MICRO_COMPACT,
    HISTORY_SNIP,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The label doubles as the wire value, and the desktop labels exactly these
    /// strings. A typo (`session-memory`) would split the vocabulary in two with
    /// nothing failing to say so, so the expected spellings are written out here
    /// independently of the constants above.
    #[test]
    fn the_wire_labels_are_exactly_what_the_ui_expects() {
        assert_eq!(
            ALL,
            &[
                "session_memory",
                "model",
                "manual",
                "resume",
                "prefire_pass2",
                "proactive_threshold",
                "reactive_model",
                "micro_compact",
                "history_snip",
            ]
        );
    }

    #[test]
    fn the_vocabulary_has_no_duplicates() {
        let mut sorted = ALL.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(sorted.len(), before, "duplicate strategy label in ALL");
    }
}
