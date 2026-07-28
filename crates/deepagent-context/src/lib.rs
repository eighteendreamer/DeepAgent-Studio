//! # deepagent-context
//!
//! Context engineering (开发提示词.md §4, §6, §7; 开发计划.md Phase 3).
//!
//! Building blocks:
//! - [`prompt`]  — the Prompt Compiler's [`prompt::PromptFragment`] AST: prompts
//!   are *structured* (priority + source + content), not raw strings.
//! - [`budget`]  — the [`budget::PromptBudget`] token-economy system that decides
//!   which fragments survive when the context window is tight.
//! - [`tokenizer`] — a pluggable [`tokenizer::TokenCounter`] (a heuristic
//!   estimator now; a real BPE tokenizer can be slotted in later).
//!
//! Composed pipeline:
//! - [`pipeline`]   — the **Five-Layer Context Pipeline**: recent conversation,
//!   task summary, memory injection, workspace context, semantic retrieval —
//!   fitted to a budget.
//! - [`compaction`] — structured-summary compaction (开发提示词.md §4 Layer 2)
//!   that compresses older turns when the window grows too large.

pub mod assembler;
pub mod budget;
pub mod compaction;
pub mod config_overlay;
pub mod model_compactor;
pub mod pack;
pub mod pipeline;
pub mod policy;
pub mod prompt;
pub mod tokenizer;

pub use assembler::{ContextAssembler, ContextEntry, ContextManifest, ContextSourceKind};
pub use budget::{BudgetOutcome, PromptBudget};
pub use compaction::{
    CompactionPolicy, CompactionResult, Compactor, HeuristicSummarizer, Summarizer, TaskSummary,
};
pub use config_overlay::{
    ConfigLayer, ConfigOverlay, ConfigSource, DualConfigLoader, MANAGED_PRECEDENCE,
};
pub use model_compactor::ModelCompactor;
pub use pack::{
    CacheScope, ContextBlock, ContextBlockKind, ContextBlockUsage, ContextPack,
    ContextUsageSnapshot,
};
pub use pipeline::ContextPipeline;
pub use policy::ContextPolicy;
pub use prompt::{CompiledPrompt, PromptFragment, PromptSource};
pub use tokenizer::{HeuristicTokenizer, TokenCounter};
