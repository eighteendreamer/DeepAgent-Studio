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
    /// The project this session belongs to (folder name for display), or null.
    pub project: Option<String>,
    /// Optional title.
    pub title: Option<String>,
    /// Run mode label (normal / resumed / remote / …).
    pub mode: String,
    /// Created-at, Unix ms.
    pub created_at: i64,
    /// Updated-at, Unix ms.
    pub updated_at: i64,
    /// Whether the session has ended.
    pub ended: bool,
    /// Whether this session is pinned to the top of the sidebar.
    pub pinned: bool,
}

/// A project (a folder) the user has opened, with its session count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectDto {
    /// The project folder name (display), e.g. "小红草".
    pub name: String,
    /// The absolute project root path (stable key).
    pub path: String,
    /// Whether this project is pinned to the top of the sidebar.
    pub pinned: bool,
    /// Number of sessions under this project.
    pub session_count: u32,
    /// Most-recent session update under this project, Unix ms (0 if none).
    pub updated_at: i64,
}

/// A conversation hidden from the live sidebar by app-level archive state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedConversationDto {
    /// Session id.
    pub session_id: String,
    /// Optional session title.
    pub title: Option<String>,
    /// Project folder display name when known.
    pub project: Option<String>,
    /// Stable project path when known.
    pub project_path: Option<String>,
    /// When this conversation was archived, Unix ms.
    pub archived_at: i64,
    /// The session's last update time, Unix ms.
    pub updated_at: i64,
}

/// Result of archiving a project's visible conversations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveProjectResultDto {
    /// The project path that was archived.
    pub project_path: String,
    /// Display name for the project.
    pub project_name: String,
    /// Number of newly archived conversations.
    pub archived_count: u32,
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

/// One ordered segment of a reconstructed assistant/user turn, mirroring the
/// live chat's `MessagePart` so a returned-to session renders with the same
/// styled tool cards / reasoning / text (not flattened timeline lines).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationPartDto {
    /// Visible assistant/user text.
    Text {
        /// The text content.
        text: String,
    },
    /// Thinking-Mode reasoning trace.
    Reasoning {
        /// The reasoning text.
        text: String,
    },
    /// A tool call card (request + completion folded together).
    Tool {
        /// Correlation id.
        call_id: String,
        /// Tool name.
        name: String,
        /// JSON-stringified arguments (pretty).
        args: String,
        /// Status: "ok" | "error" (completed) or "running" (no completion seen).
        status: String,
        /// Wall-clock duration ms when known.
        duration_ms: Option<u64>,
        /// One-line result/error summary.
        detail: Option<String>,
    },
}

/// A reconstructed conversation message (user or assistant) for replaying a
/// session with full styling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationMessageDto {
    /// "user" or "assistant".
    pub role: String,
    /// Flat text mirror (for user turns / copy / fallback).
    pub content: String,
    /// Ordered parts (assistant turns carry reasoning / tool / text in order).
    pub parts: Vec<ConversationPartDto>,
    /// Persisted token usage for this turn, when recorded (assistant turns).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ConversationUsageDto>,
}

/// Persisted per-turn token usage + duration (mirrors `UsageRecorded`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationUsageDto {
    /// Prompt (input) tokens.
    pub prompt_tokens: u32,
    /// Completion (output) tokens.
    pub completion_tokens: u32,
    /// Total tokens.
    pub total_tokens: u32,
    /// Prompt tokens served from the context cache.
    pub prompt_cache_hit_tokens: u32,
    /// Prompt tokens NOT served from cache.
    pub prompt_cache_miss_tokens: u32,
    /// Wall-clock run duration in milliseconds.
    pub duration_ms: u64,
}

/// A command-palette action the UI can present and dispatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDto {
    /// Stable command id (e.g. "session.new").
    pub id: String,
    /// Display title.
    pub title: String,
    /// Short explanation shown in command pickers.
    pub description: String,
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

/// Result of forking a session: identifies the new branch and how much of the
/// source timeline it copied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkResultDto {
    /// The new (forked) session id.
    pub new_session_id: String,
    /// The source session id the fork branched from.
    pub source_session_id: String,
    /// The sequence number the fork was taken at (inclusive).
    pub forked_at: u64,
}

/// Result of rewinding a session in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewindResultDto {
    /// The session that was rewound.
    pub session_id: String,
    /// Sequence number kept through (inclusive).
    pub kept_through: u64,
    /// Number of events discarded by the rewind.
    pub events_removed: u64,
}

/// An exported session transcript ready for the UI to save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptDto {
    /// The session id.
    pub session_id: String,
    /// Format label ("markdown" / "json").
    pub format: String,
    /// Suggested file extension ("md" / "json").
    pub extension: String,
    /// The rendered transcript content.
    pub content: String,
}

/// Identifies the active project: the working-directory the agent operates in.
/// The UI shows the folder **name**; all agent file operations are rooted here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfoDto {
    /// The project folder name (last path component), e.g. "小红草".
    pub name: String,
    /// The absolute project root path.
    pub path: String,
}

/// The result of running a one-shot terminal command in the active project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResultDto {
    /// The command that was run.
    pub command: String,
    /// The working directory it ran in (the active project root).
    pub cwd: String,
    /// Exit code (null if killed by signal).
    pub exit_code: Option<i32>,
    /// Captured stdout.
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
    /// True when the command was refused as dangerous (needs approval) and not
    /// executed at all.
    pub blocked: bool,
}
