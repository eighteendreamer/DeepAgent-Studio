//! # deepagent-app-core
//!
//! The application-service façade (开发计划.md Phase 8 backing layer).
//!
//! This crate is the stable boundary between the kernel and any UI. It exposes:
//! - [`dto`] — plain serializable shapes the frontend consumes,
//! - [`service::AppService`] — DTO-returning operations (list sessions, open a
//!   session with its timeline + stats).
//!
//! Tauri commands and a potential web backend are thin wrappers over
//! [`service::AppService`]; the UI never touches kernel internals, so the wire
//! contract stays stable as the kernel evolves.

pub mod approval_bridge;
pub mod archive_service;
pub mod attachment_service;
pub mod chat_service;
pub mod commands;
pub mod cost_service;
pub mod diff;
pub mod doctor;
pub mod dto;
pub mod file_preview_service;
pub mod git_service;
pub mod knowledge_service;
pub mod mcp_service;
pub mod office_service;
pub mod plan_mode_reminder;
pub mod project_map_service;
pub mod project_service;
pub mod recording_service;
pub mod runtime_service;
pub mod secret_store;
pub mod service;
pub mod session_state_service;
pub mod settings;
pub mod skill_catalog_reminder;
pub mod skills_service;
pub mod speech_service;
pub mod system_prompt;
pub mod system_reminder;
pub mod terminal_service;
pub mod todo_snapshot_reminder;
pub mod verification_decorator;
pub mod verification_dispatcher;
pub mod vision_service;
pub mod workspace_service;

pub use approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
pub use archive_service::ArchiveService;
pub use attachment_service::AttachmentService;
pub use chat_service::ChatService;
pub use commands::{builtin_commands, commands_from_roots, filter_commands};
pub use cost_service::{BudgetConfig, CostRecord, CostService, CostSummary, ModelPricing};
pub use diff::{diff_lines, DiffKind, DiffLine, DiffResult};
pub use doctor::{format_diagnostics, run_diagnostics, DiagStatus, DiagnosticResult};
pub use dto::{
    ApprovalRequestDto, ArchiveProjectResultDto, ArchivedConversationDto, AttachmentDto,
    AttachmentIngestDto, CommandDto, ConversationMessageDto, ConversationPartDto,
    ConversationUsageDto, ForkResultDto, GitBatchCommitPreviewItemDto, GitBatchCommitTargetDto,
    GitBatchProjectResultDto, GitBranchDto, GitChangedFileDto, GitChangesDto, GitCommitFileDto,
    GitCommitMessageDraftDto, GitCompareCommitDto, GitDiffDto, GitLogEntryDto,
    GitOperationResultDto, GitProjectStatusDto, GitPushCommitDto, GitPushPreviewDto,
    GitPushRiskItemDto, GitPushRiskScanDto, GitRefCompareDto, GitWorktreeDto, PdfRenderResultDto,
    PreviewMetadataDto, PreviewResultDto, ProjectDto, RecordingSessionDto, RewindResultDto,
    RuntimeProgressDto, RuntimeRootsDto, RuntimeStatusDto, SessionDetailDto, SessionStatsDto,
    SessionSummaryDto, SessionUiPrefsDto, SheetPreviewDto, TerminalResultDto, TimelineEntryDto,
    TranscriptDto, TranscriptSegmentDto, VisionRecognizeRequestDto, VisionRecognizeResultDto,
    WorkspaceInfoDto,
};
pub use file_preview_service::FilePreviewService;
pub use git_service::GitService;
pub use knowledge_service::{
    KnowledgeDraftDto, KnowledgeDto, KnowledgeHitDto, KnowledgeService, KnowledgeServiceBackend,
};
pub use mcp_service::{McpServerDto, McpService};
pub use office_service::{markdown_to_docspec, DocBlock, DocSpec, OfficeService};
pub use project_map_service::{
    ProjectMapEdgeDto, ProjectMapGraphDto, ProjectMapHitDto, ProjectMapImpactDto,
    ProjectMapNeighborDto, ProjectMapNeighborsDto, ProjectMapNodeDto, ProjectMapOverviewDto,
    ProjectMapRefreshDto, ProjectMapService, ProjectMapStatusDto,
};
pub use project_service::{folder_name, ProjectService};
pub use recording_service::{AudioRecorder, RecordingService, UnavailableRecorder};
pub use runtime_service::{
    default_registry, ArchiveKind, Downloader, Platform, RuntimeArtifact, RuntimeEntry,
    RuntimeService, UnavailableDownloader,
};
pub use secret_store::{EnvSecretStore, MemorySecretStore, SecretStore};
pub use service::AppService;
pub use session_state_service::SessionStateService;
pub use settings::{
    AppSettings, ApprovalPolicy, BalanceDto, BalanceInfoDto, SandboxMode, SettingsService,
    SettingsView, TerminalShell, VerificationPolicy, VisionMode, VisionSettings, WebSearchProvider,
    WebSearchSettings,
};
pub use skill_catalog_reminder::SkillCatalogSendState;
pub use skills_service::{
    ai_security_review, parse_verdict, AiReviewResult, ReviewDepth, SkillActivationDto, SkillDto,
    SkillsService, AI_SECURITY_REVIEW_SYSTEM_PROMPT,
};
pub use speech_service::{SpeechService, TranscriptionEngine, UnavailableEngine};

// Re-export marketplace types from `deepagent-skills` so the desktop Tauri
// layer can plumb the SkillsMP client handle into `AppState` without taking a
// direct dependency on the skills crate.
pub use deepagent_skills::{
    scan_dir, ApiKeySource, GithubLocator, MarketSearchData, MarketSkill, Pagination, ScanReport,
    SearchQuery, SkillsMpClient, SkillsMpClientHandle, SkillsRoots, SortBy, TempSkillDir,
};

// Tool-search lazy loading (tool-search spec): re-exposed via app-core so the
// desktop Tauri layer + downstream callers don't have to depend on
// `deepagent-builtins` directly to get the user-facing config enum.
pub use deepagent_builtins::ToolSearchMode;
pub use terminal_service::{LocalPtyHandle, TerminalService};
pub use vision_service::VisionService;
pub use workspace_service::WorkspaceService;

// Re-export the live runtime event + approval types so the Tauri/web layer can
// forward them.
pub use deepagent_hooks::{HookDefinitions, PermissionRules};
pub use deepagent_runtime::{ApprovalDecision, RuntimeEvent};

#[cfg(feature = "keychain")]
pub use secret_store::KeychainStore;
