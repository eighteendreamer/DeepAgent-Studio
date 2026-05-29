//! # deepagent-workspace
//!
//! Workspace intelligence (开发提示词.md §4 Layer 4; 开发计划.md Phase 3 §2).
//!
//! Many agents fail because they don't understand the whole workspace. This
//! crate scans a project into a compact, serializable [`WorkspaceSnapshot`]:
//! the file tree, detected ecosystem(s), manifest summaries, and a README
//! excerpt. The snapshot is what Layer 4 of the context pipeline injects so the
//! agent reasons with project structure in view.
//!
//! The scan is bounded by [`ScanLimits`] (depth / entries / README bytes) and
//! skips build output, VCS, and dependency directories, keeping it fast and
//! within a token budget.

pub mod scanner;
pub mod snapshot;

pub use scanner::{ScanLimits, WorkspaceScanner};
pub use snapshot::{ManifestInfo, ProjectKind, WorkspaceSnapshot};
