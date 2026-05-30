//! # deepagent-observation
//!
//! Observability for the runtime (开发提示词.md §18; 开发计划.md Phase 10).
//!
//! Two complementary views, both derived from the append-only event log so they
//! are **replayable**:
//! - [`timeline`] — the [`timeline::TimelineEntry`] Agent Timeline: a
//!   time-ordered, display-ready replay of everything the agent did.
//! - [`stats`] — [`stats::SessionStats`]: aggregated runtime / token / cache /
//!   tool metrics folded from the same events.
//!
//! The live in-process counters live in [`deepagent_tracing::metrics`]; this
//! crate reconstructs equivalent analytics from durable history (past sessions,
//! the UI timeline panel, audits). OpenTelemetry/OTLP export layers onto the
//! tracing subscriber without changing these projections.

pub mod stats;
pub mod timeline;
pub mod transcript;

pub use stats::SessionStats;
pub use timeline::{build_timeline, TimelineEntry};
pub use transcript::{export_transcript, TranscriptFormat};

// Re-export the live metrics types for convenience so consumers have one import.
pub use deepagent_tracing::metrics::{Metrics, MetricsSnapshot};
