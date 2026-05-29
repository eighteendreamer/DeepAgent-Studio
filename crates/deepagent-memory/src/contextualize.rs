//! Contextualize Chunk — the defining stage of Anthropic's *Contextual
//! Retrieval* technique.
//!
//! Plain chunks lose the context of the document they came from, which hurts
//! both embedding and BM25 recall (a chunk that says "the timeout was raised to
//! 30s" is useless if you don't know *what* timeout). Anthropic's fix: before
//! indexing, prepend each chunk with a short, situating context blurb generated
//! from the *whole* document. Per their report this cut failed retrievals by
//! ~35% (49% when combined with reranking).
//!
//! The generation step is abstracted behind [`Contextualizer`] so a real model
//! (the production path — Anthropic uses Claude with prompt caching) can replace
//! the deterministic [`HeadingContextualizer`], which situates a chunk using its
//! document title and markdown heading path. Either way the output is a
//! [`ContextualizedChunk`] whose `contextualized_text` (context prefix + chunk)
//! is what gets embedded and BM25-indexed.

use crate::chunking::Chunk;

/// A chunk augmented with a situating context prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextualizedChunk {
    /// The originating chunk.
    pub chunk: Chunk,
    /// The generated situating context (1–2 sentences).
    pub context: String,
}

impl ContextualizedChunk {
    /// The text that should be embedded / BM25-indexed: the context prefix
    /// followed by the original chunk text. This is the Anthropic recipe —
    /// index the *contextualized* chunk, not the bare chunk.
    pub fn contextualized_text(&self) -> String {
        if self.context.trim().is_empty() {
            self.chunk.text.clone()
        } else {
            format!("{}\n\n{}", self.context.trim(), self.chunk.text)
        }
    }
}

/// Generates situating context for a chunk given its source document.
///
/// Production implementations call an LLM with the full document + the chunk
/// (Anthropic's prompt: "give a short context to situate this chunk within the
/// overall document for search retrieval"). This trait keeps that swappable.
pub trait Contextualizer {
    /// Produce a 1–2 sentence context blurb situating `chunk` within `document`
    /// (whose human title is `doc_title`).
    fn contextualize(&self, doc_title: &str, document: &str, chunk: &Chunk) -> String;
}

/// A deterministic, model-free contextualizer.
///
/// It situates a chunk using the document title and the chunk's markdown
/// heading path — e.g. "From 'Setup Guide' > Database > Migrations:". This is a
/// faithful, dependency-free stand-in that still measurably improves retrieval
/// (the context prefix injects document-level vocabulary into the chunk's
/// embedding/BM25 representation). A model-backed contextualizer implements the
/// same trait for richer, semantic context.
#[derive(Debug, Clone, Default)]
pub struct HeadingContextualizer;

impl Contextualizer for HeadingContextualizer {
    fn contextualize(&self, doc_title: &str, _document: &str, chunk: &Chunk) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !doc_title.trim().is_empty() {
            parts.push(doc_title.trim().to_string());
        }
        parts.extend(chunk.heading_path.iter().cloned());
        if parts.is_empty() {
            return String::new();
        }
        format!("Context: this excerpt is from \"{}\".", parts.join(" > "))
    }
}

/// Apply a contextualizer to every chunk of a document, returning the
/// contextualized chunks ready for indexing.
pub fn contextualize_chunks<C: Contextualizer>(
    contextualizer: &C,
    doc_title: &str,
    document: &str,
    chunks: Vec<Chunk>,
) -> Vec<ContextualizedChunk> {
    chunks
        .into_iter()
        .map(|chunk| {
            let context = contextualizer.contextualize(doc_title, document, &chunk);
            ContextualizedChunk { chunk, context }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunking::Chunk;

    fn chunk(text: &str, heading: &[&str]) -> Chunk {
        Chunk {
            index: 0,
            heading_path: heading.iter().map(|s| s.to_string()).collect(),
            text: text.into(),
        }
    }

    #[test]
    fn prepends_document_and_heading_context() {
        let c = chunk("the timeout was raised to 30s", &["Networking", "Timeouts"]);
        let ctx = HeadingContextualizer.contextualize("Service Config", "full doc", &c);
        assert!(ctx.contains("Service Config"));
        assert!(ctx.contains("Networking > Timeouts"));
    }

    #[test]
    fn contextualized_text_combines_prefix_and_chunk() {
        let c = chunk("raw chunk body", &["Section"]);
        let cc = ContextualizedChunk {
            chunk: c,
            context: "Context: from doc.".into(),
        };
        let text = cc.contextualized_text();
        assert!(text.starts_with("Context: from doc."));
        assert!(text.contains("raw chunk body"));
    }

    #[test]
    fn empty_context_falls_back_to_chunk_text() {
        let c = chunk("body only", &[]);
        let cc = ContextualizedChunk {
            chunk: c,
            context: String::new(),
        };
        assert_eq!(cc.contextualized_text(), "body only");
    }

    #[test]
    fn contextualizes_all_chunks() {
        let chunks = vec![chunk("a", &["S1"]), chunk("b", &["S2"])];
        let out = contextualize_chunks(&HeadingContextualizer, "Doc", "full", chunks);
        assert_eq!(out.len(), 2);
        assert!(out[0].context.contains("S1"));
        assert!(out[1].context.contains("S2"));
    }
}
