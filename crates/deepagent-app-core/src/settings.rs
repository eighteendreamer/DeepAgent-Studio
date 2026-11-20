//! General application settings. Provider/model configuration and credentials
//! are owned by the SQLite harness registry and SecretStore respectively.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use deepagent_builtins::prefix_covers;
use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{bash_prefix_rule, rule_command_prefix, PermissionRules};
use deepagent_models::transport::HttpTransport;
use deepagent_models::ReasoningEffort;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;

use crate::cost_service::ModelPricing;
use crate::secret_store::SecretStore;

/// The document-store collection + id the (non-secret) settings live under.
const SETTINGS_COLLECTION: &str = "settings";
const SETTINGS_ID: &str = "app";
/// The AnySearch API key lives in the same SecretStore.
const ANYSEARCH_API_KEY_NAME: &str = "anysearch_api_key";

pub use deepagent_builtins::web_search::types::{
    EngineConfigState as WebSearchConfigState, EngineConfigStatus as WebSearchEngineStatus,
    EngineId as WebSearchEngineId, EngineSettings as WebSearchEngineSettings,
    RankingMethod as WebSearchRankingMethod, RankingProfile as WebSearchRankingProfile,
    RankingSettings as WebSearchRankingSettings, SearchRequest as WebSearchRequest,
    SearchRuntimeConfig as WebSearchRuntimeConfig, SearchSettings as WebSearchSettings,
    SearchSettingsView as WebSearchSettingsView, SearchStrategy as WebSearchStrategy,
};

/// How tool-approval requests are resolved (maps to the 设置 → 权限 panel).
///
/// - [`ApprovalPolicy::AlwaysAsk`] — 默认权限：每个需要审批的工具调用都向用户申请。
/// - [`ApprovalPolicy::AutoReview`] — 自动审核：系统默认审批通过（写/编辑等工作区操作放行）。
/// - [`ApprovalPolicy::FullAccess`] — 完全访问：全部放行（含高危），风险自负。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    /// Ask the user for every approval-requiring call (the safe default).
    #[default]
    AlwaysAsk,
    /// Auto-approve (system reviews and allows) — "自动审核".
    AutoReview,
    /// Allow everything, including high-risk — "完全访问".
    FullAccess,
}

impl ApprovalPolicy {
    /// Stable label.
    pub const fn label(&self) -> &'static str {
        match self {
            ApprovalPolicy::AlwaysAsk => "always_ask",
            ApprovalPolicy::AutoReview => "auto_review",
            ApprovalPolicy::FullAccess => "full_access",
        }
    }

    /// Whether this policy resolves approvals automatically (no human prompt).
    pub fn is_automatic(&self) -> bool {
        matches!(
            self,
            ApprovalPolicy::AutoReview | ApprovalPolicy::FullAccess
        )
    }
}

/// The user-facing permission preset that governs the full runtime policy.
///
/// This is what the Composer dropdown selects. Each preset maps to a complete
/// [`EffectivePermissionProfile`] (approval policy + fs access + network
/// policy + executor mode) at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPreset {
    /// 默认权限: ask for external file edits and internet access.
    #[default]
    Default,
    /// 自动审查: only prompt on detected-risk operations; still sandboxed.
    AutoReview,
    /// 完全访问权限: unrestricted file/network/execution, no Sandboxie.
    FullAccess,
}

impl PermissionPreset {
    pub const fn label(&self) -> &'static str {
        match self {
            PermissionPreset::Default => "default",
            PermissionPreset::AutoReview => "auto_review",
            PermissionPreset::FullAccess => "full_access",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Self::Default),
            "auto_review" => Some(Self::AutoReview),
            "full_access" => Some(Self::FullAccess),
            _ => None,
        }
    }
}

/// Controls which permission options are visible in the Composer dropdown.
/// The GeneralSettings toggles write this; Composer reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPresetVisibility {
    #[serde(default = "default_true")]
    pub default_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_review_enabled: bool,
    #[serde(default = "default_true")]
    pub full_access_enabled: bool,
}

impl Default for PermissionPresetVisibility {
    fn default() -> Self {
        Self {
            default_enabled: true,
            auto_review_enabled: true,
            full_access_enabled: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// How local shell commands are executed at the OS level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalExecutionMode {
    /// Commands run inside Sandboxie-Plus.
    SandboxiePreferred,
    /// Commands run directly on the system (no sandbox).
    Direct,
}

/// The effective runtime policy derived from a [`PermissionPreset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectivePermissionProfile {
    pub approval_policy: ApprovalPolicy,
    pub sandbox_mode: SandboxMode,
    pub local_execution_mode: LocalExecutionMode,
    pub network_always_ask: bool,
}

impl PermissionPreset {
    pub fn to_effective_profile(&self) -> EffectivePermissionProfile {
        match self {
            PermissionPreset::Default => EffectivePermissionProfile {
                approval_policy: ApprovalPolicy::AlwaysAsk,
                sandbox_mode: SandboxMode::WorkspaceWrite,
                local_execution_mode: LocalExecutionMode::SandboxiePreferred,
                network_always_ask: true,
            },
            PermissionPreset::AutoReview => EffectivePermissionProfile {
                approval_policy: ApprovalPolicy::AutoReview,
                sandbox_mode: SandboxMode::FullAccess,
                local_execution_mode: LocalExecutionMode::SandboxiePreferred,
                network_always_ask: false,
            },
            PermissionPreset::FullAccess => EffectivePermissionProfile {
                approval_policy: ApprovalPolicy::FullAccess,
                sandbox_mode: SandboxMode::FullAccess,
                local_execution_mode: LocalExecutionMode::Direct,
                network_always_ask: false,
            },
        }
    }
}

/// Maximum filesystem boundary for tools. This is separate from
/// [`ApprovalPolicy`]: sandbox mode decides what the system may touch at all,
/// while approval policy decides whether risky calls need a human decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// Tools may read project files but cannot write files.
    ReadOnly,
    /// Tools may read and write inside the active project only.
    #[default]
    WorkspaceWrite,
    /// Tools may read and write outside the active project as well.
    FullAccess,
}

impl SandboxMode {
    /// Stable label.
    pub const fn label(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read_only",
            SandboxMode::WorkspaceWrite => "workspace_write",
            SandboxMode::FullAccess => "full_access",
        }
    }
}

/// Post-edit verification policy (Phase 4C of coding-amplifier spec).
///
/// Decides what happens when [`crate::verification_dispatcher`] reports a
/// failure on a write/edit tool result:
///
/// - [`VerificationPolicy::Disabled`] — verification doesn't run at all
///   (zero overhead path, fully backward-compatible with pre-Phase-4 behavior).
/// - [`VerificationPolicy::Enabled`] — default — verification runs and a
///   `<system-reminder>` describes the outcome, but `ok` stays as the tool
///   reported it. The model sees the failure and *may* choose to fix it.
/// - [`VerificationPolicy::Strict`] — verification failure flips the tool
///   result's `ok` to `false`, which triggers the runtime's reflection /
///   recovery path so the next THINK step is forced to address the failure.
///   `TimedOut` and `Skipped` outcomes do NOT flip `ok` (conservative: only
///   confirmed failures are escalated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationPolicy {
    /// Verification is off (legacy / opt-out).
    Disabled,
    /// Verification runs and surfaces a reminder; `ok` is preserved.
    #[default]
    Enabled,
    /// Verification runs and a confirmed failure flips `ok` to `false`.
    Strict,
}

impl VerificationPolicy {
    /// Stable label.
    pub const fn label(&self) -> &'static str {
        match self {
            VerificationPolicy::Disabled => "disabled",
            VerificationPolicy::Enabled => "enabled",
            VerificationPolicy::Strict => "strict",
        }
    }

    /// Parse from a wire string (UI / Tauri command argument).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "disabled" => Some(Self::Disabled),
            "enabled" => Some(Self::Enabled),
            "strict" => Some(Self::Strict),
            _ => None,
        }
    }

    /// Whether this policy permits the verifier to flip `ok = false` on
    /// confirmed failures.
    pub fn flips_ok_on_failure(&self) -> bool {
        matches!(self, VerificationPolicy::Strict)
    }

    /// Whether this policy runs the verifier at all.
    pub fn is_enabled(&self) -> bool {
        !matches!(self, VerificationPolicy::Disabled)
    }
}

/// Which shell the desktop-integrated terminal should launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalShell {
    #[default]
    PowerShell,
    CommandPrompt,
    GitBash,
    Wsl,
}

impl TerminalShell {
    /// Stable wire label.
    pub const fn label(&self) -> &'static str {
        match self {
            TerminalShell::PowerShell => "powershell",
            TerminalShell::CommandPrompt => "command_prompt",
            TerminalShell::GitBash => "git_bash",
            TerminalShell::Wsl => "wsl",
        }
    }

    /// Parse from a wire string.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "powershell" => Some(Self::PowerShell),
            "command_prompt" | "commandprompt" | "cmd" => Some(Self::CommandPrompt),
            "git_bash" | "gitbash" => Some(Self::GitBash),
            "wsl" => Some(Self::Wsl),
            _ => None,
        }
    }
}

/// Legacy DeepSeek Responses overrides retained for existing settings documents.
/// Only explicitly supported fields are read by the model runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponsesApiSettings {
    /// Saved creativity control; `None` leaves the provider/runtime default.
    #[serde(default)]
    pub creativity: Option<u8>,
    /// Application-level scene preset. This is never accepted from developer
    /// JSON because it is a product policy, not an upstream Responses field.
    #[serde(default)]
    pub scene: Option<ResponseScenePreset>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub top_logprobs: Option<u8>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub text: Option<serde_json::Value>,
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(default)]
    pub user: Option<String>,
    /// Saved overrides from the former developer settings editor.
    #[serde(default)]
    pub developer: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub ineffective: serde_json::Map<String, serde_json::Value>,
}

impl ResponsesApiSettings {
    pub fn effective_temperature(&self) -> Option<f32> {
        self.developer_number("temperature")
            .or(self.temperature)
            .or_else(|| self.scene.map(ResponseScenePreset::temperature))
            .or_else(|| self.creativity.map(|value| f32::from(value) / 50.0))
    }

    pub fn effective_top_p(&self) -> Option<f32> {
        self.developer_number("top_p").or(self.top_p)
    }

    pub fn effective_max_output_tokens(&self) -> Option<u32> {
        self.developer
            .get("max_output_tokens")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .or(self.max_output_tokens)
    }

    pub fn effective_top_logprobs(&self) -> Option<u8> {
        self.developer
            .get("top_logprobs")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .or(self.top_logprobs)
    }

    pub fn effective_text(&self) -> Option<serde_json::Value> {
        self.developer
            .get("text")
            .cloned()
            .or_else(|| self.text.clone())
    }

    pub fn effective_tool_choice(&self) -> Option<serde_json::Value> {
        self.developer
            .get("tool_choice")
            .cloned()
            .or_else(|| self.tool_choice.clone())
    }

    pub fn effective_user(&self) -> Option<String> {
        self.developer
            .get("user")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| self.user.clone())
    }

    fn developer_number(&self, key: &str) -> Option<f32> {
        self.developer
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .map(|value| value as f32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseScenePreset {
    Code,
    Email,
    Analysis,
    Creative,
}

impl ResponseScenePreset {
    pub const fn temperature(self) -> f32 {
        // No upstream counterpart (checked: DeepSeek Responses docs / Codex):
        // application-level scene preset, chosen to preserve current behavior.
        match self {
            Self::Code => 0.4,
            Self::Email => 0.8,
            Self::Analysis => 0.5,
            Self::Creative => 1.5,
        }
    }
}

/// Persisted, **non-secret** application settings (safe to store on disk).
/// The API key is intentionally absent — it lives in the [`SecretStore`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// How tool approvals are resolved.
    #[serde(default)]
    pub approval_policy: ApprovalPolicy,
    /// Maximum filesystem boundary for tool execution.
    #[serde(default)]
    pub sandbox_mode: SandboxMode,
    /// Preferred shell for the desktop integrated terminal.
    #[serde(default)]
    pub terminal_shell: TerminalShell,
    /// Declarative permission rules (allow/ask/deny patterns).
    #[serde(default)]
    pub permission_rules: PermissionRules,
    /// Settings for the desktop workspace browser.
    #[serde(default)]
    pub browser: BrowserSettings,
    /// Declarative external hooks (`hooks.json` source). Stored verbatim so the
    /// UI round-trips exactly what the user typed; parsed lazily when assembling
    /// the runtime hook registry.
    #[serde(default)]
    pub hooks_json: String,
    /// Global Responses API controls; system-managed request fields are never
    /// accepted from this object.
    #[serde(default)]
    pub responses: ResponsesApiSettings,
    /// Post-edit verification policy (Phase 4C of coding-amplifier spec).
    /// Controls whether failed verifications stay informative reminders or
    /// flip the tool result's `ok` flag to drive automatic retry.
    #[serde(default)]
    pub verification_policy: VerificationPolicy,
    /// Web search provider settings. Defaults to DeepSeek-first.
    #[serde(default)]
    pub web_search: WebSearchSettings,
    /// Tool-search / lazy-tool-loading mode (tool-search spec).
    /// Decides whether deferrable tools (MCP + `should_defer == true`) are
    /// hidden from each request's `tools` array until discovered through the
    /// `tool_search` built-in. New settings default to `Auto` so large
    /// deferred toolsets do not bloat every model request.
    #[serde(default)]
    pub tool_search_mode: deepagent_builtins::ToolSearchMode,
    /// Auto-mode threshold in characters: total deferred-tool schema size at
    /// or above which `Auto` mode flips on. `None` falls back to the
    /// dispatcher's hard-coded default (8000). Persisted as an `Option` so
    /// older settings docs without this field round-trip cleanly.
    #[serde(default)]
    pub tool_search_auto_threshold_chars: Option<usize>,
    /// Master switch for skill-catalog `<available-skills>` reminder injection
    /// (channel A of the auto-activation design). When `false`, the chat
    /// service skips the reminder entirely (saves tokens for users who run
    /// the assistant as a pure chatbot). The `skill` tool itself remains
    /// callable. Default `true`. `#[serde(default)]` falls back to
    /// [`default_skill_catalog_enabled`] for older settings docs without
    /// the field.
    #[serde(default = "default_skill_catalog_enabled")]
    pub skill_catalog_enabled: bool,
    /// Character budget for the rendered `<available-skills>` reminder
    /// block. The catalog renderer truncates non-builtin descriptions when
    /// the total exceeds this budget. Default `8000`. A value of `0` is
    /// treated as disabled (consumer side; see R10.5).
    #[serde(default = "default_skill_catalog_char_budget")]
    pub skill_catalog_char_budget: usize,
    /// Whether the `SkillInstallDialog` runs the streaming AI security
    /// review before allowing the user to confirm install. When `false`,
    /// only the static scan report is shown and the install button is not
    /// gated on review verdict. Default `true`.
    #[serde(default = "default_skill_install_ai_review_enabled")]
    pub skill_install_ai_review_enabled: bool,
    /// Optional model override for the AI security review. `None` (the
    /// default) means "use the currently selected chat model".
    #[serde(default)]
    pub skill_install_ai_review_model: Option<String>,
    /// The user-selected permission preset (governs the full runtime policy).
    #[serde(default)]
    pub active_permission_preset: PermissionPreset,
    /// Which permission presets are visible in the Composer dropdown.
    #[serde(default)]
    pub permission_preset_visibility: PermissionPresetVisibility,
    /// User-defined name displayed in the onboarding welcome message.
    #[serde(default)]
    pub welcome_name: String,
    /// Optional override for the proactive auto-compact reserve buffer (the
    /// tokens subtracted from the effective context window to form the
    /// threshold). `None` uses the Harness default (65_536; see
    /// `ContextPolicy::autocompact_threshold_tokens`). Persisted as `Option`
    /// so older settings docs round-trip cleanly.
    #[serde(default)]
    pub autocompact_reserve_tokens: Option<usize>,
    /// Context pressure headroom; overrides the legacy reserve field.
    #[serde(default)]
    pub context_headroom_tokens: Option<usize>,
    /// Built-in system-prompt output style (Claude Code output styles §7.1).
    /// `Default` injects nothing; `Explanatory`/`Learning` add a stable,
    /// DeepSeek-worded style block. `#[serde(default)]` → `Default` for older
    /// settings docs without the field.
    /// User's preferred output language (e.g., "chinese", "english", "japanese").
    /// When set, overrides the model's auto-detection and pins the reply language
    /// for the entire session. When empty/unset, the model detects from the user's
    /// first substantial message. `#[serde(default)]` → empty string for older
    /// settings docs without the field.
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub output_style: OutputStyle,
    /// Opt-in advanced execution safeguards (§2.2/§2.3/§6.1/§6.2). Each is a
    /// self-created "从宽" guard (advisory/fail-open) that is OFF by default;
    /// this is the user-facing surface so they can be toggled without setting
    /// environment variables before launch. The matching `DEEPAGENT_*` env
    /// vars still force-enable regardless (power-user / CI override).
    #[serde(default)]
    pub execution_features: ExecutionFeatures,
    /// Official DeepSeek pricing catalog (RMB per million tokens). Refreshed
    /// at startup and when the user manually refreshes models. `#[serde(default)]`
    /// ensures old settings without this field deserialize cleanly.
    #[serde(default)]
    pub pricing_catalog: PricingCatalog,
}

/// Desktop browser controls. Screenshots are explicit tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BrowserSettings {
    pub enabled: bool,
    pub screenshots_enabled: bool,
    pub require_approval: bool,
    pub blocked_domains: Vec<String>,
    pub allowed_domains: Vec<String>,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            screenshots_enabled: true,
            require_approval: false,
            blocked_domains: Vec::new(),
            allowed_domains: Vec::new(),
        }
    }
}

pub const WORKSPACE_BROWSER_TOOL: &str = "workspace_browser";

impl Default for AppSettings {
    fn default() -> Self {
        let mut settings: Self =
            serde_json::from_value(serde_json::json!({})).expect("application defaults are valid");
        settings.pricing_catalog = SettingsService::pricing_snapshot();
        settings.tool_search_mode = SettingsService::DEFAULT_TOOL_SEARCH_MODE;
        settings
    }
}

/// DeepSeek official pricing catalog (idle-tier rates, RMB per million tokens).
///
/// Persisted alongside model discovery so offline runs can bill accurately.
/// First-stage implementation uses hardcoded official rates (verified 2026-10-02);
/// future versions may fetch dynamically if DeepSeek provides a pricing API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PricingCatalog {
    /// Pricing by model ID (e.g. "deepseek-flash", "deepseek-v4-pro").
    #[serde(default)]
    pub models: HashMap<String, ModelPricing>,
    /// When this catalog was fetched (Unix ms). 0 means never refreshed.
    #[serde(default)]
    pub fetched_at: i64,
    /// Source URL (for audit trail).
    #[serde(default)]
    pub source_url: String,
}

/// Opt-in advanced execution safeguards, all default OFF (自创机制默认从宽:
/// off unless the user asks for them). Persisted so the desktop UI can toggle
/// them; the corresponding `DEEPAGENT_*` env var force-enables each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ExecutionFeatures {
    /// §2.3 stall/laziness detector: audit a final answer for false completion
    /// and inject one advisory nudge (`DEEPAGENT_STALL_DETECTOR`).
    #[serde(default)]
    pub stall_detector: bool,
    /// §6.1 LLM command-injection guard: second-opinion classify structurally
    /// suspicious shell commands (`DEEPAGENT_LLM_COMMAND_GUARD`).
    #[serde(default)]
    pub command_guard: bool,
    /// §6.2 per-project trust gate: untrusted project dirs escalate bash to
    /// approval (`DEEPAGENT_PROJECT_TRUST`).
    #[serde(default)]
    pub project_trust: bool,
    /// §2.2 adversarial goal verifier: a skeptic panel audits goal coverage
    /// after the fact-gate (`DEEPAGENT_ADVERSARIAL_VERIFY`).
    #[serde(default)]
    pub adversarial_verify: bool,
}

/// Built-in system-prompt output style (Claude Code output styles §7.1).
/// Structure aligns with CC's default/Explanatory/Learning set; the injected
/// wording is DeepSeek-native (未照搬 Claude 措辞).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStyle {
    /// No extra style guidance — the base system prompt governs tone.
    #[default]
    Default,
    /// Add brief “why” insights alongside the work, without slowing delivery.
    Explanatory,
    /// Teaching mode: explain concepts and reasoning as you go, for learning.
    Learning,
}

impl OutputStyle {
    /// Stable snake_case label for settings/UI.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputStyle::Default => "default",
            OutputStyle::Explanatory => "explanatory",
            OutputStyle::Learning => "learning",
        }
    }

    /// Parse a snake_case label (from the UI / env). Returns `None` for an
    /// unknown value so the caller can reject it rather than silently reset.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" => Some(OutputStyle::Default),
            "explanatory" => Some(OutputStyle::Explanatory),
            "learning" => Some(OutputStyle::Learning),
            _ => None,
        }
    }
}

fn default_skill_catalog_enabled() -> bool {
    true
}

fn default_skill_catalog_char_budget() -> usize {
    8_000
}

fn default_skill_install_ai_review_enabled() -> bool {
    true
}

/// A redacted view of settings safe to send to the UI (no secret material).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsView {
    /// Whether the selected registry route is active and has a credential.
    pub configured: bool,
    /// Current approval policy label (always_ask / auto_review / full_access).
    pub approval_policy: String,
    /// Current sandbox mode label (read_only / workspace_write / full_access).
    pub sandbox_mode: String,
    /// Current preferred integrated-terminal shell.
    pub terminal_shell: String,
    /// Current canonical DeepSeek Responses reasoning effort.
    pub reasoning_effort: String,
    /// Current web-search settings.
    pub web_search: WebSearchSettings,
    /// Opt-in advanced execution safeguards (§2.2/§2.3/§6.1/§6.2).
    #[serde(default)]
    pub execution_features: ExecutionFeatures,
    /// Selected built-in output style (§7.1): `default`/`explanatory`/`learning`.
    #[serde(default)]
    pub output_style: String,
    /// Pricing catalog dynamically fetched from official sources.
    #[serde(default)]
    pub pricing_catalog: PricingCatalog,
}

/// General settings share the existing database and provider service.
pub struct SettingsService {
    db: Arc<Database>,
    transport: Arc<dyn HttpTransport>,
    secrets: Arc<dyn SecretStore>,
    settings_cache: Mutex<Option<(String, AppSettings)>>,
}

/// The command prefixes `rules` allows, minus any fenced by an `ask`/`deny` rule.
fn granted_prefixes(rules: &PermissionRules) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pattern in &rules.allow {
        let Some(prefix) = rule_command_prefix(pattern) else {
            continue;
        };
        // A bare `bash` allow rule is not a prefix grant; it claims everything.
        if prefix.is_empty() || fencing_rule(rules, prefix).is_some() {
            continue;
        }
        let prefix = normalize_prefix(prefix);
        if !out.contains(&prefix) {
            out.push(prefix);
        }
    }
    out
}

/// The `ask`/`deny` rule that already fences `prefix`. A bare `bash` rule fences
/// every command; `Bash(rm:*)` fences `rm -rf` but not `git status`.
fn fencing_rule<'a>(rules: &'a PermissionRules, prefix: &str) -> Option<&'a String> {
    rules
        .ask
        .iter()
        .chain(rules.deny.iter())
        .find(|pattern| match rule_command_prefix(pattern) {
            None => false,
            Some("") => true,
            Some(fenced) => prefix_covers(fenced, prefix),
        })
}

/// Collapse whitespace so `git  status` and `git status` are the same rule.
fn normalize_prefix(prefix: &str) -> String {
    prefix.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn migrate_persisted_reasoning(value: &mut serde_json::Value) -> usize {
    let Some(settings) = value.as_object_mut() else {
        return 0;
    };
    let mut changes = 0;

    let Some(responses) = settings
        .get_mut("responses")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return changes;
    };
    if let Some(effort) = responses.get("reasoning_effort") {
        if let Some(value) = effort.as_str() {
            if let Some(canonical) = ReasoningEffort::parse_responses(value) {
                if value != canonical.label() {
                    responses.insert(
                        "reasoning_effort".to_string(),
                        serde_json::Value::String(canonical.label().to_string()),
                    );
                    changes += 1;
                }
            } else {
                responses.remove("reasoning_effort");
                changes += 1;
            }
        }
    }

    if let Some(developer) = responses
        .get_mut("developer")
        .and_then(serde_json::Value::as_object_mut)
    {
        let effort = developer
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("effort"))
            .and_then(serde_json::Value::as_str);
        if let Some(value) = effort {
            if let Some(canonical) = ReasoningEffort::parse_responses(value) {
                if value != canonical.label() {
                    if let Some(reasoning) = developer
                        .get_mut("reasoning")
                        .and_then(serde_json::Value::as_object_mut)
                    {
                        reasoning.insert(
                            "effort".to_string(),
                            serde_json::Value::String(canonical.label().to_string()),
                        );
                        changes += 1;
                    }
                }
            } else {
                developer.remove("reasoning");
                changes += 1;
            }
        }
    }
    changes
}

impl SettingsService {
    /// The one durable image-reference cache attached to the existing SQLite
    /// authority. It stores only Digests and provider file metadata.
    pub(crate) fn image_file_cache(
        &self,
    ) -> deepagent_models::file_references::DeepSeekImageFileCache {
        deepagent_models::file_references::DeepSeekImageFileCache::new(Arc::new(
            crate::deepseek_file_store::SqliteImageFileReferenceStore::new(self.db.clone()),
        ))
    }

    /// Build the service from a shared database, an HTTP transport (reaches
    /// DeepSeek for discovery), and a secret store (holds the API key).
    pub fn new(
        db: Arc<Database>,
        transport: Arc<dyn HttpTransport>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            db,
            transport,
            secrets,
            settings_cache: Mutex::new(None),
        }
    }

    /// Refresh official DeepSeek pricing and persist to settings.
    ///
    /// First-stage implementation: returns hardcoded idle-tier rates verified
    /// from https://api-docs.deepseek.com/zh-cn/quick_start/pricing (2026-10-02).
    /// Future versions may fetch dynamically if DeepSeek provides a pricing API.
    pub async fn refresh_pricing(&self) -> Result<PricingCatalog> {
        let pricing = Self::pricing_snapshot();
        let settings = self.update_settings(|settings| {
            settings.pricing_catalog = pricing.clone();
            Ok(())
        })?;
        Ok(settings.pricing_catalog)
    }

    /// Local pricing snapshot; persistence belongs to the settings commit owner.
    fn pricing_snapshot() -> PricingCatalog {
        // Official DeepSeek pricing (idle-tier, RMB per million tokens).
        // Source: https://api-docs.deepseek.com/zh-cn/quick_start/pricing
        // Verified: 2026-10-02
        let mut models = HashMap::new();

        // deepseek-flash (DeepSeek-V4.1-Flash)
        // Idle: cache_hit=0.02, cache_miss=1.0, output=4.0
        models.insert(
            "deepseek-flash".to_string(),
            ModelPricing {
                input_cache_hit_per_million: 0.02,
                input_cache_miss_per_million: 1.0,
                output_per_million: 4.0,
            },
        );

        // Legacy alias: deepseek-v4-flash routes to V4.1-Flash (same pricing)
        models.insert(
            "deepseek-v4-flash".to_string(),
            ModelPricing {
                input_cache_hit_per_million: 0.02,
                input_cache_miss_per_million: 1.0,
                output_per_million: 4.0,
            },
        );

        // deepseek-v4-pro (DeepSeek-V4-Pro-0813)
        // Idle: cache_hit=0.15, cache_miss=4.5, output=13.5
        models.insert(
            "deepseek-v4-pro".to_string(),
            ModelPricing {
                input_cache_hit_per_million: 0.15,
                input_cache_miss_per_million: 4.5,
                output_per_million: 13.5,
            },
        );

        PricingCatalog {
            models,
            fetched_at: now_ms(),
            source_url: "https://api-docs.deepseek.com/zh-cn/quick_start/pricing".to_string(),
        }
    }

    /// Load general settings, creating only application defaults when absent.
    /// This never discovers, activates, imports or selects a model.
    pub fn load(&self) -> Result<Option<AppSettings>> {
        match self.load_current_settings()? {
            Some(settings) => Ok(Some(settings)),
            None => self
                .edit_settings(|prior| Ok(prior.unwrap_or_default()))
                .map(Some),
        }
    }

    /// Read the persisted welcome name. An empty value means the UI should use
    /// its localized default name.
    pub fn welcome_name(&self) -> Result<String> {
        Ok(self
            .load()?
            .map(|settings| settings.welcome_name)
            .unwrap_or_default())
    }

    /// Persist the welcome name in the shared SQLite-backed settings document.
    pub fn set_welcome_name(&self, name: &str) -> Result<String> {
        let name = name.trim();
        if name.chars().count() > 32 {
            return Err(CoreError::invalid(
                "welcome name must be at most 32 characters",
            ));
        }

        let settings = self.update_settings(|settings| {
            settings.welcome_name = name.to_string();
            Ok(())
        })?;
        Ok(settings.welcome_name)
    }

    pub fn browser_settings(&self) -> Result<BrowserSettings> {
        Ok(self
            .load_current_settings()?
            .map(|s| s.browser)
            .unwrap_or_default())
    }

    pub fn set_browser_settings(&self, browser: BrowserSettings) -> Result<BrowserSettings> {
        Ok(self
            .update_settings(|settings| {
                settings.browser = browser.clone();
                Ok(())
            })?
            .browser)
    }

    /// The current approval policy (defaults to [`ApprovalPolicy::AlwaysAsk`]
    /// when uninitialized).
    pub fn approval_policy(&self) -> Result<ApprovalPolicy> {
        Ok(self.load()?.map(|s| s.approval_policy).unwrap_or_default())
    }

    /// The current sandbox mode.
    pub fn sandbox_mode(&self) -> Result<SandboxMode> {
        Ok(self.load()?.map(|s| s.sandbox_mode).unwrap_or_default())
    }

    /// The preferred integrated-terminal shell.
    pub fn terminal_shell(&self) -> Result<TerminalShell> {
        Ok(self.load()?.map(|s| s.terminal_shell).unwrap_or_default())
    }

    /// The current route's effective user-controlled thinking level.
    pub fn reasoning_effort(&self) -> Result<ReasoningEffort> {
        let Some(settings) = self.load()? else {
            return Ok(ReasoningEffort::default());
        };
        Ok(
            crate::model_runtime::resolve_run_model(&self.db, &settings, None, None, None)?
                .reasoning_effort
                .unwrap_or_default(),
        )
    }

    /// Effective proactive auto-compact reserve buffer override. Precedence:
    /// new headroom env/setting > legacy reserve env/setting > Harness default.
    pub fn autocompact_reserve_tokens(&self) -> Option<usize> {
        if let Ok(raw) = std::env::var("DEEPAGENT_CONTEXT_HEADROOM_TOKENS") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                return Some(parsed);
            }
        }
        let settings = self.load().ok().flatten();
        if let Some(headroom) = settings.as_ref().and_then(|s| s.context_headroom_tokens) {
            return Some(headroom);
        }
        if let Ok(raw) = std::env::var("DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                return Some(parsed);
            }
        }
        settings.and_then(|s| s.autocompact_reserve_tokens)
    }

    /// The selected built-in output style. `DEEPAGENT_OUTPUT_STYLE`
    /// (`explanatory`/`learning`/`default`) overrides the persisted setting;
    /// otherwise the persisted value, defaulting to `Default`.
    pub fn output_style(&self) -> OutputStyle {
        if let Ok(raw) = std::env::var("DEEPAGENT_OUTPUT_STYLE") {
            match raw.trim().to_ascii_lowercase().as_str() {
                "explanatory" => return OutputStyle::Explanatory,
                "learning" => return OutputStyle::Learning,
                "default" => return OutputStyle::Default,
                _ => {}
            }
        }
        self.load()
            .ok()
            .flatten()
            .map(|s| s.output_style)
            .unwrap_or_default()
    }

    /// Persist the selected built-in output style (§7.1). Takes effect on the
    /// next run: `output_style()` → `output_style_prompt_block()` injects the
    /// matching (or no) style block into the cacheable system prefix.
    pub fn set_output_style(&self, style: OutputStyle) -> Result<()> {
        self.update_settings(|settings| {
            settings.output_style = style;
            Ok(())
        })?;
        Ok(())
    }

    /// The current post-edit verification policy.
    pub fn verification_policy(&self) -> Result<VerificationPolicy> {
        Ok(self
            .load()?
            .map(|s| s.verification_policy)
            .unwrap_or_default())
    }

    /// The persisted opt-in execution safeguards (default: all off). Env
    /// overrides are applied by the per-feature enable checks, not here — this
    /// returns exactly what the user persisted so the UI round-trips cleanly.
    pub fn execution_features(&self) -> ExecutionFeatures {
        self.load()
            .ok()
            .flatten()
            .map(|s| s.execution_features)
            .unwrap_or_default()
    }

    /// Persist the opt-in execution safeguards.
    pub fn set_execution_features(&self, features: ExecutionFeatures) -> Result<()> {
        self.update_settings(|settings| {
            settings.execution_features = features;
            Ok(())
        })?;
        Ok(())
    }

    /// Set the post-edit verification policy, persisting it.
    pub fn set_verification_policy(&self, policy: VerificationPolicy) -> Result<()> {
        self.update_settings(|settings| {
            settings.verification_policy = policy;
            Ok(())
        })?;
        Ok(())
    }

    /// Current versioned engine settings; legacy documents are decoded compatibly.
    pub fn web_search_settings(&self) -> Result<WebSearchSettings> {
        Ok(normalized_web_search_settings(
            self.load()?
                .map(|settings| settings.web_search)
                .unwrap_or_default(),
        ))
    }

    /// Redacted readiness view; ready means configured, not reachable.
    pub fn web_search_settings_view(&self) -> Result<WebSearchSettingsView> {
        let settings = self.web_search_settings()?;
        let deepseek_ready = self
            .deepseek_search_api_key()?
            .is_some_and(|key| !key.trim().is_empty());
        let anysearch_ready = self
            .anysearch_api_key()?
            .is_some_and(|key| !key.trim().is_empty());
        let engine_statuses = settings
            .engines
            .iter()
            .map(|engine| WebSearchEngineStatus {
                id: engine.id,
                state: if !engine.enabled {
                    WebSearchConfigState::Disabled
                } else {
                    match engine.id {
                        WebSearchEngineId::Deepseek if !deepseek_ready => {
                            WebSearchConfigState::NeedsConfiguration
                        }
                        WebSearchEngineId::Anysearch if !anysearch_ready => {
                            WebSearchConfigState::NeedsConfiguration
                        }
                        WebSearchEngineId::Searxng if engine.base_url.is_none() => {
                            WebSearchConfigState::NeedsConfiguration
                        }
                        _ => WebSearchConfigState::Ready,
                    }
                },
            })
            .collect();
        Ok(WebSearchSettingsView {
            settings,
            engine_statuses,
        })
    }

    /// Capture one call-local config/credential snapshot. It is never cached.
    pub fn web_search_runtime_config(&self) -> Result<WebSearchRuntimeConfig> {
        let settings = self.web_search_settings()?;
        let deepseek_api_key = settings
            .engines
            .iter()
            .any(|engine| engine.enabled && engine.id == WebSearchEngineId::Deepseek)
            .then(|| self.deepseek_search_api_key())
            .transpose()?
            .flatten();
        let anysearch_api_key = settings
            .engines
            .iter()
            .any(|engine| engine.enabled && engine.id == WebSearchEngineId::Anysearch)
            .then(|| self.anysearch_api_key())
            .transpose()?
            .flatten();
        Ok(WebSearchRuntimeConfig {
            settings,
            deepseek_api_key,
            anysearch_api_key,
        })
    }

    /// Persist one normalized, versioned engine list and priority order atomically.
    pub fn set_web_search_settings(&self, mut settings_next: WebSearchSettings) -> Result<()> {
        settings_next.normalize();
        settings_next.validate()?;
        self.update_settings(|settings| {
            settings.web_search = settings_next.clone();
            Ok(())
        })?;
        Ok(())
    }

    /// The configured AnySearch API key, if present.
    pub fn anysearch_api_key(&self) -> Result<Option<String>> {
        self.secrets.get(ANYSEARCH_API_KEY_NAME)
    }

    /// Save the AnySearch API key to the secret store.
    pub fn set_anysearch_api_key(&self, api_key: &str) -> Result<()> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(CoreError::invalid("AnySearch API key must not be empty"));
        }
        self.secrets.set(ANYSEARCH_API_KEY_NAME, api_key)
    }

    /// Delete the configured AnySearch API key.
    pub fn clear_anysearch_api_key(&self) -> Result<()> {
        let _ = self.secrets.delete(ANYSEARCH_API_KEY_NAME);
        Ok(())
    }

    /// Performance-oriented default for new settings documents: use Auto so
    /// large MCP/deferred tool schema sets are discovered lazily, while small
    /// toolsets remain byte-equivalent to fully loaded requests.
    pub const DEFAULT_TOOL_SEARCH_MODE: deepagent_builtins::ToolSearchMode =
        deepagent_builtins::ToolSearchMode::Auto;

    /// The current tool-search mode (default `Auto` for missing/new settings).
    pub fn tool_search_mode(&self) -> Result<deepagent_builtins::ToolSearchMode> {
        Ok(self
            .load()?
            .map(|s| s.tool_search_mode)
            .unwrap_or(Self::DEFAULT_TOOL_SEARCH_MODE))
    }

    /// Set the tool-search mode, persisting it.
    pub fn set_tool_search_mode(&self, mode: deepagent_builtins::ToolSearchMode) -> Result<()> {
        self.update_settings(|settings| {
            settings.tool_search_mode = mode;
            Ok(())
        })?;
        Ok(())
    }

    /// Hard-coded fallback threshold (in characters) used when the user
    /// hasn't customized the value. Mirrors the constant the chat_service
    /// dispatcher applies — exposed here so the desktop UI can show the
    /// effective default in placeholder text.
    pub const DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS: usize = 8_000;

    /// The Auto-mode threshold currently in effect. Returns the persisted
    /// value when set, otherwise [`Self::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS`].
    pub fn tool_search_auto_threshold(&self) -> Result<usize> {
        Ok(self
            .load()?
            .and_then(|s| s.tool_search_auto_threshold_chars)
            .unwrap_or(Self::DEFAULT_TOOL_SEARCH_AUTO_THRESHOLD_CHARS))
    }

    /// Persist a new Auto-mode threshold. `None` reverts to the default.
    /// Values below 1 are clamped to 1 (zero would mean "Auto always
    /// activates", which is what `Enabled` already does).
    pub fn set_tool_search_auto_threshold(&self, value: Option<usize>) -> Result<()> {
        self.update_settings(|settings| {
            settings.tool_search_auto_threshold_chars = value.map(|v| v.max(1));
            Ok(())
        })?;
        Ok(())
    }

    // ---- Skill marketplace settings (Skill Marketplace spec, R10) ---------

    /// Default character budget for the `<available-skills>` reminder block.
    /// Mirrors the [`default_skill_catalog_char_budget`] free function so
    /// callers (UI placeholder text, integration tests) can refer to it via
    /// the service surface without poking at private items.
    pub const DEFAULT_SKILL_CATALOG_CHAR_BUDGET: usize = 8_000;

    /// Whether the skill-catalog reminder (channel A) is enabled. Default
    /// `true` when uninitialized.
    pub fn skill_catalog_enabled(&self) -> Result<bool> {
        Ok(self
            .load()?
            .map(|s| s.skill_catalog_enabled)
            .unwrap_or_else(default_skill_catalog_enabled))
    }

    /// Persist the master switch for the skill-catalog reminder.
    pub fn set_skill_catalog_enabled(&self, enabled: bool) -> Result<()> {
        self.update_settings(|settings| {
            settings.skill_catalog_enabled = enabled;
            Ok(())
        })?;
        Ok(())
    }

    /// Character budget for the rendered `<available-skills>` reminder.
    /// Returns the persisted value (default 8000) when uninitialized.
    pub fn skill_catalog_char_budget(&self) -> Result<usize> {
        Ok(self
            .load()?
            .map(|s| s.skill_catalog_char_budget)
            .unwrap_or_else(default_skill_catalog_char_budget))
    }

    /// Persist the catalog character budget. The value is stored as-is;
    /// consumers are expected to honor R10.5 (treat `0` as disabled) at the
    /// rendering site, not here.
    pub fn set_skill_catalog_char_budget(&self, budget: usize) -> Result<()> {
        self.update_settings(|settings| {
            settings.skill_catalog_char_budget = budget;
            Ok(())
        })?;
        Ok(())
    }

    /// Whether the `SkillInstallDialog` runs the AI security review before
    /// allowing install confirmation. Default `true` when uninitialized.
    pub fn skill_install_ai_review_enabled(&self) -> Result<bool> {
        Ok(self
            .load()?
            .map(|s| s.skill_install_ai_review_enabled)
            .unwrap_or_else(default_skill_install_ai_review_enabled))
    }

    /// Persist the AI-review master switch.
    pub fn set_skill_install_ai_review_enabled(&self, enabled: bool) -> Result<()> {
        self.update_settings(|settings| {
            settings.skill_install_ai_review_enabled = enabled;
            Ok(())
        })?;
        Ok(())
    }

    /// Optional model override for the AI security review. `None` means
    /// "use the currently selected chat model".
    pub fn skill_install_ai_review_model(&self) -> Result<Option<String>> {
        Ok(self.load()?.and_then(|s| s.skill_install_ai_review_model))
    }

    /// Persist the AI-review model override. Pass `None` (or an empty
    /// string, which is normalized to `None`) to fall back to the chat
    /// model.
    pub fn set_skill_install_ai_review_model(&self, model: Option<String>) -> Result<()> {
        self.update_settings(|settings| {
            settings.skill_install_ai_review_model = model
                .clone()
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty());
            Ok(())
        })?;
        Ok(())
    }

    // ---- Permission Preset (Sandboxie integration) --------------------------

    /// The current active permission preset (default `Default`).
    pub fn active_permission_preset(&self) -> Result<PermissionPreset> {
        Ok(self
            .load()?
            .map(|s| s.active_permission_preset)
            .unwrap_or_default())
    }

    /// Set the active permission preset, persisting it. Also syncs the legacy
    /// `approval_policy` and `sandbox_mode` fields for backward compatibility.
    pub fn set_active_permission_preset(&self, preset: PermissionPreset) -> Result<()> {
        self.update_settings(|settings| {
            settings.active_permission_preset = preset;
            let profile = preset.to_effective_profile();
            settings.approval_policy = profile.approval_policy;
            settings.sandbox_mode = profile.sandbox_mode;
            Ok(())
        })?;
        Ok(())
    }

    /// Which permission presets are visible in the Composer dropdown.
    pub fn permission_preset_visibility(&self) -> Result<PermissionPresetVisibility> {
        Ok(self
            .load()?
            .map(|s| s.permission_preset_visibility)
            .unwrap_or_default())
    }

    /// Set which permission presets are visible. At least one must remain
    /// enabled; returns an error if all three are disabled.
    pub fn set_permission_preset_visibility(
        &self,
        visibility: PermissionPresetVisibility,
    ) -> Result<()> {
        if !visibility.default_enabled
            && !visibility.auto_review_enabled
            && !visibility.full_access_enabled
        {
            return Err(CoreError::invalid(
                "at least one permission preset must remain visible",
            ));
        }
        self.update_settings(|settings| {
            // If the current preset is being hidden, fall back to the first visible one.
            let current_hidden = match settings.active_permission_preset {
                PermissionPreset::Default => !visibility.default_enabled,
                PermissionPreset::AutoReview => !visibility.auto_review_enabled,
                PermissionPreset::FullAccess => !visibility.full_access_enabled,
            };
            if current_hidden {
                let fallback = if visibility.default_enabled {
                    PermissionPreset::Default
                } else if visibility.auto_review_enabled {
                    PermissionPreset::AutoReview
                } else {
                    PermissionPreset::FullAccess
                };
                settings.active_permission_preset = fallback;
                let profile = fallback.to_effective_profile();
                settings.approval_policy = profile.approval_policy;
                settings.sandbox_mode = profile.sandbox_mode;
            }
            settings.permission_preset_visibility = visibility.clone();
            Ok(())
        })?;
        Ok(())
    }

    /// Derive the full effective runtime profile from the persisted fields.
    /// Both the preset setter and the legacy setters (set_approval_policy,
    /// set_sandbox_mode) write to the same underlying fields, so reading them
    /// directly is always authoritative.
    pub fn effective_permission_profile(&self) -> Result<EffectivePermissionProfile> {
        // Coupled policy fields must come from one committed document, not
        // separate getters that can straddle another host's preset update.
        let settings = self.load()?;
        Ok(Self::permission_profile_from_settings(settings.as_ref()))
    }

    pub(crate) fn permission_profile_from_settings(
        settings: Option<&AppSettings>,
    ) -> EffectivePermissionProfile {
        let policy = settings
            .as_ref()
            .map(|s| s.approval_policy)
            .unwrap_or_default();
        let sandbox = settings
            .as_ref()
            .map(|s| s.sandbox_mode)
            .unwrap_or_default();
        let local_execution_mode = if matches!(policy, ApprovalPolicy::FullAccess)
            && matches!(sandbox, SandboxMode::FullAccess)
        {
            LocalExecutionMode::Direct
        } else {
            LocalExecutionMode::SandboxiePreferred
        };
        let network_always_ask = matches!(policy, ApprovalPolicy::AlwaysAsk);
        EffectivePermissionProfile {
            approval_policy: policy,
            sandbox_mode: sandbox,
            local_execution_mode,
            network_always_ask,
        }
    }

    /// Set the Thinking Mode depth, persisting it. Returns the redacted view.
    pub fn set_reasoning_effort(&self, effort: ReasoningEffort) -> Result<SettingsView> {
        self.set_reasoning_selection(Some(effort))
    }

    pub fn set_reasoning_selection(&self, effort: Option<ReasoningEffort>) -> Result<SettingsView> {
        if let Some(model) = self.harness_models().selected_model()? {
            self.harness_models().select_model_with_reasoning(
                &model.provider_id,
                &model.model_name,
                effort.map(ReasoningEffort::label),
            )?;
            return self
                .view()?
                .ok_or_else(|| CoreError::invalid("project not initialized"));
        }
        Err(CoreError::invalid(
            "select a configured model before changing reasoning effort",
        ))
    }

    /// Set the approval policy, persisting it. Returns the redacted view.
    pub fn set_approval_policy(&self, policy: ApprovalPolicy) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            settings.approval_policy = policy;
            Ok(())
        })?;
        self.view_from_settings(&settings)
    }

    /// Set the sandbox mode, persisting it. Returns the redacted view.
    pub fn set_sandbox_mode(&self, mode: SandboxMode) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            settings.sandbox_mode = mode;
            Ok(())
        })?;
        self.view_from_settings(&settings)
    }

    /// Set the preferred integrated-terminal shell, persisting it.
    pub fn set_terminal_shell(&self, shell: TerminalShell) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            settings.terminal_shell = shell;
            Ok(())
        })?;
        self.view_from_settings(&settings)
    }

    /// The current declarative permission rules (empty when uninitialized).
    pub fn permission_rules(&self) -> Result<PermissionRules> {
        Ok(self.load()?.map(|s| s.permission_rules).unwrap_or_default())
    }

    /// Set the declarative permission rules, persisting them.
    pub fn set_permission_rules(&self, rules: PermissionRules) -> Result<()> {
        self.update_settings(|settings| {
            settings.permission_rules = rules.clone();
            Ok(())
        })
        .map(|_| ())
    }

    /// Command prefixes the human gave standing approval for, by answering an
    /// approval prompt with the `command_prefix` scope.
    ///
    /// They are stored as `Bash(prefix:*)` entries in [`Self::permission_rules`],
    /// so the same settings list that shows the rules is the place to revoke one,
    /// and a prefix the user fenced with `ask`/`deny` never reads back as a grant.
    pub fn command_prefix_grants(&self) -> Result<Vec<String>> {
        Ok(granted_prefixes(&self.permission_rules()?))
    }

    /// Record `prefix` as a standing allowance.
    ///
    /// Errors when an `ask`/`deny` rule already fences that prefix: an approval
    /// answer must not walk past a rule the user wrote by hand.
    pub fn grant_command_prefix(&self, prefix: &str) -> Result<()> {
        let prefix = normalize_prefix(prefix);
        if prefix.is_empty() {
            return Err(CoreError::invalid(
                "a command prefix grant needs a command prefix",
            ));
        }
        self.update_settings(|settings| {
            let rules = &mut settings.permission_rules;
            if let Some(fence) = fencing_rule(rules, &prefix) {
                return Err(CoreError::invalid(format!(
                    "permission rule `{fence}` fences a grant for `{prefix}`; remove it first"
                )));
            }
            let rule = bash_prefix_rule(&prefix);
            if !rules.allow.contains(&rule) {
                rules.allow.push(rule);
            }
            Ok(())
        })
        .map(|_| ())
    }

    /// The raw `hooks.json` source (empty when unset).
    pub fn hooks_json(&self) -> Result<String> {
        Ok(self.load()?.map(|s| s.hooks_json).unwrap_or_default())
    }

    /// The parsed external hook definitions (empty when unset). Returns an error
    /// only if the stored JSON is malformed.
    pub fn hook_definitions(&self) -> Result<deepagent_hooks::HookDefinitions> {
        let raw = self.hooks_json()?;
        if raw.trim().is_empty() {
            return Ok(deepagent_hooks::HookDefinitions::default());
        }
        deepagent_hooks::HookDefinitions::parse(&raw)
    }

    /// Set the `hooks.json` source, persisting it. Validates that the JSON
    /// parses (when non-empty) before saving so the UI gets immediate feedback.
    pub fn set_hooks_json(&self, hooks_json: &str) -> Result<()> {
        if !hooks_json.trim().is_empty() {
            // Validate before persisting; reject malformed input early.
            deepagent_hooks::HookDefinitions::parse(hooks_json)?;
        }
        self.update_settings(|settings| {
            settings.hooks_json = hooks_json.to_string();
            Ok(())
        })
        .map(|_| ())
    }

    /// The redacted view for the UI, or `None` if uninitialized.
    pub fn view(&self) -> Result<Option<SettingsView>> {
        self.load()?
            .map(|settings| self.view_from_settings(&settings))
            .transpose()
    }

    fn view_from_settings(&self, settings: &AppSettings) -> Result<SettingsView> {
        let configured = match self.db.harness_chat_model_selection()? {
            Some(selection) => match self
                .db
                .harness_model_route(&selection.provider_id, &selection.model_name)?
            {
                Some((provider, model)) if provider.enabled && model.active => self
                    .secrets
                    .get(&provider.api_key_ref)?
                    .is_some_and(|key| !key.trim().is_empty()),
                _ => false,
            },
            None => false,
        };
        let reasoning_effort =
            match crate::model_runtime::resolve_run_model(&self.db, settings, None, None, None) {
                Ok(plan) => plan
                    .reasoning_effort
                    .map(|value| value.label().to_owned())
                    .unwrap_or_else(|| "default".into()),
                Err(error) => {
                    tracing::warn!(%error, "reasoning setting is not valid for the active model");
                    "default".into()
                }
            };
        Ok(SettingsView {
            configured,
            approval_policy: settings.approval_policy.label().to_string(),
            sandbox_mode: settings.sandbox_mode.label().to_string(),
            terminal_shell: settings.terminal_shell.label().to_string(),
            reasoning_effort,
            web_search: normalized_web_search_settings(settings.web_search.clone()),
            execution_features: settings.execution_features,
            output_style: settings.output_style.as_str().to_string(),
            pricing_catalog: settings.pricing_catalog.clone(),
        })
    }

    // Explicit full snapshots are only for test setup. Production writers use
    // edit_settings/update_settings and never publish cached whole documents.
    #[cfg(test)]
    fn save(&self, settings: &AppSettings) -> Result<()> {
        let mut cache = self
            .settings_cache
            .lock()
            .map_err(|_| CoreError::other("settings cache lock poisoned"))?;
        let store = DocumentStore::new(&self.db);
        let body = serde_json::to_string(settings)?;
        store.put(
            SETTINGS_COLLECTION,
            SETTINGS_ID,
            &body,
            None,
            Timestamp::from_millis(now_ms()),
        )?;
        *cache = Some((body, settings.clone()));
        Ok(())
    }
}

fn now_ms() -> i64 {
    use deepagent_core::clock::{Clock, SystemClock};
    SystemClock.now().as_millis()
}

fn normalized_web_search_settings(mut settings: WebSearchSettings) -> WebSearchSettings {
    settings.normalize();
    settings
}

#[path = "settings/persistence.rs"]
mod persistence;

#[path = "settings/harness_registry.rs"]
mod harness_registry;

#[cfg(test)]
#[path = "settings/reasoning_tests.rs"]
mod reasoning_tests;

#[cfg(test)]
#[path = "settings/repair_tests.rs"]
mod repair_tests;

#[cfg(test)]
#[path = "settings/process_tests.rs"]
mod process_tests;

#[cfg(test)]
#[path = "settings/web_search_tests.rs"]
mod web_search_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::MemorySecretStore;
    use async_trait::async_trait;
    use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Notify;

    struct GateTransport {
        body: String,
        block_next: Arc<AtomicBool>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl HttpTransport for GateTransport {
        async fn stream(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
        ) -> Result<()> {
            Err(CoreError::other("stream not used by settings audit"))
        }

        async fn get_json(&self, _url: &str, _api_key: &str) -> Result<String> {
            if self.block_next.swap(false, Ordering::SeqCst) {
                self.started.notify_one();
                self.release.notified().await;
            }
            Ok(self.body.clone())
        }
    }

    #[tokio::test]
    async fn context_headroom_overrides_legacy_reserve_and_survives_settings_update() {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-synthetic-test")
            .await
            .unwrap();
        let mut settings = svc.load().unwrap().unwrap();
        settings.autocompact_reserve_tokens = Some(23_456);
        settings.context_headroom_tokens = Some(65_536);
        svc.save(&settings).unwrap();
        assert_eq!(svc.autocompact_reserve_tokens(), Some(65_536));
        crate::registry_fixture::configure(&svc, "sk-synthetic-test")
            .await
            .unwrap();
        assert_eq!(
            svc.load().unwrap().unwrap().context_headroom_tokens,
            Some(65_536)
        );
        let mut legacy = serde_json::to_value(&settings).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("context_headroom_tokens");
        let legacy: AppSettings = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.context_headroom_tokens, None);
        assert_eq!(legacy.autocompact_reserve_tokens, Some(23_456));
    }

    fn transport_with_models() -> Arc<dyn HttpTransport> {
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
        ]}"#;
        Arc::new(MockTransport::with_get_json(body))
    }

    fn service() -> (SettingsService, Arc<MemorySecretStore>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let svc = SettingsService::new(db, transport_with_models(), secrets.clone());
        (svc, secrets)
    }

    #[tokio::test]
    async fn legacy_system_vision_is_ignored_without_breaking_settings_or_exposing_it_in_the_view()
    {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "synthetic-vision-migration")
            .await
            .unwrap();
        let mut legacy = serde_json::to_value(svc.load().unwrap().unwrap()).unwrap();
        legacy["vision"] = serde_json::json!({
            "mode": "system", "base_url": "https://legacy-vision.invalid/v1",
            "system_model": "legacy-vision", "api_key": "synthetic-unused-secret"
        });
        let settings: AppSettings = serde_json::from_value(legacy).unwrap();
        assert!(serde_json::to_value(&settings)
            .unwrap()
            .get("vision")
            .is_none());
        svc.save(&settings).unwrap();
        let view = serde_json::to_value(svc.view().unwrap().unwrap()).unwrap();
        assert!(view.get("vision").is_none());
        assert_eq!(view["configured"], true);
        assert!(view["configured"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn a_prefix_grant_is_a_permission_rule_the_settings_page_lists() {
        let (svc, _secrets) = service();
        crate::registry_fixture::configure(&svc, "sk-secret-1234")
            .await
            .unwrap();
        assert!(svc.command_prefix_grants().unwrap().is_empty());

        svc.grant_command_prefix("git status").unwrap();
        // The rule is spelled in the vocabulary the permission-rules editor
        // already shows, so seeing it and revoking it need no second surface.
        assert_eq!(
            svc.permission_rules().unwrap().allow,
            vec!["Bash(git status:*)".to_string()]
        );
        assert_eq!(svc.command_prefix_grants().unwrap(), ["git status"]);

        // The same answer twice, or spelled with extra spaces, is still one rule.
        svc.grant_command_prefix("git status").unwrap();
        svc.grant_command_prefix("  git   status ").unwrap();
        assert_eq!(svc.permission_rules().unwrap().allow.len(), 1);

        // A bare `bash` allow rule claims every command; it is not a prefix grant.
        svc.set_permission_rules(PermissionRules::new(
            ["bash".to_string()],
            [],
            ["WebSearch".to_string()],
        ))
        .unwrap();
        assert!(svc.command_prefix_grants().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_grant_never_walks_past_a_rule_the_user_wrote() {
        let (svc, _secrets) = service();
        crate::registry_fixture::configure(&svc, "sk-secret-1234")
            .await
            .unwrap();
        svc.set_permission_rules(PermissionRules::new(
            [],
            ["Bash(git:*)".to_string()],
            ["Bash(sudo:*)".to_string()],
        ))
        .unwrap();

        let refused = svc.grant_command_prefix("git status").unwrap_err();
        assert!(refused.to_string().contains("fences"), "{refused}");
        assert!(svc.permission_rules().unwrap().allow.is_empty());
        // A deny fence blocks it too, even though the allow list has nothing yet.
        let refused = svc.grant_command_prefix("sudo rm out").unwrap_err();
        assert!(refused.to_string().contains("fences"), "{refused}");
        // An unrelated command is still grantable.
        svc.grant_command_prefix("npm run").unwrap();
        assert_eq!(svc.command_prefix_grants().unwrap(), ["npm run"]);
    }

    #[tokio::test]
    async fn a_prefix_grant_survives_a_restart() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let first = SettingsService::new(
            db.clone(),
            transport_with_models(),
            Arc::new(MemorySecretStore::new()),
        );
        crate::registry_fixture::configure(&first, "sk-secret-1234")
            .await
            .unwrap();
        first.grant_command_prefix("cargo test").unwrap();

        // A fresh service over the same data root (the next process) reads it back.
        let second = SettingsService::new(
            db,
            transport_with_models(),
            Arc::new(MemorySecretStore::new()),
        );
        assert_eq!(second.command_prefix_grants().unwrap(), ["cargo test"]);

        // Deleting the rule in settings is the revocation: nothing left to honor.
        second
            .set_permission_rules(PermissionRules::default())
            .unwrap();
        assert!(second.command_prefix_grants().unwrap().is_empty());
    }

    #[tokio::test]
    async fn output_style_round_trip_and_takes_effect_in_prompt() {
        use crate::system_context::output_style_prompt_block;
        let (svc, _secrets) = service();
        crate::registry_fixture::configure(&svc, "sk-secret-1234")
            .await
            .unwrap();

        // Default: no style block injected.
        assert_eq!(svc.output_style(), OutputStyle::Default);
        assert!(output_style_prompt_block(svc.output_style()).is_none());

        // Switch to Explanatory → persisted, surfaced in the view, and the
        // injected prompt block changes (“output style 切换生效” 验收).
        svc.set_output_style(OutputStyle::Explanatory).unwrap();
        assert_eq!(svc.output_style(), OutputStyle::Explanatory);
        assert_eq!(svc.view().unwrap().unwrap().output_style, "explanatory");
        let block = output_style_prompt_block(svc.output_style());
        assert!(block.is_some(), "Explanatory must inject a style block");

        // Parse round-trips the labels; unknown is rejected.
        assert_eq!(OutputStyle::parse("learning"), Some(OutputStyle::Learning));
        assert_eq!(OutputStyle::parse("Default"), Some(OutputStyle::Default));
        assert_eq!(OutputStyle::parse("nonsense"), None);
    }

    #[tokio::test]
    async fn execution_features_default_off_and_round_trip() {
        let (svc, _secrets) = service();
        crate::registry_fixture::configure(&svc, "sk-secret-1234")
            .await
            .unwrap();

        // Default: every opt-in safeguard is OFF (自创机制默认从宽).
        let features = svc.execution_features();
        assert_eq!(features, ExecutionFeatures::default());
        assert!(!features.stall_detector);
        assert!(!features.command_guard);
        assert!(!features.project_trust);
        assert!(!features.adversarial_verify);

        // Persist a subset and confirm it round-trips (and surfaces in the view).
        svc.set_execution_features(ExecutionFeatures {
            stall_detector: true,
            command_guard: false,
            project_trust: true,
            adversarial_verify: false,
        })
        .unwrap();
        let features = svc.execution_features();
        assert!(features.stall_detector);
        assert!(!features.command_guard);
        assert!(features.project_trust);
        assert!(!features.adversarial_verify);
        let view = svc.view().unwrap().unwrap();
        assert!(view.execution_features.stall_detector);
        assert!(view.execution_features.project_trust);
    }

    #[tokio::test]
    async fn audit_refresh_models_preserves_other_host_sandbox_mode() {
        let database_dir = tempfile::tempdir().unwrap();
        let database_path = database_dir.path().join("settings.sqlite");
        let first_db = Arc::new(Database::open(&database_path).unwrap());
        let secrets = Arc::new(MemorySecretStore::new());
        let body = r#"{"object":"list","data":[
            {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
            {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
        ]}"#;
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let block_next = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(GateTransport {
            body: body.to_string(),
            block_next: block_next.clone(),
            started: started.clone(),
            release: release.clone(),
        });
        let first = Arc::new(SettingsService::new(first_db, transport, secrets.clone()));
        crate::registry_fixture::configure(&first, "sk-audit-placeholder")
            .await
            .unwrap();
        // The first host keeps the original sandbox policy in its own cache.
        assert_eq!(
            first.load().unwrap().unwrap().sandbox_mode,
            SandboxMode::WorkspaceWrite
        );

        let second = SettingsService::new(
            Arc::new(Database::open(&database_path).unwrap()),
            Arc::new(MockTransport::with_get_json(body)),
            secrets,
        );
        block_next.store(true, Ordering::SeqCst);
        let refresh = {
            let first = first.clone();
            tokio::spawn(async move { first.harness_models().fetch_models("deepseek").await })
        };
        started.notified().await;

        // A different host changes settings while the first host is in its
        // network await. The first host later reads its warm cache, rather than
        // the database values written by this host.
        let restricted = second.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
        assert_eq!(restricted.sandbox_mode, "read_only");
        release.notify_one();
        refresh.await.unwrap().unwrap();

        let reopened = SettingsService::new(
            Arc::new(Database::open(&database_path).unwrap()),
            Arc::new(MockTransport::with_get_json(body)),
            Arc::new(MemorySecretStore::new()),
        );
        let reopened_settings = reopened.load().unwrap().unwrap();
        assert_eq!(
            reopened_settings.sandbox_mode,
            SandboxMode::ReadOnly,
            "refresh overwrote the sandbox restriction persisted by another host"
        );
    }

    #[tokio::test]
    async fn empty_key_rejected_and_not_stored() {
        let (svc, secrets) = service();
        assert!(crate::registry_fixture::configure(&svc, "   ")
            .await
            .is_err());
        assert!(secrets.get("harness_provider:deepseek").unwrap().is_none());
    }

    #[tokio::test]
    async fn reasoning_effort_roundtrips_and_survives_refresh() {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        let view = svc.set_reasoning_effort(ReasoningEffort::Max).unwrap();
        assert_eq!(view.reasoning_effort, "max");
        assert_eq!(svc.reasoning_effort().unwrap(), ReasoningEffort::Max);
        svc.harness_models().fetch_models("deepseek").await.unwrap();
        let refreshed = svc.view().unwrap().unwrap();
        assert_eq!(refreshed.reasoning_effort, "max");
    }

    #[tokio::test]
    async fn sandbox_mode_roundtrips_and_survives_refresh() {
        let (svc, _) = service();
        let initial = crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        assert_eq!(initial.sandbox_mode, "workspace_write");
        assert_eq!(svc.sandbox_mode().unwrap(), SandboxMode::WorkspaceWrite);

        let view = svc.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
        assert_eq!(view.sandbox_mode, "read_only");
        assert_eq!(svc.sandbox_mode().unwrap(), SandboxMode::ReadOnly);

        svc.harness_models().fetch_models("deepseek").await.unwrap();
        let refreshed = svc.view().unwrap().unwrap();
        assert_eq!(refreshed.sandbox_mode, "read_only");
    }

    #[tokio::test]
    async fn set_unknown_model_rejected() {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        assert!(svc
            .harness_models()
            .select_model("deepseek", "ghost-model")
            .is_err());
    }

    #[tokio::test]
    async fn tool_search_mode_default_is_auto_and_round_trips() {
        // Default after initialize is Auto so large deferred toolsets do not
        // bloat every prompt. Setting / getting roundtrips through the
        // SQLite-backed settings doc, and survives a discovery refresh.
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        assert_eq!(
            svc.tool_search_mode().unwrap(),
            deepagent_builtins::ToolSearchMode::Auto
        );

        svc.set_tool_search_mode(deepagent_builtins::ToolSearchMode::Enabled)
            .unwrap();
        assert_eq!(
            svc.tool_search_mode().unwrap(),
            deepagent_builtins::ToolSearchMode::Enabled
        );

        svc.set_tool_search_mode(deepagent_builtins::ToolSearchMode::Auto)
            .unwrap();
        assert_eq!(
            svc.tool_search_mode().unwrap(),
            deepagent_builtins::ToolSearchMode::Auto
        );

        svc.harness_models().fetch_models("deepseek").await.unwrap();
        assert_eq!(
            svc.tool_search_mode().unwrap(),
            deepagent_builtins::ToolSearchMode::Auto
        );
    }

    #[tokio::test]
    async fn web_search_settings_default_and_round_trip() {
        let (svc, _) = service();
        let view = crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        assert_eq!(view.web_search.schema_version, 2);
        assert_eq!(view.web_search.strategy, WebSearchStrategy::Fallback);
        assert_eq!(
            view.web_search
                .engines
                .iter()
                .map(|engine| engine.id)
                .collect::<Vec<_>>(),
            [
                WebSearchEngineId::Deepseek,
                WebSearchEngineId::Anysearch,
                WebSearchEngineId::Searxng,
                WebSearchEngineId::Duckduckgo
            ]
        );
        assert!(view.web_search.engines[0].enabled);
        assert!(!view.web_search.engines[1].enabled);
        assert!(view.web_search.engines[2].enabled && view.web_search.engines[3].enabled);

        let settings = WebSearchSettings {
            schema_version: 2,
            strategy: WebSearchStrategy::Verify,
            engines: vec![
                WebSearchEngineSettings {
                    id: WebSearchEngineId::Searxng,
                    enabled: true,
                    base_url: Some(" https://search.example.com/ ".into()),
                },
                WebSearchEngineSettings {
                    id: WebSearchEngineId::Deepseek,
                    enabled: false,
                    base_url: None,
                },
                WebSearchEngineSettings {
                    id: WebSearchEngineId::Duckduckgo,
                    enabled: false,
                    base_url: None,
                },
                WebSearchEngineSettings {
                    id: WebSearchEngineId::Anysearch,
                    enabled: true,
                    base_url: Some(" https://api.anysearch.com/ ".into()),
                },
            ],
            ranking: WebSearchRankingSettings {
                method: WebSearchRankingMethod::System,
                profile: WebSearchRankingProfile::Timely,
            },
        };
        svc.set_web_search_settings(settings).unwrap();
        let persisted = svc.web_search_settings().unwrap();
        assert_eq!(persisted.strategy, WebSearchStrategy::Verify);
        assert_eq!(persisted.ranking.profile, WebSearchRankingProfile::Timely);
        assert_eq!(persisted.engines[0].id, WebSearchEngineId::Searxng);
        assert_eq!(
            persisted.engines[0].base_url.as_deref(),
            Some("https://search.example.com")
        );

        svc.harness_models().fetch_models("deepseek").await.unwrap();
        let refreshed = svc.view().unwrap().unwrap();
        assert_eq!(refreshed.web_search.strategy, WebSearchStrategy::Verify);
        assert_eq!(
            refreshed.web_search.engines[0].id,
            WebSearchEngineId::Searxng
        );
        assert_eq!(
            refreshed.web_search.engines[0].base_url.as_deref(),
            Some("https://search.example.com")
        );
    }

    #[tokio::test]
    async fn web_search_runtime_config_uses_current_login_key_and_drops_it_after_sign_out() {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "synthetic-old-login-key")
            .await
            .unwrap();

        assert_eq!(
            svc.web_search_runtime_config()
                .unwrap()
                .deepseek_api_key
                .as_deref(),
            Some("synthetic-old-login-key")
        );

        crate::registry_fixture::configure(&svc, "synthetic-new-login-key")
            .await
            .unwrap();
        assert_eq!(
            svc.web_search_runtime_config()
                .unwrap()
                .deepseek_api_key
                .as_deref(),
            Some("synthetic-new-login-key")
        );

        svc.harness_models().clear_credentials().unwrap();
        assert_eq!(
            svc.web_search_runtime_config().unwrap().deepseek_api_key,
            None
        );
    }

    // ---- Skill marketplace settings (R10) ---------------------------------

    #[tokio::test]
    async fn skill_marketplace_settings_have_documented_defaults() {
        // R10.1: defaults after initialize are: enabled=true, budget=8000,
        // ai_review=true, ai_review_model=None.
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();

        assert!(
            svc.skill_catalog_enabled().unwrap(),
            "catalog default is on"
        );
        assert_eq!(svc.skill_catalog_char_budget().unwrap(), 8_000);
        assert!(
            svc.skill_install_ai_review_enabled().unwrap(),
            "AI review default is on"
        );
        assert!(svc.skill_install_ai_review_model().unwrap().is_none());
    }

    #[tokio::test]
    async fn skill_marketplace_settings_round_trip_and_survive_refresh() {
        // R10.2: persistence round-trips and changes take effect without
        // needing a fresh initialize. We use refresh_models() (the closest
        // thing to a "restart" available to a unit test) to confirm the
        // values survive a re-discovery cycle.
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();

        svc.set_skill_catalog_enabled(false).unwrap();
        svc.set_skill_catalog_char_budget(0).unwrap();
        svc.set_skill_install_ai_review_enabled(false).unwrap();
        svc.set_skill_install_ai_review_model(Some("deepseek-v4-pro".into()))
            .unwrap();

        assert!(!svc.skill_catalog_enabled().unwrap());
        assert_eq!(svc.skill_catalog_char_budget().unwrap(), 0);
        assert!(!svc.skill_install_ai_review_enabled().unwrap());
        assert_eq!(
            svc.skill_install_ai_review_model().unwrap().as_deref(),
            Some("deepseek-v4-pro")
        );

        // Refresh models (re-runs discovery and persists). Skill settings
        // must carry over via the `prior` path in `initialize`.
        svc.harness_models().fetch_models("deepseek").await.unwrap();

        assert!(!svc.skill_catalog_enabled().unwrap());
        assert_eq!(svc.skill_catalog_char_budget().unwrap(), 0);
        assert!(!svc.skill_install_ai_review_enabled().unwrap());
        assert_eq!(
            svc.skill_install_ai_review_model().unwrap().as_deref(),
            Some("deepseek-v4-pro")
        );
    }

    #[tokio::test]
    async fn skill_install_ai_review_model_normalizes_empty_to_none() {
        // Pass an empty / whitespace-only string and the setter falls back
        // to None, so the consumer site reads "use the chat model".
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();

        svc.set_skill_install_ai_review_model(Some("   ".into()))
            .unwrap();
        assert!(svc.skill_install_ai_review_model().unwrap().is_none());

        svc.set_skill_install_ai_review_model(Some("custom-model".into()))
            .unwrap();
        svc.set_skill_install_ai_review_model(None).unwrap();
        assert!(svc.skill_install_ai_review_model().unwrap().is_none());
    }

    #[test]
    fn old_settings_payload_decodes_with_skill_defaults() {
        // R10.1 backwards-compatibility: a settings JSON written by an
        // older build (no skill_* keys) must decode cleanly with the
        // documented defaults. This is the contract that `#[serde(default
        // = "...")]` provides; we lock it in with an explicit test.
        let json = r#"{"sandbox_mode":"workspace_write"}"#;
        let parsed: AppSettings = serde_json::from_str(json).expect("decode old payload");

        assert!(parsed.skill_catalog_enabled);
        assert_eq!(parsed.skill_catalog_char_budget, 8_000);
        assert!(parsed.skill_install_ai_review_enabled);
        assert!(parsed.skill_install_ai_review_model.is_none());
        assert_eq!(parsed.web_search.schema_version, 2);
        assert!(parsed
            .web_search
            .engines
            .iter()
            .any(|engine| engine.id == WebSearchEngineId::Deepseek && engine.enabled));
    }

    #[tokio::test]
    async fn skill_settings_serialize_with_new_fields() {
        // Round-trip: persisted JSON must contain all four new keys
        // (snake_case, matching the field names) so the desktop layer's
        // Tauri commands can decode + re-encode them without lossiness.
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        svc.set_skill_catalog_char_budget(123).unwrap();
        svc.set_skill_install_ai_review_model(Some("m".into()))
            .unwrap();

        let persisted = svc.load().unwrap().unwrap();
        let json = serde_json::to_string(&persisted).unwrap();
        assert!(json.contains("\"web_search\""));
        assert!(json.contains("\"skill_catalog_enabled\":true"));
        assert!(json.contains("\"skill_catalog_char_budget\":123"));
        assert!(json.contains("\"skill_install_ai_review_enabled\":true"));
        assert!(json.contains("\"skill_install_ai_review_model\":\"m\""));
    }

    #[tokio::test]
    async fn hooks_json_roundtrips_and_validates() {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        // Default empty.
        assert_eq!(svc.hooks_json().unwrap(), "");
        assert!(svc.hook_definitions().unwrap().is_empty());

        let json = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"validate"}]}]}}"#;
        svc.set_hooks_json(json).unwrap();
        assert_eq!(svc.hooks_json().unwrap(), json);
        let defs = svc.hook_definitions().unwrap();
        assert_eq!(defs.action_count(), 1);

        // Malformed JSON is rejected and not persisted.
        assert!(svc.set_hooks_json("{ not json").is_err());
        assert_eq!(svc.hooks_json().unwrap(), json);
    }

    #[tokio::test]
    async fn hooks_json_survives_refresh() {
        let (svc, _) = service();
        crate::registry_fixture::configure(&svc, "sk-abcd1234")
            .await
            .unwrap();
        let json = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"./stop.sh"}]}]}}"#;
        svc.set_hooks_json(json).unwrap();
        svc.harness_models().fetch_models("deepseek").await.unwrap();
        assert_eq!(svc.hooks_json().unwrap(), json);
    }
}
#[tokio::test]
async fn load_preserves_explicit_responses_wire_values() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let svc = SettingsService::new(
        db.clone(),
        Arc::new(deepagent_models::transport::MockTransport::with_get_json(
            r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(crate::secret_store::MemorySecretStore::new()),
    );
    crate::registry_fixture::configure(&svc, "sk-synthetic-test")
        .await
        .unwrap();
    let store = DocumentStore::new(&svc.db);
    let doc = store
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap();
    let mut stored: serde_json::Value = serde_json::from_str(&doc.body).unwrap();
    stored["responses"]["reasoning_effort"] = serde_json::json!("medium");
    stored["responses"]["developer"] = serde_json::json!({
        "reasoning": { "effort": "xhigh" },
        "temperature": 0.4
    });
    store
        .put(
            SETTINGS_COLLECTION,
            SETTINGS_ID,
            &stored.to_string(),
            None,
            Timestamp::from_millis(now_ms()),
        )
        .unwrap();
    *svc.settings_cache.lock().unwrap() = None;

    let loaded = svc.load().unwrap().unwrap();
    assert_eq!(loaded.responses.reasoning_effort.as_deref(), Some("medium"));
    assert_eq!(loaded.responses.developer["reasoning"]["effort"], "xhigh");
    assert_eq!(loaded.responses.effective_temperature(), Some(0.4));

    let view = serde_json::to_value(svc.view().unwrap().unwrap()).unwrap();
    assert!(view.get("responses").is_none());
    assert!(view.get("responses_sampling").is_none());

    let cleaned = store
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap();
    let cleaned: serde_json::Value = serde_json::from_str(&cleaned.body).unwrap();
    assert!(cleaned.get("thinking_depth").is_none());
    assert!(cleaned.get("reasoning_effort").is_none());
    assert_eq!(cleaned["responses"]["reasoning_effort"], "medium");
    assert_eq!(
        cleaned["responses"]["developer"]["reasoning"]["effort"],
        "xhigh"
    );
    assert_eq!(cleaned["responses"]["developer"]["temperature"], 0.4);
}

#[test]
fn responses_parameter_layers_follow_documented_priority() {
    let settings = ResponsesApiSettings {
        creativity: Some(90),
        scene: Some(ResponseScenePreset::Creative),
        temperature: Some(0.7),
        developer: serde_json::Map::from_iter([(
            "temperature".to_string(),
            serde_json::json!(0.2),
        )]),
        ..ResponsesApiSettings::default()
    };
    assert_eq!(settings.effective_temperature(), Some(0.2));

    let panel = ResponsesApiSettings {
        developer: serde_json::Map::new(),
        ..settings.clone()
    };
    assert_eq!(panel.effective_temperature(), Some(0.7));
    let scene = ResponsesApiSettings {
        temperature: None,
        ..panel.clone()
    };
    assert_eq!(scene.effective_temperature(), Some(1.5));
    let creativity = ResponsesApiSettings {
        scene: None,
        ..scene
    };
    assert_eq!(creativity.effective_temperature(), Some(1.8));
}
