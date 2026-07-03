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

/// Durable UI preferences bound to a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionUiPrefsDto {
    /// Whether the environment info panel may auto-open when new outputs appear.
    pub env_panel_auto_open: bool,
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

/// Current Git state for one opened project/worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitProjectStatusDto {
    /// Project path that was queried.
    pub project_path: String,
    /// Repository root if the project is inside a Git work tree.
    pub repo_root: Option<String>,
    /// Stable repository/worktree group id for multi-project grouping.
    pub repo_id: Option<String>,
    /// Whether `project_path` is inside a Git work tree.
    pub is_repo: bool,
    /// Current branch, or short commit id in detached HEAD state.
    pub current_branch: Option<String>,
    /// True when HEAD is detached.
    pub detached_head: bool,
    /// Upstream tracking branch, when configured.
    pub upstream: Option<String>,
    /// Commits ahead of upstream.
    pub ahead: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// True when staged/unstaged/untracked/conflicted files exist.
    pub has_changes: bool,
    /// Number of files with changes.
    pub files_changed: u32,
    /// Added lines across the working tree diff.
    pub additions: u32,
    /// Deleted lines across the working tree diff.
    pub deletions: u32,
    /// `merge` or `apply` while an interactive/non-interactive rebase is active.
    pub rebase_state: Option<String>,
    /// True while a merge is in progress.
    pub merge_state: bool,
    /// Whether GitHub CLI is available on PATH.
    pub gh_available: bool,
}

/// One Git branch row for a branch popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranchDto {
    /// Display branch name, e.g. `main` or `origin/main`.
    pub name: String,
    /// Full ref name, e.g. `refs/heads/main`.
    pub full_name: String,
    /// `local` or `remote`.
    pub kind: String,
    /// Whether this is the active branch for the queried worktree.
    pub current: bool,
    /// Upstream tracking branch for local refs.
    pub upstream: Option<String>,
    /// Commits ahead of upstream for local refs.
    pub ahead: u32,
    /// Commits behind upstream for local refs.
    pub behind: u32,
    /// Short head commit id.
    pub commit: Option<String>,
    /// Commit subject.
    pub subject: Option<String>,
    /// Path of a worktree using this local branch, when Git reports one.
    pub worktree_path: Option<String>,
}

/// One changed file row in the Git changes panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChangedFileDto {
    /// Relative path in the repository.
    pub path: String,
    /// Optional previous path for renames/copies.
    pub old_path: Option<String>,
    /// Two-column porcelain status, e.g. `M `, ` M`, `??`, `UU`.
    pub status: String,
    /// Coarse group: staged / unstaged / untracked / conflicted.
    pub category: String,
    /// Added lines for this path, when available.
    pub additions: u32,
    /// Deleted lines for this path, when available.
    pub deletions: u32,
}

/// Working tree changes for a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitChangesDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub is_repo: bool,
    pub files: Vec<GitChangedFileDto>,
    pub additions: u32,
    pub deletions: u32,
}

/// Unified diff text for one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiffDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub file_path: String,
    pub staged: bool,
    pub is_repo: bool,
    pub text: String,
    pub truncated: bool,
}

/// One changed file in a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitFileDto {
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
}

/// One row in the Git Log view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitLogEntryDto {
    pub hash: String,
    pub full_hash: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub date: String,
    pub refs: Vec<String>,
    pub subject: String,
    pub files: Vec<GitCommitFileDto>,
}

/// One commit that would be pushed to the configured upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPushCommitDto {
    pub hash: String,
    pub full_hash: String,
    pub author_name: String,
    pub date: String,
    pub subject: String,
}

/// Safe preview payload for a push operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPushPreviewDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub is_repo: bool,
    pub current_branch: Option<String>,
    pub upstream: Option<String>,
    pub remote: Option<String>,
    pub remote_branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub commits: Vec<GitPushCommitDto>,
    /// Present when pushing should be blocked until the user fixes the state.
    pub blocked_reason: Option<String>,
}

/// One pre-push risk finding. This is intentionally advisory and read-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPushRiskItemDto {
    /// `high`, `medium`, or `low`.
    pub severity: String,
    /// Stable category such as `secret`, `large_file`, `debug_log`, or `binary_file`.
    pub category: String,
    pub title: String,
    pub detail: String,
    pub file_path: Option<String>,
}

/// Local pre-push risk scan over outgoing commits/diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitPushRiskScanDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub is_repo: bool,
    pub current_branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub scanned_files: u32,
    pub risks: Vec<GitPushRiskItemDto>,
    /// Present when scanning cannot run because pushing itself is not currently valid.
    pub blocked_reason: Option<String>,
}

/// One commit that exists on exactly one side of a ref comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCompareCommitDto {
    /// `base` when the commit is only in `base_ref`, `target` when only in `target_ref`.
    pub side: String,
    pub hash: String,
    pub full_hash: String,
    pub author_name: String,
    pub date: String,
    pub subject: String,
}

/// Safe read-only comparison between two refs in one repository/worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRefCompareDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub is_repo: bool,
    pub base_ref: String,
    pub target_ref: String,
    pub merge_base: Option<String>,
    /// Commits on `target_ref` that are not on `base_ref`.
    pub ahead: u32,
    /// Commits on `base_ref` that are not on `target_ref`.
    pub behind: u32,
    pub commits: Vec<GitCompareCommitDto>,
    pub files: Vec<GitCommitFileDto>,
    /// Present when comparison should be blocked until the user fixes the input/state.
    pub blocked_reason: Option<String>,
}

/// One project/worktree target selected for a batch Git action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBatchCommitTargetDto {
    pub project_path: String,
    /// Optional per-project commit message override.
    pub message: Option<String>,
}

/// Read-only preview row for a project selected in the batch commit/upload UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBatchCommitPreviewItemDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub is_repo: bool,
    pub current_branch: Option<String>,
    pub files_changed: u32,
    pub staged_files: u32,
    pub additions: u32,
    pub deletions: u32,
    pub ahead: u32,
    pub behind: u32,
    pub blocked_reason: Option<String>,
}

/// Result row for one project in a batch commit/upload operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBatchProjectResultDto {
    pub project_path: String,
    pub current_branch: Option<String>,
    pub ok: bool,
    pub committed: bool,
    pub pushed: bool,
    pub skipped: bool,
    pub message: String,
    pub commit_result: Option<GitOperationResultDto>,
    pub push_result: Option<GitOperationResultDto>,
}

/// Result of a local Git operation initiated by the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitOperationResultDto {
    pub ok: bool,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
}

/// Suggested commit message generated from staged changes, or all changes when
/// nothing is staged yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommitMessageDraftDto {
    pub project_path: String,
    pub repo_root: Option<String>,
    pub is_repo: bool,
    /// `staged` when based on staged files, `working_tree` when based on all changes.
    pub source: String,
    pub title: String,
    pub body: String,
    pub files: Vec<GitChangedFileDto>,
    /// Present when a draft cannot be produced.
    pub blocked_reason: Option<String>,
}

/// One `git worktree list --porcelain` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitWorktreeDto {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
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
        /// Raw tool output, when available. The UI uses this for richer tool
        /// cards (for example, web_search provider diagnostics) while keeping
        /// `detail` as a compact fallback.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
    /// Backend-computed RMB cost for this assistant turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_yuan: Option<f64>,
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

/// Metadata about a file selected for preview (office-agent file preview).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewMetadataDto {
    /// Absolute path of the file.
    pub path: String,
    /// File name (last path component).
    pub name: String,
    /// Lowercased extension without the dot (e.g. "docx"), empty if none.
    pub ext: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Classified kind: "text" | "image" | "pdf" | "docx" | "xlsx" | "pptx"
    /// | "csv" | "unknown".
    pub kind: String,
}

/// One sheet's preview (first N rows) for an xlsx workbook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetPreviewDto {
    /// Sheet name.
    pub name: String,
    /// First N rows, each a list of cell strings.
    pub rows: Vec<Vec<String>>,
    /// True when more rows exist beyond the previewed window.
    pub truncated: bool,
}

/// The result of extracting a previewable representation of a file.
///
/// Tier C (pure-Rust) preview: text for text/markdown/json/csv/docx/pptx/pdf,
/// sheet tables for xlsx. Images carry no text — the frontend renders them
/// directly from `metadata.path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewResultDto {
    /// File metadata (includes the classified `kind`).
    pub metadata: PreviewMetadataDto,
    /// Extracted text content when applicable (None for images / xlsx).
    pub text: Option<String>,
    /// Sheet previews for xlsx workbooks (None otherwise).
    pub sheets: Option<Vec<SheetPreviewDto>>,
    /// True when `text` was truncated to the preview byte cap.
    pub truncated: bool,
    /// Optional human-readable note (e.g. "rendered as text fallback").
    pub message: Option<String>,
}

// ---- managed runtimes (office-agent RuntimeService) -----------------------

/// Status of one managed runtime (downloadable, installed into the app's own
/// runtimes dir). Surfaced by `runtime_list` / `runtime_status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatusDto {
    /// Stable id, e.g. "whisper-base" / "pdfium" / "pandoc".
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Version label.
    pub version: String,
    /// Capability this runtime provides, e.g. "speech-model" / "pdf-render".
    pub capability: String,
    /// Approximate download size in bytes (for the consent prompt).
    pub size_bytes: u64,
    /// True when installed and present on disk.
    pub installed: bool,
    /// Whether this runtime has an artifact for the current platform.
    pub available_for_platform: bool,
    /// Whether the artifact carries a pinned SHA-256 (installable). When false,
    /// install is blocked until a checksum is pinned (fail-closed integrity).
    pub checksum_pinned: bool,
    /// Absolute install path when installed, else null.
    pub install_path: Option<String>,
}

/// Progress event payload for a runtime download (emitted as `runtime:progress`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeProgressDto {
    /// The runtime id being installed.
    pub id: String,
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Total bytes when known (from Content-Length), else null.
    pub total: Option<u64>,
    /// Phase label: "downloading" | "verifying" | "extracting" | "done" | "error".
    pub phase: String,
}

// ---- recording + transcription (office-agent) -----------------------------

/// A recording session's lifecycle state for the recording panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSessionDto {
    /// Opaque session id.
    pub id: String,
    /// Status: "idle" | "recording" | "paused" | "transcribing" | "done" | "error".
    pub status: String,
    /// When recording started, Unix ms.
    pub started_at: i64,
    /// Elapsed duration in ms (set on stop).
    pub duration_ms: u64,
    /// Absolute path of the captured WAV, when available.
    pub audio_path: Option<String>,
    /// Absolute path of the transcript JSON, when available.
    pub transcript_path: Option<String>,
    /// Human-readable error message when `status == "error"`.
    pub error: Option<String>,
}

/// One transcript segment (mirrors the planned TranscriptSegment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegmentDto {
    /// Segment start, ms from the recording start.
    pub start_ms: u64,
    /// Segment end, ms from the recording start.
    pub end_ms: u64,
    /// Recognized text.
    pub text: String,
    /// Optional speaker label.
    pub speaker: Option<String>,
    /// Optional confidence in [0,1].
    pub confidence: Option<f32>,
}

/// Result of a PDF render request (office-agent). Tier R returns page image
/// paths; Tier C returns extracted text with a note that pdfium is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfRenderResultDto {
    /// True when pages were rasterized (pdfium installed + feature enabled).
    pub rendered: bool,
    /// Absolute paths of rendered page PNGs (empty when not rendered).
    pub pages: Vec<String>,
    /// Extracted text (Tier C degrade) when pages were not rendered.
    pub text: Option<String>,
    /// Human-readable note (e.g. "install pdfium to render pages").
    pub message: Option<String>,
}
