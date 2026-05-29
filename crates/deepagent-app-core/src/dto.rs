//! Serializable DTOs — the stable contract between the kernel and the UI.
//!
//! These are plain, `serde`-friendly shapes the frontend (Tauri/web) consumes.
//! Keeping them separate from internal kernel types means the UI never depends
//! on kernel internals and the wire format stays stable as internals evolve.

use serde::{Deserialize, Serialize};

/// Summary of a session for the sidebar list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummaryDto {
    /// Session id (string form).
    pub id: String,
    /// Optional title.
    pub title: Option<String>,
    /// Created-at, Unix ms.
    pub created_at: i64,
    /// Updated-at, Unix ms.
    pub updated_at: i64,
    /// Whether the session has ended.
    pub ended: bool,
}

/// A timeline entry for the Codex-style timeline panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntryDto {
    /// Stable ordering key.
    pub sequence: u64,
    /// Unix ms.
    pub timestamp: i64,
    /// Kind tag (session/task/tool/message/...).
    pub kind: String,
    /// Icon hint.
    pub icon: String,
    /// One-line label.
    pub label: String,
    /// Optional detail.
    pub detail: Option<String>,
    /// Duration ms when known.
    pub duration_ms: Option<u64>,
}

/// Aggregated session statistics for the metrics header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionStatsDto {
    /// Total events.
    pub event_count: u64,
    /// Messages.
    pub messages: u64,
    /// Tool calls.
    pub tool_calls: u64,
    /// Tool successes.
    pub tool_successes: u64,
    /// Tool failures.
    pub tool_failures: u64,
    /// Total tool ms.
    pub total_tool_ms: u64,
    /// Tool success rate in [0,1], or null.
    pub tool_success_rate: Option<f64>,
    /// Session duration ms.
    pub duration_ms: i64,
}

/// A full session detail payload for the main view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionDetailDto {
    /// The session summary.
    pub summary: SessionSummaryDto,
    /// The agent timeline.
    pub timeline: Vec<TimelineEntryDto>,
    /// Aggregated stats.
    pub stats: SessionStatsDto,
}

/// A command-palette action the UI can present and dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDto {
    /// Stable command id (e.g. "session.new").
    pub id: String,
    /// Display title.
    pub title: String,
    /// Grouping category (e.g. "Session", "View").
    pub category: String,
    /// Optional keyboard shortcut hint.
    pub shortcut: Option<String>,
}

/// A pending tool approval awaiting a human decision (high-risk tool gate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestDto {
    /// Correlates the decision back to the blocked tool call.
    pub call_id: String,
    /// The tool name.
    pub tool: String,
    /// Risk label (e.g. "high").
    pub risk: String,
    /// Pretty-printed arguments for review.
    pub arguments: String,
    /// Why approval is required.
    pub reason: String,
}
