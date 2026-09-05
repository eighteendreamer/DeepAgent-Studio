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
pub mod command_guard_llm;
pub mod commands;
pub mod completion_plan;
pub mod context_runtime;
pub mod cost_service;
pub mod diff;
pub mod doctor;
pub mod dto;
pub mod file_preview_service;
pub mod git_service;
pub mod hook_assembly;
pub mod hook_runtime;
pub mod input_runtime;
pub mod kernel_runtime;
pub mod knowledge_service;
pub mod managed_files;
pub mod mcp_runtime;
pub mod mcp_service;
pub mod mobile_service;
pub mod model_runtime;
pub mod nested_instructions;
pub mod office_service;
pub mod permissions_prompt;
pub mod plan_mode_reminder;
pub mod plugin;
pub mod plugin_dependency;
pub mod plugin_loader;
pub mod plugin_manifest;
pub mod plugin_marketplace;
pub mod plugin_runtime;
pub mod plugin_security;
pub mod plugin_service;
pub mod project_map_service;
pub mod project_service;
pub mod prompt_gate;
pub mod recording_service;
pub mod run_config;
pub mod run_coordinator;
pub mod run_environment;
pub mod run_finalizer;
pub mod run_graph;
pub mod runtime_event_log;
pub mod runtime_service;
pub mod sandbox_backend;
pub mod sandboxie_service;
pub mod secret_store;
pub mod service;
pub mod session_state_service;
pub mod settings;
pub mod skill_catalog_reminder;
pub mod skills_service;
pub mod slash_panel;
pub mod speech_service;
pub mod stall_classifier;
pub mod subagent_runner;
pub mod system_context;
pub mod system_prompt;
pub mod system_reminder;
pub mod terminal_lease;
pub mod terminal_service;
pub mod todo_snapshot_reminder;
pub mod tool_manifest;
pub mod tool_runtime;
pub mod trust_service;
pub mod verification_decorator;
pub mod verification_dispatcher;
pub mod verification_panel;
pub mod vision_cache_service;
pub mod vision_provider_service;
pub mod vision_service;
pub mod workspace_service;

pub use approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
pub use archive_service::ArchiveService;
pub use attachment_service::AttachmentService;
pub use chat_service::{ChatService, HarnessRunOverrides};
pub use commands::{
    builtin_commands, commands_from_roots, commands_from_roots_and_plugins, filter_commands,
};
pub use cost_service::{BudgetConfig, CostRecord, CostService, CostSummary, ModelPricing};
pub use deepagent_persistence::subagent_store::SubagentRunRecord;
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
    PreflightToolCallDto, PreviewMetadataDto, PreviewResultDto, ProjectDto, RecordingSessionDto,
    RewindResultDto, RunRecoveryDto, RuntimeProgressDto, RuntimeRootsDto, RuntimeStatusDto,
    SessionDetailDto, SessionStatsDto, SessionSummaryDto, SessionUiPrefsDto, SheetPreviewDto,
    TerminalResultDto, TimelineEntryDto, TranscriptDto, TranscriptSegmentDto,
    VisionRecognizeRequestDto, VisionRecognizeResultDto, WorkspaceInfoDto,
};
pub use file_preview_service::FilePreviewService;
pub use git_service::GitService;
pub use knowledge_service::{
    KnowledgeDraftDto, KnowledgeDto, KnowledgeHitDto, KnowledgeService, KnowledgeServiceBackend,
};
pub use managed_files::ManagedFileInventory;
pub use mcp_service::{McpConnectionStatusDto, McpServerDto, McpService, McpToolInfoDto};
pub use office_service::{markdown_to_docspec, DocBlock, DocSpec, OfficeService};
pub use plugin_loader::{PluginLoadError, PluginOrigin, PluginRoots};
pub use plugin_marketplace::{
    AddPluginMarketplaceDto, PluginMarketplaceDto, PluginMarketplaceEntriesQueryDto,
    PluginMarketplaceEntryDto, PluginMarketplacePageDto,
};
pub use plugin_runtime::{
    PluginAgentRoot, PluginAppEntry, PluginCommandRoot, PluginConnectorEntry,
    PluginMcpServerSource, PluginOutputStyleEntry, PluginRuntimeError, PluginRuntimeProjection,
};
pub use plugin_security::{PluginComponentSummaryDto, PluginRiskItemDto, PluginScanReportDto};
pub use plugin_service::{
    CreatePluginDraftDto, PluginDto, PluginExecutionKind, PluginHealthStatus, PluginLicenseStatus,
    PluginLifecycleState, PluginRuntimeInspectionDto, PluginService, PluginSourceDto,
    PreparedPluginInstallDto,
};
pub use project_map_service::{
    ProjectMapEdgeDto, ProjectMapGraphDto, ProjectMapHitDto, ProjectMapImpactDto,
    ProjectMapNeighborDto, ProjectMapNeighborsDto, ProjectMapNodeDto, ProjectMapOverviewDto,
    ProjectMapRefreshDto, ProjectMapService, ProjectMapStatusDto,
};
pub use project_service::{folder_name, ProjectService};
pub use recording_service::{AudioRecorder, RecordingService, UnavailableRecorder};
pub use run_coordinator::{CoordinatorReadiness, RunCoordinator};
pub use runtime_service::{
    default_registry, ArchiveKind, Downloader, Platform, RuntimeArtifact, RuntimeBroker,
    RuntimeDiagnostic, RuntimeEntry, RuntimeKind, RuntimePreference, RuntimeRequirement,
    RuntimeResolution, RuntimeService, RuntimeSource, UnavailableDownloader,
};
pub use sandbox_backend::{
    DirectSandboxBackend, SandboxBackend, SandboxBackendCommandExecutor, SandboxBackendKind,
    SandboxCapabilities, SandboxExecutionRequest, SandboxExecutionResult, SandboxNetworkPolicy,
    SandboxieBackend, WindowsSandboxBackend, WindowsSandboxTaskPlan, WindowsSandboxTaskPlanRequest,
};
pub use sandboxie_service::{SandboxieExecutor, SandboxieService, SandboxieStatusDto};
pub use secret_store::{EnvSecretStore, MemorySecretStore, SecretStore, SqliteSecretStore};
pub use service::AppService;
pub use session_state_service::SessionStateService;
pub use settings::{
    AppSettings, ApprovalPolicy, BalanceDto, BalanceInfoDto, EffectivePermissionProfile,
    ExecutionFeatures, LocalExecutionMode, OutputStyle, PermissionPreset,
    PermissionPresetVisibility, SandboxMode, SettingsService, SettingsView, TerminalShell,
    VerificationPolicy, VisionMode, VisionSettings, WebSearchProvider, WebSearchSettings,
};
pub use skill_catalog_reminder::SkillCatalogSendState;
pub use skills_service::{
    ai_security_review, parse_verdict, AiReviewResult, ReviewDepth, SkillActivationDto, SkillDto,
    SkillsService, AI_SECURITY_REVIEW_SYSTEM_PROMPT,
};
pub use slash_panel::{kv, SlashPanel, SlashPanelItem, SlashSection};
pub use speech_service::{SpeechService, TranscriptionEngine, UnavailableEngine};
pub use trust_service::{ProjectTrustDto, TrustService};

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
pub use deepagent_terminal;
pub use run_graph::{RunGraphNodeDto, RunGraphViewDto};
pub use terminal_lease::SqliteTerminalLeaseStore;
pub use terminal_service::{
    DirectTerminalSessionBackend, LocalPtyHandle, PtyReadChunk, TerminalService,
};
pub use vision_service::VisionService;
pub use workspace_service::WorkspaceService;

// Re-export the live runtime event + approval types so the Tauri/web layer can
// forward them.
pub use deepagent_hooks::{
    HookAction, HookActionType, HookCommandResult, HookCommandRunner, HookCommandShell,
    HookDefinitions, PermissionRules, SystemHookRunner,
};
pub use deepagent_persistence::run_store::{RunRecord, StoredRunEvent};
pub use deepagent_persistence::runtime_log_store::{
    NewRuntimeLogEntry, RuntimeLogEntry, RuntimeLogStore,
};
pub use deepagent_runtime::{ApprovalDecision, RuntimeEvent};
pub use hook_runtime::{test_hook_action, HookActionTestResult};
pub use model_runtime::build_chat_model_client;

#[cfg(feature = "keychain")]
pub use secret_store::KeychainStore;
