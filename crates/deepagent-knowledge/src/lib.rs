//! # deepagent-knowledge
//!
//! A cumulative, precisely-retrievable **knowledge base** for DeepAgent Studio.
//!
//! Day-to-day usage surfaces reusable knowledge — pitfalls, fixes, frequently
//! used commands, important configs. This crate persists that knowledge as
//! **Obsidian-style Markdown notes** (one `.md` file per entry, human-readable
//! and editable by external tools) and makes it **precisely retrievable** so
//! the same pitfalls are not hit twice.
//!
//! ## How precision is guaranteed: two channels
//!
//! 1. **Passive injection (primary, default-on)** — before each turn the host
//!    retrieves against the user's query and injects entries scoring above a
//!    threshold as a source-cited context block. The model never has to
//!    remember to look; relevant knowledge shows up automatically.
//! 2. **Active query (supplementary)** — `knowledge_search` / `knowledge_write`
//!    tools let the model (or a sub-agent) deliberately look deeper or capture
//!    new knowledge.
//!
//! ## Reuse over reinvention
//!
//! Retrieval is the existing contextual-retrieval stack from `deepagent-memory`
//! ([`deepagent_memory::ContextualRetriever`]: chunk → contextualize → embed →
//! BM25 → RRF → rerank). Frontmatter parsing reuses the dependency-light
//! splitter from `deepagent-skills`. Embeddings default to the offline
//! [`deepagent_memory::HashingEmbedder`]. The on-disk `.md` files are the only
//! source of truth; everything else is a derived, rebuildable index.

#![warn(missing_docs)]

pub mod base;
pub mod capture;
pub mod entry;
pub mod vault;

pub use base::{KnowledgeBase, KnowledgeConfig, KnowledgeDraft, KnowledgeHit};
pub use capture::{
    detect_recovery, detect_session_digest, is_session_substantive, is_worth_capturing,
    RecoverySignal, SessionDigest, SUBSTANTIVE_CHAR_THRESHOLD,
};
pub use entry::{EntryKind, EntryStatus, KnowledgeEntry, Scope};
pub use vault::Vault;

// Re-export the default offline embedder so downstream crates (app-core) can
// name the concrete `KnowledgeBase<HashingEmbedder>` type without taking a
// direct dependency on `deepagent-memory`.
pub use deepagent_memory::HashingEmbedder;
