//! Project settings & initialization.
//!
//! The product flow: at project init the user supplies **only an API key**. The
//! system (base URL hard-coded to DeepSeek) discovers available models, auto-
//! selects the latest ones into a [`ModelCatalog`], and persists everything.
//! That persisted model configuration is the key the rest of the system reads
//! to know which model to call.
//!
//! **Credential storage:** API keys live behind [`SecretStore`]. The desktop
//! persists authenticated ciphertext in SQLite and keeps only a device-bound
//! wrapping secret in the OS keychain. Public [`ModelCatalog`] settings remain
//! in the `documents` table (collection `"settings"`, id `"app"`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use deepagent_builtins::prefix_covers;
use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::{bash_prefix_rule, rule_command_prefix, PermissionRules};
use deepagent_models::balance::{fetch_balance, BalanceResponse};
use deepagent_models::capability::{ModelCapability, ModelCapabilityResolver};
use deepagent_models::chat::validate_responses_user_identifier;
use deepagent_models::discovery::{ModelCatalog, ModelDiscovery};
use deepagent_models::transport::HttpTransport;
use deepagent_models::ReasoningEffort;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;

use crate::cost_service::ModelPricing;
use crate::secret_store::SecretStore;

/// The document-store collection + id the (non-secret) settings live under.
const SETTINGS_COLLECTION: &str = "settings";
const SETTINGS_ID: &str = "app";
/// The logical name the API key is stored under in the secret store.
const API_KEY_NAME: &str = "deepseek_api_key";
/// The third-party system-vision API key lives in the same SecretStore.
const VISION_API_KEY_NAME: &str = "vision_api_key";
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

/// Global DeepSeek Responses controls. Unknown or provider-managed fields are
/// rejected before persistence; ineffective fields are retained with warnings
/// by the UI but never silently dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ResponsesApiSettings {
    /// User-level creativity control. `None` preserves the provider/runtime
    /// default until the user explicitly moves the control.
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
    /// Strictly validated developer JSON, kept separate so editing it never
    /// destroys ordinary/advanced product-layer settings.
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

    pub fn effective_reasoning_effort(&self) -> Option<ReasoningEffort> {
        self.developer
            .get("reasoning")
            .and_then(|value| value.get("effort"))
            .and_then(serde_json::Value::as_str)
            .and_then(ReasoningEffort::parse)
            .or_else(|| {
                self.reasoning_effort
                    .as_deref()
                    .and_then(ReasoningEffort::parse)
            })
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

    pub fn validate(&self) -> Result<()> {
        if self.creativity.is_some_and(|value| value > 100) {
            return Err(CoreError::invalid(
                "Responses creativity must be between 0 and 100",
            ));
        }
        if let Some(value) = self.temperature {
            if !(0.0..=2.0).contains(&value) {
                return Err(CoreError::invalid(
                    "Responses temperature must be between 0 and 2",
                ));
            }
        }
        if let Some(value) = self.top_p {
            if !(0.0..=1.0).contains(&value) {
                return Err(CoreError::invalid(
                    "Responses top_p must be between 0 and 1",
                ));
            }
        }
        if let Some(value) = self.top_logprobs {
            if value > 20 {
                return Err(CoreError::invalid(
                    "Responses top_logprobs must be between 0 and 20",
                ));
            }
        }
        if let Some(value) = &self.reasoning_effort {
            if !matches!(value.as_str(), "none" | "low" | "high" | "max") {
                return Err(CoreError::invalid(
                    "Responses reasoning_effort must be none, low, high, or max",
                ));
            }
        }
        if let Some(text) = &self.text {
            validate_developer_responses_fields(&serde_json::Map::from_iter([(
                "text".to_string(),
                text.clone(),
            )]))?;
        }
        if let Some(tool_choice) = &self.tool_choice {
            validate_developer_responses_fields(&serde_json::Map::from_iter([(
                "tool_choice".to_string(),
                tool_choice.clone(),
            )]))?;
        }
        if let Some(user) = &self.user {
            validate_responses_user_identifier(user)?;
        }
        if !self.developer.is_empty() {
            let raw = serde_json::Value::Object(self.developer.clone()).to_string();
            Self::from_developer_json(&raw)?;
        }
        Ok(())
    }

    pub fn from_developer_json(raw: &str) -> Result<Self> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .map_err(|e| CoreError::invalid(format!("invalid Responses JSON: {e}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| CoreError::invalid("Responses JSON must be an object"))?
            .clone();
        let allowed = [
            "temperature",
            "top_p",
            "max_output_tokens",
            "top_logprobs",
            "reasoning",
            "text",
            "tool_choice",
            "user",
            "verbosity",
            "parallel_tool_calls",
            "max_tool_calls",
            "store",
        ];
        let rejected = [
            "model",
            "input",
            "instructions",
            "tools",
            "stream",
            "previous_response_id",
            "conversation",
            "background",
            "metadata",
            "include",
            "prompt",
            "truncation",
            "service_tier",
            "safety_identifier",
            "prompt_cache_key",
            "prompt_cache_retention",
            "context_management",
            "stream_options",
            "stop",
        ];
        for key in object.keys() {
            if rejected.contains(&key.as_str()) {
                return Err(CoreError::invalid(format!(
                    "Responses field `{key}` is system-managed or unsupported by DeepSeek"
                )));
            }
            if !allowed.contains(&key.as_str()) {
                return Err(CoreError::invalid(format!(
                    "unknown Responses field `{key}`"
                )));
            }
        }
        validate_developer_responses_fields(&object)?;
        let mut settings = Self {
            developer: object.clone(),
            ..Self::default()
        };
        for key in [
            "verbosity",
            "parallel_tool_calls",
            "max_tool_calls",
            "store",
        ] {
            if let Some(v) = object.get(key) {
                settings.ineffective.insert(key.to_string(), v.clone());
            }
        }
        Ok(settings)
    }
}

fn validate_developer_responses_fields(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    if let Some(value) = object.get("temperature") {
        if value
            .as_f64()
            .map(|value| !(0.0..=2.0).contains(&value))
            .unwrap_or(true)
        {
            return Err(CoreError::invalid(
                "Responses temperature must be a number between 0 and 2",
            ));
        }
    }
    if let Some(value) = object.get("top_p") {
        if value
            .as_f64()
            .map(|value| !(0.0..=1.0).contains(&value))
            .unwrap_or(true)
        {
            return Err(CoreError::invalid(
                "Responses top_p must be a number between 0 and 1",
            ));
        }
    }
    if object.get("max_output_tokens").is_some_and(|value| {
        value
            .as_u64()
            .map(|tokens| tokens == 0 || tokens > u64::from(u32::MAX))
            .unwrap_or(true)
    }) {
        return Err(CoreError::invalid(
            "Responses max_output_tokens must be a positive integer",
        ));
    }
    if object
        .get("top_logprobs")
        .is_some_and(|value| value.as_u64().map(|count| count > 20).unwrap_or(true))
    {
        return Err(CoreError::invalid(
            "Responses top_logprobs must be an integer between 0 and 20",
        ));
    }
    if let Some(reasoning) = object.get("reasoning") {
        let Some(reasoning) = reasoning.as_object() else {
            return Err(CoreError::invalid("Responses reasoning must be an object"));
        };
        if reasoning.keys().any(|key| key != "effort") {
            return Err(CoreError::invalid(
                "DeepSeek Responses reasoning only supports `effort`",
            ));
        }
        if !matches!(
            reasoning.get("effort").and_then(serde_json::Value::as_str),
            Some("none" | "low" | "high" | "max")
        ) {
            return Err(CoreError::invalid(
                "Responses reasoning.effort must be none, low, high, or max",
            ));
        }
    }
    if let Some(text) = object.get("text") {
        let Some(text) = text.as_object() else {
            return Err(CoreError::invalid("Responses text must be an object"));
        };
        if text.keys().any(|key| key != "format") {
            return Err(CoreError::invalid(
                "DeepSeek Responses text only supports `format`",
            ));
        }
        let Some(format) = text.get("format").and_then(serde_json::Value::as_object) else {
            return Err(CoreError::invalid(
                "Responses text.format must be an object",
            ));
        };
        let format_type = format
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CoreError::invalid("Responses text.format.type is required"))?;
        match format_type {
            "text" | "json_object" => {
                if format.keys().any(|key| key != "type") {
                    return Err(CoreError::invalid(format!(
                        "Responses text.format `{format_type}` has unknown fields"
                    )));
                }
            }
            "json_schema" => {
                if format.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "type" | "name" | "description" | "schema" | "strict"
                    )
                }) {
                    return Err(CoreError::invalid(
                        "Responses text.format json_schema has unknown fields",
                    ));
                }
                if format
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::is_empty)
                    .unwrap_or(true)
                    || format
                        .get("schema")
                        .map(|value| !value.is_object())
                        .unwrap_or(true)
                {
                    return Err(CoreError::invalid(
                        "Responses text.format json_schema requires a non-empty name and object schema",
                    ));
                }
                if format
                    .get("strict")
                    .is_some_and(|value| !value.is_boolean())
                {
                    return Err(CoreError::invalid(
                        "Responses text.format.strict must be boolean",
                    ));
                }
            }
            _ => {
                return Err(CoreError::invalid(
                    "Responses text.format.type must be text, json_object, or json_schema",
                ));
            }
        }
    }
    if let Some(choice) = object.get("tool_choice") {
        deepagent_models::validate_responses_tool_choice(choice)?;
    }
    if let Some(value) = object.get("user") {
        let user = value
            .as_str()
            .ok_or_else(|| CoreError::invalid("Responses user must be a string"))?;
        validate_responses_user_identifier(user)?;
    }
    if object
        .get("verbosity")
        .is_some_and(|value| !matches!(value.as_str(), Some("low" | "medium" | "high")))
    {
        return Err(CoreError::invalid(
            "Responses verbosity must be low, medium, or high",
        ));
    }
    if object
        .get("parallel_tool_calls")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(CoreError::invalid(
            "Responses parallel_tool_calls must be boolean",
        ));
    }
    if object
        .get("max_tool_calls")
        .is_some_and(|value| value.as_u64().map(|count| count == 0).unwrap_or(true))
    {
        return Err(CoreError::invalid(
            "Responses max_tool_calls must be a positive integer",
        ));
    }
    if object.get("store").is_some_and(|value| !value.is_boolean()) {
        return Err(CoreError::invalid("Responses store must be boolean"));
    }
    Ok(())
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

/// How pasted/dropped images are converted into model-readable context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VisionMode {
    /// Do not use a third-party vision API: image attachments are sent to the
    /// main model directly as structured image parts (native DeepSeek vision).
    Off,
    /// Use the configured third-party vision API, then pass the text result to
    /// the main chat model.
    #[default]
    System,
}

impl<'de> Deserialize<'de> for VisionMode {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Tolerant read: unknown values (including the removed legacy `"model"`
        // mode) fall back instead of failing the whole settings load.
        let raw = String::deserialize(deserializer)?;
        Ok(Self::parse(&raw).unwrap_or(VisionMode::System))
    }
}

impl VisionMode {
    pub const fn label(&self) -> &'static str {
        match self {
            VisionMode::Off => "off",
            VisionMode::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "system" => Some(Self::System),
            // Legacy: the removed `model` mode selected native vision, which the
            // `system`/`off` toggle now expresses as `off`.
            "model" => Some(Self::Off),
            _ => None,
        }
    }
}

fn default_vision_provider() -> String {
    "modelscope".to_string()
}

fn default_vision_base_url() -> String {
    "https://api-inference.modelscope.cn/v1".to_string()
}

fn default_system_vision_model() -> String {
    "moonshotai/Kimi-K2.5:DashScope".to_string()
}

fn default_vision_timeout_ms() -> u64 {
    60_000
}

fn default_auto_analyze_pasted_images() -> bool {
    true
}

/// Persisted image-analysis settings for composer attachments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisionSettings {
    #[serde(default)]
    pub mode: VisionMode,
    #[serde(default = "default_vision_provider")]
    pub provider: String,
    #[serde(default = "default_vision_base_url")]
    pub base_url: String,
    #[serde(default, skip_serializing)]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing)]
    pub api_key_configured: bool,
    #[serde(default = "default_system_vision_model")]
    pub system_model: String,
    #[serde(default = "default_vision_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_auto_analyze_pasted_images")]
    pub auto_analyze_pasted_images: bool,
}

impl Default for VisionSettings {
    fn default() -> Self {
        Self {
            mode: VisionMode::default(),
            provider: default_vision_provider(),
            base_url: default_vision_base_url(),
            api_key: None,
            api_key_configured: false,
            system_model: default_system_vision_model(),
            timeout_ms: default_vision_timeout_ms(),
            auto_analyze_pasted_images: default_auto_analyze_pasted_images(),
        }
    }
}

fn normalize_vision_settings(mut settings: VisionSettings) -> VisionSettings {
    settings.provider = settings.provider.trim().to_ascii_lowercase();
    if settings.provider.is_empty() {
        settings.provider = default_vision_provider();
    }
    settings.base_url = settings.base_url.trim().trim_end_matches('/').to_string();
    if settings.base_url.is_empty() {
        settings.base_url = default_vision_base_url();
    }
    if settings.system_model.trim().is_empty() {
        settings.system_model = default_system_vision_model();
    } else {
        settings.system_model = settings.system_model.trim().to_string();
    }
    if settings.timeout_ms == 0 {
        settings.timeout_ms = default_vision_timeout_ms();
    }
    settings.api_key_configured = false;
    settings
}

/// Persisted, **non-secret** application settings (safe to store on disk).
/// The API key is intentionally absent — it lives in the [`SecretStore`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// The discovered + auto-selected model configuration.
    pub catalog: ModelCatalog,
    /// When discovery last ran (Unix ms).
    pub discovered_at: i64,
    /// How tool approvals are resolved.
    #[serde(default)]
    pub approval_policy: ApprovalPolicy,
    /// Maximum filesystem boundary for tool execution.
    pub sandbox_mode: SandboxMode,
    /// Preferred shell for the desktop integrated terminal.
    #[serde(default)]
    pub terminal_shell: TerminalShell,
    /// Declarative permission rules (allow/ask/deny patterns).
    #[serde(default)]
    pub permission_rules: PermissionRules,
    /// Declarative external hooks (`hooks.json` source). Stored verbatim so the
    /// UI round-trips exactly what the user typed; parsed lazily when assembling
    /// the runtime hook registry.
    #[serde(default)]
    pub hooks_json: String,
    /// User-selected DeepSeek Responses reasoning effort.
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
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
    /// Local/model vision settings for image attachments.
    #[serde(default)]
    pub vision: VisionSettings,
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
    /// Masked key (e.g. `"sk-…last4"`), or `"(not set)"`. Never the full secret.
    pub api_key_masked: String,
    /// Base URL in use (hard-coded DeepSeek).
    pub base_url: String,
    /// All discovered model ids.
    pub available_models: Vec<String>,
    /// Resolved capability (context window / max output) for every discovered
    /// model, resolved exactly the way the runtime resolves it (provider
    /// metadata wins, then the bundled official-doc snapshot, then a
    /// conservative fallback). Lets the UI show the real context capacity
    /// instead of guessing it from the model id.
    #[serde(default)]
    pub model_capabilities: Vec<ModelCapability>,
    /// Selected chat model.
    pub chat_model: String,
    /// Selected reasoner model.
    pub reasoner_model: String,
    /// Whether the project is initialized (key present + models discovered).
    pub configured: bool,
    /// Current approval policy label (always_ask / auto_review / full_access).
    pub approval_policy: String,
    /// Current sandbox mode label (read_only / workspace_write / full_access).
    pub sandbox_mode: String,
    /// Current preferred integrated-terminal shell.
    pub terminal_shell: String,
    /// Current canonical DeepSeek Responses reasoning effort.
    pub reasoning_effort: String,
    #[serde(default)]
    pub responses: ResponsesApiSettings,
    /// Read-only scenarios for the selected role models, not persisted settings
    /// or a claim about a future run's dynamic tool set/model overrides.
    #[serde(default)]
    pub responses_sampling: Vec<ResponsesSamplingPreview>,
    /// Current web-search settings.
    pub web_search: WebSearchSettings,
    /// Current image-analysis settings.
    pub vision: VisionSettings,
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

/// One per-currency balance row exposed to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BalanceInfoDto {
    /// Currency code (e.g. `"CNY"`, `"USD"`).
    pub currency: String,
    /// Total spendable balance (granted + topped-up).
    pub total_balance: String,
    /// Granted (free credit) portion.
    pub granted_balance: String,
    /// Topped-up (paid) portion.
    pub topped_up_balance: String,
}

/// Account balance summary from the DeepSeek `/user/balance` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BalanceDto {
    /// Whether the account currently has any spendable balance.
    pub is_available: bool,
    /// Per-currency balance breakdown.
    pub infos: Vec<BalanceInfoDto>,
}

impl BalanceDto {
    fn from_response(resp: BalanceResponse) -> Self {
        Self {
            is_available: resp.is_available,
            infos: resp
                .balance_infos
                .into_iter()
                .map(|i| BalanceInfoDto {
                    currency: i.currency,
                    total_balance: i.total_balance,
                    granted_balance: i.granted_balance,
                    topped_up_balance: i.topped_up_balance,
                })
                .collect(),
        }
    }
}

/// Mask an API key to a non-leaking preview.
fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 4 {
        "****".to_string()
    } else {
        let last4: String = key.chars().skip(n - 4).collect();
        format!("sk-…{last4}")
    }
}

/// Manages project settings + initialization over a database, a transport, and
/// a secret store. The DB holds the public catalog; the secret store holds the
/// API key.
pub struct SettingsService {
    db: Arc<Database>,
    transport: Arc<dyn HttpTransport>,
    secrets: Arc<dyn SecretStore>,
    settings_cache: Mutex<Option<(String, AppSettings)>>,
}

/// Initialization may choose defaults; discovery refresh has no such authority.
#[derive(Clone, Copy)]
enum CatalogUpdate {
    Initialize,
    Refresh,
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
    let legacy_depth_value = settings.remove("thinking_depth");
    let legacy_depth = legacy_depth_value
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut changes = usize::from(legacy_depth_value.is_some());
    if settings
        .get("reasoning_effort")
        .is_some_and(|value| value.as_str().and_then(ReasoningEffort::parse).is_none())
    {
        settings.remove("reasoning_effort");
        changes += 1;
    }
    if !settings.contains_key("reasoning_effort") {
        let migrated = match legacy_depth.as_deref() {
            Some("simple") => Some(ReasoningEffort::None),
            Some("medium") => Some(ReasoningEffort::High),
            Some("deep") => Some(ReasoningEffort::Max),
            _ => None,
        };
        if let Some(effort) = migrated {
            settings.insert(
                "reasoning_effort".to_string(),
                serde_json::Value::String(effort.label().to_string()),
            );
            changes += 1;
        }
    }

    let Some(responses) = settings
        .get_mut("responses")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return changes;
    };
    if let Some(effort) = responses.get("reasoning_effort") {
        match effort.as_str() {
            Some("medium") => {
                responses.insert(
                    "reasoning_effort".to_string(),
                    serde_json::Value::String(ReasoningEffort::High.label().to_string()),
                );
                changes += 1;
            }
            Some(value) if ReasoningEffort::parse(value).is_none() => {
                responses.remove("reasoning_effort");
                changes += 1;
            }
            _ => {}
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
        match effort {
            Some("medium") => {
                if let Some(reasoning) = developer
                    .get_mut("reasoning")
                    .and_then(serde_json::Value::as_object_mut)
                {
                    reasoning.insert(
                        "effort".to_string(),
                        serde_json::Value::String(ReasoningEffort::High.label().to_string()),
                    );
                    changes += 1;
                }
            }
            Some(value) if ReasoningEffort::parse(value).is_none() => {
                developer.remove("reasoning");
                changes += 1;
            }
            _ => {}
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

    /// Initialize the project with just an API key: store the key in the secret
    /// store, discover models, auto-select the catalog, persist the catalog, and
    /// return the redacted view. The single call the UI makes after the user
    /// enters their key.
    pub async fn initialize(&self, api_key: &str) -> Result<SettingsView> {
        if api_key.trim().is_empty() {
            return Err(CoreError::invalid("API key must not be empty"));
        }
        // Intent takes a shared durable generation before waiting for discovery.
        let generation = self.begin_credential_change()?;
        // Discover first so we never store a key that doesn't work.
        let discovery = ModelDiscovery::deepseek(self.transport.clone());
        let catalog = discovery.discover(api_key).await?;

        self.complete_credential_change(&generation, api_key, catalog)
    }

    fn merge_discovered_catalog(
        &self,
        catalog: ModelCatalog,
        update: CatalogUpdate,
    ) -> Result<AppSettings> {
        // Merge only discovery-owned fields into the current durable document.
        let discovered_at = now_ms();
        let pricing = Self::pricing_snapshot();
        self.edit_settings(|prior| {
            let mut catalog = catalog.clone();
            if let (CatalogUpdate::Refresh, Some(current)) = (update, prior.as_ref()) {
                // Re-evaluate the latest durable choices on every CAS attempt,
                // never the choices captured before asynchronous discovery.
                if !catalog.has_model(&current.catalog.chat_model)
                    || !catalog.has_model(&current.catalog.reasoner_model)
                {
                    return Err(CoreError::invalid(
                        "model catalog refresh rejected: selected model is unavailable; \
                         choose another model or explicitly initialize again",
                    ));
                }
                catalog.chat_model = current.catalog.chat_model.clone();
                catalog.reasoner_model = current.catalog.reasoner_model.clone();
            }
            let mut settings = match prior {
                Some(settings) => settings,
                None => serde_json::from_value(serde_json::json!({
                    "catalog": &catalog,
                    "discovered_at": discovered_at,
                    "sandbox_mode": SandboxMode::default(),
                    "tool_search_mode": Self::DEFAULT_TOOL_SEARCH_MODE,
                }))?,
            };
            settings.catalog = catalog;
            settings.discovered_at = discovered_at;
            settings.pricing_catalog = pricing.clone();
            settings.web_search = normalized_web_search_settings(settings.web_search);
            Ok(settings)
        })
    }

    /// Re-run discovery with the stored key (e.g. to pick up new models).
    /// Preserve current role choices; reject the entire publication if either
    /// selected id disappears. Pricing refreshes only alongside a committed catalog.
    pub async fn refresh_models(&self) -> Result<SettingsView> {
        self.discover_for_refresh().await
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

    /// Manually override which model fills a role (must be a discovered id).
    pub fn set_model(
        &self,
        role: deepagent_models::ModelRole,
        model_id: &str,
    ) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            if !settings.catalog.has_model(model_id) {
                return Err(CoreError::invalid(format!(
                    "model '{model_id}' is not in the discovered set"
                )));
            }
            match role {
                deepagent_models::ModelRole::Chat => {
                    settings.catalog.chat_model = model_id.to_string()
                }
                deepagent_models::ModelRole::Reasoner => {
                    settings.catalog.reasoner_model = model_id.to_string()
                }
            }
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Load the public settings (catalog) from the database.
    pub fn load(&self) -> Result<Option<AppSettings>> {
        self.load_current_settings()
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

    /// The raw API key from the secret store (for internal runtime use only).
    pub fn api_key(&self) -> Result<Option<String>> {
        self.read_current_credential()
    }

    /// Validate an API key with a read-only `/models` discovery call.
    ///
    /// Unlike [`initialize`](Self::initialize) or [`refresh_models`](Self::refresh_models),
    /// this does not persist the returned catalog. It is used by `/doctor` so a
    /// diagnostic scan can verify credentials without mutating settings.
    pub async fn validate_api_key(&self, api_key: &str) -> Result<usize> {
        if api_key.trim().is_empty() {
            return Err(CoreError::invalid("API key must not be empty"));
        }
        let discovery = ModelDiscovery::deepseek(self.transport.clone());
        let models = discovery.list_models(api_key).await?;
        if models.is_empty() {
            return Err(CoreError::other(
                "model discovery returned no models (check API key / connectivity)",
            ));
        }
        Ok(models.len())
    }

    /// Fetch the user's DeepSeek account balance via `GET /user/balance` using
    /// the stored API key. Returns a UI-safe DTO. Errors when the key is not
    /// set, the catalog is missing, or the network/auth fails — the caller
    /// surfaces the error message verbatim.
    pub async fn query_balance(&self) -> Result<BalanceDto> {
        let key = self
            .api_key()?
            .ok_or_else(|| CoreError::not_found("API key not set; initialize first"))?;
        let settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        let resp = fetch_balance(self.transport.clone(), &settings.catalog.base_url, &key).await?;
        Ok(BalanceDto::from_response(resp))
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

    /// The current DeepSeek Thinking Mode depth.
    pub fn reasoning_effort(&self) -> Result<ReasoningEffort> {
        Ok(self.load()?.map(|s| s.reasoning_effort).unwrap_or_default())
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
        let deepseek_ready = self.api_key()?.is_some_and(|key| !key.trim().is_empty());
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
            .then(|| self.api_key())
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

    /// Current image-analysis settings.
    pub fn vision_settings(&self) -> Result<VisionSettings> {
        let settings = self.load()?.map(|s| s.vision).unwrap_or_default();
        self.vision_settings_view(settings)
    }

    /// Set image-analysis settings.
    pub fn set_vision_settings(&self, settings_next: VisionSettings) -> Result<VisionSettings> {
        self.load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        if let Some(api_key) = settings_next.api_key.as_deref() {
            let api_key = api_key.trim();
            if api_key.is_empty() {
                self.secrets.delete(VISION_API_KEY_NAME)?;
            } else {
                self.secrets.set(VISION_API_KEY_NAME, api_key)?;
            }
        }
        let next = normalize_vision_settings(settings_next);
        let settings = self.update_settings(|settings| {
            settings.vision = next.clone();
            Ok(())
        })?;
        self.vision_settings_view(settings.vision)
    }

    /// The configured third-party system-vision API key, if present.
    pub fn vision_api_key(&self) -> Result<Option<String>> {
        self.secrets.get(VISION_API_KEY_NAME)
    }

    fn vision_settings_view(&self, mut settings: VisionSettings) -> Result<VisionSettings> {
        settings.api_key = None;
        settings.api_key_configured = self
            .secrets
            .get(VISION_API_KEY_NAME)?
            .map(|key| !key.trim().is_empty())
            .unwrap_or(false);
        Ok(settings)
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
        let settings = self.update_settings(|settings| {
            settings.reasoning_effort = effort;
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
    }

    pub fn responses_settings(&self) -> Result<ResponsesApiSettings> {
        Ok(self.load()?.map(|s| s.responses).unwrap_or_default())
    }

    pub fn set_responses_settings(
        &self,
        mut settings_next: ResponsesApiSettings,
    ) -> Result<SettingsView> {
        settings_next.validate()?;
        let canonical_developer = ResponsesApiSettings::from_developer_json(
            &serde_json::Value::Object(settings_next.developer.clone()).to_string(),
        )?;
        settings_next.developer = canonical_developer.developer;
        settings_next.ineffective = canonical_developer.ineffective;
        let settings = self.update_settings(|settings| {
            settings.responses = settings_next.clone();
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
    }

    pub fn set_responses_developer_json(&self, raw: &str) -> Result<SettingsView> {
        let developer = ResponsesApiSettings::from_developer_json(raw)?;
        let settings = self.update_settings(|settings| {
            settings.responses.developer = developer.developer.clone();
            settings.responses.ineffective = developer.ineffective.clone();
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Set the approval policy, persisting it. Returns the redacted view.
    pub fn set_approval_policy(&self, policy: ApprovalPolicy) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            settings.approval_policy = policy;
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Set the sandbox mode, persisting it. Returns the redacted view.
    pub fn set_sandbox_mode(&self, mode: SandboxMode) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            settings.sandbox_mode = mode;
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Set the preferred integrated-terminal shell, persisting it.
    pub fn set_terminal_shell(&self, shell: TerminalShell) -> Result<SettingsView> {
        let settings = self.update_settings(|settings| {
            settings.terminal_shell = shell;
            Ok(())
        })?;
        let key = self.api_key()?;
        self.view_with_key(key.as_deref(), &settings)
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
        self.current_credential_view()
    }

    /// Clear the stored key (sign-out). Leaves the catalog in place.
    pub fn clear_key(&self) -> Result<()> {
        self.clear_current_credential()
    }

    fn view_with_key(&self, key: Option<&str>, settings: &AppSettings) -> Result<SettingsView> {
        // Resolve through the same resolver the runtime uses so the UI never
        // has to guess a model's context window from its id.
        let capability_resolver = ModelCapabilityResolver::new();
        Ok(SettingsView {
            api_key_masked: key.map(mask_key).unwrap_or_else(|| "(not set)".to_string()),
            base_url: settings.catalog.base_url.clone(),
            available_models: settings
                .catalog
                .available
                .iter()
                .map(|m| m.id.clone())
                .collect(),
            model_capabilities: settings
                .catalog
                .available
                .iter()
                .map(|m| capability_resolver.resolve_model(m))
                .collect(),
            chat_model: settings.catalog.chat_model.clone(),
            reasoner_model: settings.catalog.reasoner_model.clone(),
            configured: key.map(|k| !k.trim().is_empty()).unwrap_or(false)
                && !settings.catalog.available.is_empty(),
            approval_policy: settings.approval_policy.label().to_string(),
            sandbox_mode: settings.sandbox_mode.label().to_string(),
            terminal_shell: settings.terminal_shell.label().to_string(),
            reasoning_effort: settings.reasoning_effort.label().to_string(),
            responses: settings.responses.clone(),
            responses_sampling: responses_sampling_preview(settings),
            web_search: normalized_web_search_settings(settings.web_search.clone()),
            vision: self.vision_settings_view(settings.vision.clone())?,
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
            Timestamp::from_millis(settings.discovered_at),
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

#[path = "settings/credential_lifecycle.rs"]
mod credential_lifecycle;

mod sampling;
use sampling::responses_sampling_preview;
pub use sampling::ResponsesSamplingPreview;

#[cfg(test)]
#[path = "settings/repair_tests.rs"]
mod repair_tests;

#[cfg(test)]
#[path = "settings/selection_tests.rs"]
mod selection_tests;

#[cfg(test)]
#[path = "settings/process_tests.rs"]
mod process_tests;

#[cfg(test)]
#[path = "settings/credential_tests.rs"]
mod credential_tests;

#[cfg(test)]
#[path = "settings/web_search_tests.rs"]
mod web_search_tests;

#[cfg(test)]
#[path = "settings/credential_process_tests.rs"]
mod credential_process_tests;

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
        svc.initialize("sk-synthetic-test").await.unwrap();
        let mut settings = svc.load().unwrap().unwrap();
        settings.autocompact_reserve_tokens = Some(23_456);
        settings.context_headroom_tokens = Some(65_536);
        svc.save(&settings).unwrap();
        assert_eq!(svc.autocompact_reserve_tokens(), Some(65_536));
        svc.initialize("sk-synthetic-test").await.unwrap();
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

    #[test]
    fn vision_mode_tolerates_legacy_and_unknown_values() {
        // Legacy persisted "model" selected native vision → maps to Off now.
        let legacy: VisionMode = serde_json::from_str("\"model\"").unwrap();
        assert_eq!(legacy, VisionMode::Off);
        // Unknown values fall back to the safe default (third-party off? no: system).
        let unknown: VisionMode = serde_json::from_str("\"nonsense\"").unwrap();
        assert_eq!(unknown, VisionMode::System);
        assert_eq!(VisionMode::parse("system"), Some(VisionMode::System));
        assert_eq!(VisionMode::parse("off"), Some(VisionMode::Off));
        assert_eq!(VisionMode::parse("model"), Some(VisionMode::Off));
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
    async fn pricing_activation_initialization_commits_models_and_prices_together() {
        let (settings, _) = service();
        settings
            .db
            .with_conn(|connection| {
                connection
                    .execute_batch(
                        "CREATE TRIGGER reject_initial_pricing BEFORE INSERT ON documents
                 WHEN NEW.collection = 'settings' AND NEW.id = 'app'
                 BEGIN SELECT RAISE(ABORT, 'synthetic initial save failure'); END;",
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        assert!(settings
            .initialize("synthetic-pricing-test")
            .await
            .unwrap_err()
            .to_string()
            .contains("synthetic initial save failure"));
        assert!(settings.load().unwrap().is_none());
        settings
            .db
            .with_conn(|connection| {
                connection
                    .execute_batch(
                        "DROP TRIGGER reject_initial_pricing;
                 CREATE TRIGGER reject_second_pricing_write BEFORE UPDATE ON documents
                 WHEN NEW.collection = 'settings' AND NEW.id = 'app'
                 BEGIN SELECT RAISE(ABORT, 'models/prices must be one commit'); END;",
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        let view = settings.initialize("synthetic-pricing-test").await.unwrap();
        let persisted: AppSettings = serde_json::from_str(
            &DocumentStore::new(&settings.db)
                .get(SETTINGS_COLLECTION, SETTINGS_ID)
                .unwrap()
                .unwrap()
                .body,
        )
        .unwrap();
        assert!(!persisted.pricing_catalog.models.is_empty());
        assert_eq!(view.pricing_catalog, persisted.pricing_catalog);
        assert_eq!(
            settings.load().unwrap().unwrap().pricing_catalog,
            persisted.pricing_catalog
        );
    }

    #[tokio::test]
    async fn pricing_activation_save_publishes_only_committed_catalog() {
        use crate::cost_service::{CostRecordRequest, CostService};

        let (settings, _) = service();
        let settings = Arc::new(settings);
        settings.initialize("synthetic-pricing-test").await.unwrap();
        settings
            .db
            .with_conn(|connection| {
                connection
                    .execute_batch(
                        "INSERT INTO sessions (id, title, mode, created_at, updated_at)
                 VALUES ('pricing', 'fixture', 'normal', 0, 0);
                 CREATE TRIGGER reject_pricing_update BEFORE UPDATE ON documents
                 WHEN NEW.collection = 'settings' AND NEW.id = 'app'
                 BEGIN SELECT RAISE(ABORT, 'synthetic settings write failure'); END;",
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        let cost = CostService::from_settings(settings.db.clone(), settings.clone());
        let committed = settings.load().unwrap().unwrap();
        let mut changed = committed.clone();
        let price = changed
            .pricing_catalog
            .models
            .get_mut("deepseek-flash")
            .unwrap();
        price.output_per_million = 100.0;
        assert!(settings
            .save(&changed)
            .unwrap_err()
            .to_string()
            .contains("synthetic settings write failure"));
        assert_eq!(
            settings.load().unwrap().unwrap().pricing_catalog,
            committed.pricing_catalog
        );
        let request = || CostRecordRequest {
            session_id: "pricing".into(),
            model: "deepseek-flash".into(),
            input_tokens: 1000,
            output_tokens: 100,
            cache_hit_tokens: 800,
            cache_miss_tokens: 200,
            total_tokens: 1100,
        };
        assert_eq!(cost.record(request()).unwrap(), Some(0.000616));
        settings
            .db
            .with_conn(|connection| {
                connection
                    .execute_batch("DROP TRIGGER reject_pricing_update")
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        settings.save(&changed).unwrap();
        assert_eq!(cost.record(request()).unwrap(), Some(0.010216));
        let rows = deepagent_persistence::cost_store::CostStore::new(&settings.db)
            .session_costs_detailed("pricing")
            .unwrap();
        assert_eq!(rows.len(), 2);
        // A later activation never reprices an already committed cost row.
        assert_eq!(rows[0].output_price, Some(4.0));
        assert_eq!(rows[1].output_price, Some(100.0));
    }

    #[tokio::test]
    async fn pricing_activation_serializes_database_and_snapshot_publication() {
        let (settings, _) = service();
        settings.initialize("synthetic-pricing-test").await.unwrap();
        let settings = Arc::new(settings);
        // Force both cold load and save to wait at the real DB boundary. They
        // must already own the cache guard, preventing out-of-order publication.
        for loading in [true, false] {
            if loading {
                *settings.settings_cache.lock().unwrap() = None;
            }
            let current = serde_json::from_str::<AppSettings>(
                &DocumentStore::new(&settings.db)
                    .get(SETTINGS_COLLECTION, SETTINGS_ID)
                    .unwrap()
                    .unwrap()
                    .body,
            )
            .unwrap();
            settings
                .db
                .with_conn(|_| {
                    let owner = settings.clone();
                    let worker = std::thread::spawn(move || {
                        if loading {
                            owner.load().map(|_| ())
                        } else {
                            owner.save(&current)
                        }
                    });
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                    while settings.settings_cache.try_lock().is_ok() {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "DB operation did not acquire snapshot publication guard"
                        );
                        std::thread::yield_now();
                    }
                    // Return the worker; joining while holding the DB would deadlock.
                    Ok(worker)
                })
                .unwrap()
                .join()
                .unwrap()
                .unwrap();
        }
        let start = Arc::new(std::sync::Barrier::new(9));
        let workers: Vec<_> = (1..=8)
            .map(|version| {
                let owner = settings.clone();
                let start = start.clone();
                std::thread::spawn(move || {
                    let mut snapshot = owner.load().unwrap().unwrap();
                    snapshot.pricing_catalog.fetched_at = version;
                    snapshot
                        .pricing_catalog
                        .models
                        .get_mut("deepseek-flash")
                        .unwrap()
                        .output_per_million = version as f64;
                    start.wait();
                    owner.save(&snapshot).unwrap();
                })
            })
            .collect();
        start.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        let persisted: AppSettings = serde_json::from_str(
            &DocumentStore::new(&settings.db)
                .get(SETTINGS_COLLECTION, SETTINGS_ID)
                .unwrap()
                .unwrap()
                .body,
        )
        .unwrap();
        assert_eq!(
            settings.load().unwrap().unwrap().pricing_catalog,
            persisted.pricing_catalog
        );
    }

    #[tokio::test]
    async fn a_prefix_grant_is_a_permission_rule_the_settings_page_lists() {
        let (svc, _secrets) = service();
        svc.initialize("sk-secret-1234").await.unwrap();
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
        svc.initialize("sk-secret-1234").await.unwrap();
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
        first.initialize("sk-secret-1234").await.unwrap();
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
        svc.initialize("sk-secret-1234").await.unwrap();

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
        svc.initialize("sk-secret-1234").await.unwrap();

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
    async fn initialize_stores_key_through_secret_store_boundary() {
        let (svc, secrets) = service();
        let view = svc.initialize("sk-secret-1234").await.unwrap();
        assert!(view.configured);
        assert_eq!(view.chat_model, "deepseek-v4-flash");
        assert_eq!(view.reasoner_model, "deepseek-v4-pro");
        assert_eq!(view.reasoning_effort, "high");
        assert_eq!(view.api_key_masked, "sk-…1234");

        // The key is in the secret store...
        assert_eq!(
            secrets.get("deepseek_api_key").unwrap().as_deref(),
            Some("sk-secret-1234")
        );
        // ...but NOT in the persisted (DB) settings JSON.
        let persisted = svc.load().unwrap().unwrap();
        let json = serde_json::to_string(&persisted).unwrap();
        assert!(!json.contains("sk-secret-1234"));
        assert!(!json.contains("\"deepseek_api_key\""));
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
        first.initialize("sk-audit-placeholder").await.unwrap();
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
            tokio::spawn(async move { first.refresh_models().await })
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
    async fn audit_refresh_models_does_not_restore_a_superseded_api_key() {
        use crate::secret_store::SqliteSecretStore;

        let body =
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#;
        let mut overwritten_actions = Vec::new();
        for action in ["sign_out", "replace_key"] {
            let database_dir = tempfile::tempdir().unwrap();
            let database_path = database_dir.path().join("credentials.sqlite");
            let first_db = Arc::new(Database::open(&database_path).unwrap());
            let wrapping = Arc::new(MemorySecretStore::new());
            let first_secrets =
                Arc::new(SqliteSecretStore::new(first_db.clone(), wrapping.clone()));
            let started = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let block_next = Arc::new(AtomicBool::new(false));
            let first = Arc::new(SettingsService::new(
                first_db,
                Arc::new(GateTransport {
                    body: body.to_string(),
                    block_next: block_next.clone(),
                    started: started.clone(),
                    release: release.clone(),
                }),
                first_secrets,
            ));
            first.initialize("audit-old-credential").await.unwrap();

            block_next.store(true, Ordering::SeqCst);
            let refresh = {
                let first = first.clone();
                tokio::spawn(async move { first.refresh_models().await })
            };
            tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
                .await
                .expect("model refresh must reach the controlled network wait");

            let second_db = Arc::new(Database::open(&database_path).unwrap());
            let second = SettingsService::new(
                second_db.clone(),
                Arc::new(MockTransport::with_get_json(body)),
                Arc::new(SqliteSecretStore::new(second_db, wrapping.clone())),
            );
            let expected_key = if action == "sign_out" {
                second.clear_key().unwrap();
                None
            } else {
                second.initialize("audit-new-credential").await.unwrap();
                Some("audit-new-credential")
            };
            assert_eq!(second.api_key().unwrap().as_deref(), expected_key);

            release.notify_one();
            // Rejecting a stale refresh is valid; it must not restore its key.
            let refresh_succeeded = refresh.await.unwrap().is_ok();
            let reopened_db = Arc::new(Database::open(&database_path).unwrap());
            let reopened = SettingsService::new(
                reopened_db.clone(),
                Arc::new(MockTransport::with_get_json(body)),
                Arc::new(SqliteSecretStore::new(reopened_db, wrapping)),
            );
            let reopened_key = reopened.api_key().unwrap();
            let preserved = reopened_key.as_deref() == expected_key;
            let restored_previous = reopened_key.as_deref() == Some("audit-old-credential");
            let configured = reopened.view().unwrap().unwrap().configured;
            eprintln!(
                "action={action} refresh_succeeded={refresh_succeeded} credential_preserved={preserved} previous_restored={restored_previous} configured={configured}"
            );
            assert!(
                preserved || restored_previous,
                "unexpected fixture credential"
            );
            if !preserved {
                overwritten_actions.push(action);
            }
        }
        assert!(
            overwritten_actions.is_empty(),
            "a stale refresh restored its credential after: {overwritten_actions:?}"
        );
    }

    #[tokio::test]
    async fn runtime_can_read_raw_key() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
        assert_eq!(svc.api_key().unwrap().as_deref(), Some("sk-abcd1234"));
    }

    #[tokio::test]
    async fn empty_key_rejected_and_not_stored() {
        let (svc, secrets) = service();
        assert!(svc.initialize("   ").await.is_err());
        assert!(secrets.get("deepseek_api_key").unwrap().is_none());
    }

    #[tokio::test]
    async fn clear_key_signs_out_but_keeps_catalog() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
        svc.clear_key().unwrap();
        assert!(svc.api_key().unwrap().is_none());
        // Catalog remains; view shows not-configured + masked "(not set)".
        let view = svc.view().unwrap().unwrap();
        assert!(!view.configured);
        assert_eq!(view.api_key_masked, "(not set)");
        assert_eq!(view.chat_model, "deepseek-v4-flash");
    }

    #[tokio::test]
    async fn set_model_overrides_role() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
        let view = svc
            .set_model(deepagent_models::ModelRole::Chat, "deepseek-v4-pro")
            .unwrap();
        assert_eq!(view.chat_model, "deepseek-v4-pro");
    }

    #[tokio::test]
    async fn reasoning_effort_roundtrips_and_survives_refresh() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
        let view = svc.set_reasoning_effort(ReasoningEffort::Max).unwrap();
        assert_eq!(view.reasoning_effort, "max");
        assert_eq!(svc.reasoning_effort().unwrap(), ReasoningEffort::Max);
        let refreshed = svc.refresh_models().await.unwrap();
        assert_eq!(refreshed.reasoning_effort, "max");
    }

    #[tokio::test]
    async fn sandbox_mode_roundtrips_and_survives_refresh() {
        let (svc, _) = service();
        let initial = svc.initialize("sk-abcd1234").await.unwrap();
        assert_eq!(initial.sandbox_mode, "workspace_write");
        assert_eq!(svc.sandbox_mode().unwrap(), SandboxMode::WorkspaceWrite);

        let view = svc.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
        assert_eq!(view.sandbox_mode, "read_only");
        assert_eq!(svc.sandbox_mode().unwrap(), SandboxMode::ReadOnly);

        let refreshed = svc.refresh_models().await.unwrap();
        assert_eq!(refreshed.sandbox_mode, "read_only");
    }

    #[tokio::test]
    async fn set_unknown_model_rejected() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
        assert!(svc
            .set_model(deepagent_models::ModelRole::Chat, "ghost-model")
            .is_err());
    }

    #[tokio::test]
    async fn tool_search_mode_default_is_auto_and_round_trips() {
        // Default after initialize is Auto so large deferred toolsets do not
        // bloat every prompt. Setting / getting roundtrips through the
        // SQLite-backed settings doc, and survives a discovery refresh.
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
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

        svc.refresh_models().await.unwrap();
        assert_eq!(
            svc.tool_search_mode().unwrap(),
            deepagent_builtins::ToolSearchMode::Auto
        );
    }

    #[test]
    fn tool_search_mode_set_before_initialize_errors() {
        let (svc, _) = service();
        let err = svc
            .set_tool_search_mode(deepagent_builtins::ToolSearchMode::Enabled)
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn web_search_settings_default_and_round_trip() {
        let (svc, _) = service();
        let view = svc.initialize("sk-abcd1234").await.unwrap();
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

        let refreshed = svc.refresh_models().await.unwrap();
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
        svc.initialize("synthetic-old-login-key").await.unwrap();

        assert_eq!(
            svc.web_search_runtime_config()
                .unwrap()
                .deepseek_api_key
                .as_deref(),
            Some("synthetic-old-login-key")
        );

        svc.initialize("synthetic-new-login-key").await.unwrap();
        assert_eq!(
            svc.web_search_runtime_config()
                .unwrap()
                .deepseek_api_key
                .as_deref(),
            Some("synthetic-new-login-key")
        );

        svc.clear_key().unwrap();
        assert_eq!(
            svc.web_search_runtime_config().unwrap().deepseek_api_key,
            None
        );
    }

    #[test]
    fn web_search_settings_set_before_initialize_errors() {
        let (svc, _) = service();
        let err = svc
            .set_web_search_settings(WebSearchSettings::default())
            .unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    // ---- Skill marketplace settings (R10) ---------------------------------

    #[tokio::test]
    async fn skill_marketplace_settings_have_documented_defaults() {
        // R10.1: defaults after initialize are: enabled=true, budget=8000,
        // ai_review=true, ai_review_model=None.
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();

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
        svc.initialize("sk-abcd1234").await.unwrap();

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
        svc.refresh_models().await.unwrap();

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
        svc.initialize("sk-abcd1234").await.unwrap();

        svc.set_skill_install_ai_review_model(Some("   ".into()))
            .unwrap();
        assert!(svc.skill_install_ai_review_model().unwrap().is_none());

        svc.set_skill_install_ai_review_model(Some("custom-model".into()))
            .unwrap();
        svc.set_skill_install_ai_review_model(None).unwrap();
        assert!(svc.skill_install_ai_review_model().unwrap().is_none());
    }

    #[test]
    fn skill_settings_set_before_initialize_errors() {
        // Mirrors `tool_search_mode_set_before_initialize_errors` so the
        // failure mode is consistent across all post-init settings.
        let (svc, _) = service();
        assert!(matches!(
            svc.set_skill_catalog_enabled(false).unwrap_err(),
            CoreError::NotFound(_)
        ));
        assert!(matches!(
            svc.set_skill_catalog_char_budget(0).unwrap_err(),
            CoreError::NotFound(_)
        ));
        assert!(matches!(
            svc.set_skill_install_ai_review_enabled(false).unwrap_err(),
            CoreError::NotFound(_)
        ));
        assert!(matches!(
            svc.set_skill_install_ai_review_model(Some("m".into()))
                .unwrap_err(),
            CoreError::NotFound(_)
        ));
    }

    #[test]
    fn old_settings_payload_decodes_with_skill_defaults() {
        // R10.1 backwards-compatibility: a settings JSON written by an
        // older build (no skill_* keys) must decode cleanly with the
        // documented defaults. This is the contract that `#[serde(default
        // = "...")]` provides; we lock it in with an explicit test.
        let json = r#"{
            "catalog": {
                "base_url": "https://api.deepseek.com/v1",
                "available": [
                    {"id": "deepseek-v4-flash", "object": "model", "owned_by": "deepseek"}
                ],
                "chat_model": "deepseek-v4-flash",
                "reasoner_model": "deepseek-v4-flash"
            },
            "discovered_at": 0,
            "sandbox_mode": "workspace_write"
        }"#;
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
        svc.initialize("sk-abcd1234").await.unwrap();
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

    #[test]
    fn view_before_init_is_none() {
        let (svc, _) = service();
        assert!(svc.view().unwrap().is_none());
    }

    #[tokio::test]
    async fn hooks_json_roundtrips_and_validates() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
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
        svc.initialize("sk-abcd1234").await.unwrap();
        let json = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"./stop.sh"}]}]}}"#;
        svc.set_hooks_json(json).unwrap();
        svc.refresh_models().await.unwrap();
        assert_eq!(svc.hooks_json().unwrap(), json);
    }

    #[tokio::test]
    async fn query_balance_without_key_errors_cleanly() {
        let (svc, _) = service();
        let err = svc.query_balance().await.unwrap_err();
        assert!(matches!(err, CoreError::NotFound(_)));
    }

    #[tokio::test]
    async fn query_balance_parses_response() {
        // A bespoke service whose mock transport returns a balance body so we
        // can exercise the parsing path without going through `initialize`
        // (which expects the models body).
        let body = r#"{
            "is_available": true,
            "balance_infos": [
                {"currency":"CNY","total_balance":"42.50","granted_balance":"2.50","topped_up_balance":"40.00"}
            ]
        }"#;
        let db = Arc::new(Database::open_in_memory().unwrap());
        let secrets: Arc<dyn crate::secret_store::SecretStore> = Arc::new(MemorySecretStore::new());
        secrets.set("deepseek_api_key", "sk-test").unwrap();
        let transport: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(body));
        let svc = SettingsService::new(db, transport, secrets);
        // Seed a minimal AppSettings so query_balance can read the base_url.
        let settings = AppSettings {
            catalog: ModelCatalog::auto_select(
                deepagent_models::DEEPSEEK_BASE_URL.to_string(),
                vec![
                    deepagent_models::ModelInfo {
                        input_modalities: None,
                        id: "deepseek-v4-flash".into(),
                        object: "model".into(),
                        owned_by: "deepseek".into(),
                        context_window: None,
                        max_output_tokens: None,
                    },
                    deepagent_models::ModelInfo {
                        input_modalities: None,
                        id: "deepseek-v4-pro".into(),
                        object: "model".into(),
                        owned_by: "deepseek".into(),
                        context_window: None,
                        max_output_tokens: None,
                    },
                ],
            )
            .unwrap(),
            discovered_at: 0,
            pricing_catalog: Default::default(),
            approval_policy: ApprovalPolicy::AlwaysAsk,
            sandbox_mode: SandboxMode::WorkspaceWrite,
            terminal_shell: TerminalShell::default(),
            reasoning_effort: ReasoningEffort::High,
            responses: ResponsesApiSettings::default(),
            permission_rules: PermissionRules::default(),
            hooks_json: String::new(),
            verification_policy: VerificationPolicy::default(),
            web_search: WebSearchSettings::default(),
            vision: VisionSettings::default(),
            tool_search_mode: deepagent_builtins::ToolSearchMode::default(),
            tool_search_auto_threshold_chars: None,
            skill_catalog_enabled: default_skill_catalog_enabled(),
            skill_catalog_char_budget: default_skill_catalog_char_budget(),
            skill_install_ai_review_enabled: default_skill_install_ai_review_enabled(),
            skill_install_ai_review_model: None,
            active_permission_preset: PermissionPreset::default(),
            permission_preset_visibility: PermissionPresetVisibility::default(),
            welcome_name: String::new(),
            autocompact_reserve_tokens: None,
            context_headroom_tokens: None,
            output_style: OutputStyle::default(),
            execution_features: ExecutionFeatures::default(),
            language: String::new(),
        };
        svc.save(&settings).unwrap();

        let dto = svc.query_balance().await.unwrap();
        assert!(dto.is_available);
        assert_eq!(dto.infos.len(), 1);
        let info = &dto.infos[0];
        assert_eq!(info.currency, "CNY");
        assert_eq!(info.total_balance, "42.50");
        assert_eq!(info.granted_balance, "2.50");
        assert_eq!(info.topped_up_balance, "40.00");
    }
}
#[test]
fn responses_developer_json_rejects_system_fields_and_unknowns() {
    assert!(ResponsesApiSettings::from_developer_json(r#"{"model":"x"}"#).is_err());
    assert!(ResponsesApiSettings::from_developer_json(r#"{"unknown":true}"#).is_err());
}

#[test]
fn responses_developer_json_preserves_ineffective_fields() {
    let settings = ResponsesApiSettings::from_developer_json(
        r#"{"temperature":0.4,"reasoning":{"effort":"high"},"user":"account-1","parallel_tool_calls":false,"store":true}"#,
    )
    .unwrap();
    assert_eq!(settings.temperature, None);
    assert_eq!(settings.effective_temperature(), Some(0.4));
    assert_eq!(settings.developer["temperature"], 0.4);
    assert_eq!(
        settings.effective_reasoning_effort(),
        Some(ReasoningEffort::High)
    );
    assert_eq!(settings.effective_user().as_deref(), Some("account-1"));
    assert!(settings.ineffective.contains_key("parallel_tool_calls"));
    assert!(settings.ineffective.contains_key("store"));
}

#[test]
fn responses_developer_json_uses_official_nested_reasoning_shape() {
    assert!(ResponsesApiSettings::from_developer_json(r#"{"reasoning_effort":"high"}"#).is_err());
    assert!(
        ResponsesApiSettings::from_developer_json(r#"{"reasoning":{"effort":"extreme"}}"#).is_err()
    );
    assert!(ResponsesApiSettings::from_developer_json(
        r#"{"reasoning":{"effort":"high","summary":"auto"}}"#
    )
    .is_err());
}

#[test]
fn reasoning_effort_rejects_noncanonical_aliases() {
    for value in ["minimal", "medium", "xhigh", "simple", "deep"] {
        assert!(ResponsesApiSettings {
            reasoning_effort: Some(value.to_string()),
            ..ResponsesApiSettings::default()
        }
        .validate()
        .is_err());
        assert!(
            ResponsesApiSettings::from_developer_json(&format!(
                r#"{{"reasoning":{{"effort":"{value}"}}}}"#
            ))
            .is_err(),
            "developer JSON must reject {value}"
        );
        assert!(
            ReasoningEffort::parse(value).is_none(),
            "must reject {value}"
        );
    }
    for value in ["none", "low", "high", "max"] {
        assert!(ResponsesApiSettings::from_developer_json(&format!(
            r#"{{"reasoning":{{"effort":"{value}"}}}}"#
        ))
        .is_ok());
        assert!(ReasoningEffort::parse(value).is_some());
    }
}

#[test]
fn migration_persists_removal_of_malformed_legacy_reasoning_field() {
    let mut stored = serde_json::json!({"thinking_depth": true});
    assert_eq!(migrate_persisted_reasoning(&mut stored), 1);
    assert!(stored.get("thinking_depth").is_none());
}

#[test]
fn responses_user_identifier_validates_both_settings_surfaces() {
    let max_length = "a".repeat(512);
    for user in ["A1-_z", max_length.as_str()] {
        assert!(ResponsesApiSettings {
            user: Some(user.to_string()),
            ..ResponsesApiSettings::default()
        }
        .validate()
        .is_ok());
        assert!(ResponsesApiSettings::from_developer_json(
            &serde_json::json!({"user": user}).to_string()
        )
        .is_ok());
    }

    let too_long = "a".repeat(513);
    for user in [
        "",
        "account 1",
        "account/1",
        "用户-1",
        "account\n1",
        too_long.as_str(),
    ] {
        assert!(
            ResponsesApiSettings {
                user: Some(user.to_string()),
                ..ResponsesApiSettings::default()
            }
            .validate()
            .is_err(),
            "ordinary settings must reject {user:?}"
        );
        assert!(
            ResponsesApiSettings::from_developer_json(
                &serde_json::json!({"user": user}).to_string()
            )
            .is_err(),
            "developer JSON must reject {user:?}"
        );
    }
}

#[tokio::test]
async fn load_migrates_legacy_reasoning_and_removes_noncanonical_values() {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let svc = SettingsService::new(
        db.clone(),
        Arc::new(deepagent_models::transport::MockTransport::with_get_json(
            r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(crate::secret_store::MemorySecretStore::new()),
    );
    svc.initialize("sk-synthetic-test").await.unwrap();
    let store = DocumentStore::new(&svc.db);
    let doc = store
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap();
    let mut stored: serde_json::Value = serde_json::from_str(&doc.body).unwrap();
    stored["thinking_depth"] = serde_json::json!("medium");
    stored["reasoning_effort"] = serde_json::json!("deep");
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
    assert_eq!(loaded.reasoning_effort, ReasoningEffort::High);
    assert_eq!(loaded.responses.reasoning_effort.as_deref(), Some("high"));
    assert!(!loaded.responses.developer.contains_key("reasoning"));
    assert_eq!(loaded.responses.effective_temperature(), Some(0.4));

    let cleaned = store
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap();
    let cleaned: serde_json::Value = serde_json::from_str(&cleaned.body).unwrap();
    assert!(cleaned.get("thinking_depth").is_none());
    assert_eq!(cleaned["reasoning_effort"], "high");
    assert_eq!(cleaned["responses"]["reasoning_effort"], "high");
    assert!(cleaned["responses"]["developer"].get("reasoning").is_none());
    assert_eq!(cleaned["responses"]["developer"]["temperature"], 0.4);
}

#[test]
fn responses_tool_choice_settings_use_only_supported_shapes_and_shared_names() {
    for choice in [
        serde_json::json!({"type":"custom","name":"apply_patch"}),
        serde_json::json!({"type":"web_search"}),
        serde_json::json!({"type":"function","name":""}),
        serde_json::json!({"type":"function","name":"private/value"}),
        serde_json::json!({"type":"function","name":"x".repeat(129)}),
        serde_json::json!({"type":"function","name":"read_file","extra":true}),
        serde_json::json!({"type":"function","function":{"name":"read_file"}}),
        serde_json::json!(true),
    ] {
        let settings = ResponsesApiSettings {
            tool_choice: Some(choice.clone()),
            ..ResponsesApiSettings::default()
        };
        assert!(settings.validate().is_err());
        assert!(ResponsesApiSettings::from_developer_json(
            &serde_json::json!({"tool_choice":choice}).to_string()
        )
        .is_err());
    }
    for choice in [
        serde_json::json!("auto"),
        serde_json::json!("none"),
        serde_json::json!("required"),
        serde_json::json!({"type":"function","name":"read_file"}),
        serde_json::json!({"type":"function","name":"x".repeat(128)}),
    ] {
        ResponsesApiSettings {
            tool_choice: Some(choice.clone()),
            ..ResponsesApiSettings::default()
        }
        .validate()
        .unwrap();
        let settings = ResponsesApiSettings::from_developer_json(
            &serde_json::json!({"tool_choice":choice}).to_string(),
        )
        .unwrap();
        assert_eq!(settings.effective_tool_choice(), Some(choice));
    }
}

#[test]
fn responses_developer_json_validates_numeric_ranges() {
    assert!(ResponsesApiSettings::from_developer_json(r#"{"temperature":2.1}"#).is_err());
    assert!(ResponsesApiSettings::from_developer_json(r#"{"temperature":"high"}"#).is_err());
    assert!(ResponsesApiSettings::from_developer_json(r#"{"top_p":1.1}"#).is_err());
    assert!(ResponsesApiSettings::from_developer_json(r#"{"top_p":true}"#).is_err());
    assert!(ResponsesApiSettings::from_developer_json(r#"{"max_output_tokens":0}"#).is_err());
    assert!(ResponsesApiSettings::from_developer_json(r#"{"top_logprobs":21}"#).is_err());
}

#[test]
fn responses_developer_json_rejects_nested_unknowns() {
    assert!(ResponsesApiSettings::from_developer_json(
        r#"{"text":{"format":{"type":"json_schema","name":"answer","schema":{},"unknown":true}}}"#
    )
    .is_err());
    assert!(ResponsesApiSettings::from_developer_json(
        r#"{"tool_choice":{"type":"function","name":"read_file","unknown":true}}"#
    )
    .is_err());
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
