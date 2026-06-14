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
pub mod chat_service;
pub mod commands;
pub mod cost_service;
pub mod diff;
pub mod doctor;
pub mod dto;
pub mod knowledge_service;
pub mod mcp_service;
pub mod project_map_service;
pub mod project_service;
pub mod secret_store;
pub mod service;
pub mod session_state_service;
pub mod settings;
pub mod skills_service;
pub mod terminal_service;
pub mod workspace_service;

pub use approval_bridge::{ChannelApprovalGate, PendingApprovals, PolicyGate};
pub use archive_service::ArchiveService;
pub use chat_service::ChatService;
pub use commands::{builtin_commands, commands_from_roots, filter_commands};
pub use cost_service::{BudgetConfig, CostRecord, CostService, CostSummary, ModelPricing};
pub use diff::{diff_lines, DiffKind, DiffLine, DiffResult};
pub use doctor::{format_diagnostics, run_diagnostics, DiagStatus, DiagnosticResult};
pub use dto::{
    ApprovalRequestDto, ArchiveProjectResultDto, ArchivedConversationDto, CommandDto,
    ConversationMessageDto, ConversationPartDto, ConversationUsageDto, ForkResultDto, ProjectDto,
    RewindResultDto, SessionDetailDto, SessionStatsDto, SessionSummaryDto, TerminalResultDto,
    TimelineEntryDto, TranscriptDto, WorkspaceInfoDto,
};
pub use knowledge_service::{
    KnowledgeDraftDto, KnowledgeDto, KnowledgeHitDto, KnowledgeService, KnowledgeServiceBackend,
};
pub use mcp_service::{McpServerDto, McpService};
pub use project_map_service::{
    ProjectMapEdgeDto, ProjectMapGraphDto, ProjectMapHitDto, ProjectMapImpactDto,
    ProjectMapNeighborDto, ProjectMapNeighborsDto, ProjectMapNodeDto, ProjectMapOverviewDto,
    ProjectMapRefreshDto, ProjectMapService, ProjectMapStatusDto,
};
pub use project_service::{folder_name, ProjectService};
pub use secret_store::{EnvSecretStore, MemorySecretStore, SecretStore};
pub use service::AppService;
pub use session_state_service::SessionStateService;
pub use settings::{
    AppSettings, ApprovalPolicy, BalanceDto, BalanceInfoDto, SandboxMode, SettingsService,
    SettingsView,
};
pub use skills_service::{SkillActivationDto, SkillDto, SkillsService};
pub use terminal_service::TerminalService;
pub use workspace_service::WorkspaceService;

// Re-export the live runtime event + approval types so the Tauri/web layer can
// forward them.
pub use deepagent_hooks::{HookDefinitions, PermissionRules};
pub use deepagent_runtime::{ApprovalDecision, RuntimeEvent};

#[cfg(feature = "keychain")]
pub use secret_store::KeychainStore;
