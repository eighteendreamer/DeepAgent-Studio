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

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use deepagent_core::clock::Timestamp;
use deepagent_core::error::{CoreError, Result};
use deepagent_hooks::PermissionRules;
use deepagent_models::balance::{fetch_balance, BalanceResponse};
use deepagent_models::discovery::{ModelCatalog, ModelDiscovery};
use deepagent_models::transport::HttpTransport;
use deepagent_models::ThinkingDepth;
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;

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
const ANYSEARCH_DEFAULT_BASE_URL: &str = "https://api.anysearch.com";

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

/// Preferred backend for the `web_search` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WebSearchProvider {
    /// Use DeepSeek server web-search first, then configured fallbacks.
    #[default]
    DeepSeekFirst,
    /// Use the configured SearXNG bridge first, then DuckDuckGo.
    Searxng,
    /// Use DuckDuckGo HTML only.
    DuckDuckGo,
}

impl WebSearchProvider {
    /// Stable wire label.
    pub const fn label(&self) -> &'static str {
        match self {
            WebSearchProvider::DeepSeekFirst => "deepseek_first",
            WebSearchProvider::Searxng => "searxng",
            WebSearchProvider::DuckDuckGo => "duckduckgo",
        }
    }

    /// Parse from a wire string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "deepseek_first" | "deepseek" => Some(Self::DeepSeekFirst),
            "searxng" => Some(Self::Searxng),
            "duckduckgo" => Some(Self::DuckDuckGo),
            _ => None,
        }
    }
}

/// Persisted, non-secret configuration for `web_search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchSettings {
    /// Whether the `web_search` tool is registered for chat runs.
    #[serde(default = "default_web_search_enabled")]
    pub enabled: bool,
    /// Preferred provider chain.
    #[serde(default)]
    pub provider: WebSearchProvider,
    /// Optional SearXNG base URL, for example `https://search.example.com`.
    #[serde(default)]
    pub searxng_url: Option<String>,
    /// Whether AnySearch should be attempted before the selected fallback chain.
    #[serde(default)]
    pub anysearch_enabled: bool,
    /// Optional AnySearch API base URL. Defaults to `https://api.anysearch.com`.
    #[serde(default)]
    pub anysearch_base_url: Option<String>,
    /// Redacted view-only flag, populated by [`SettingsService`].
    #[serde(default)]
    pub anysearch_api_key_configured: bool,
}

impl Default for WebSearchSettings {
    fn default() -> Self {
        Self {
            enabled: default_web_search_enabled(),
            provider: WebSearchProvider::default(),
            searxng_url: None,
            anysearch_enabled: false,
            anysearch_base_url: None,
            anysearch_api_key_configured: false,
        }
    }
}

fn default_web_search_enabled() -> bool {
    true
}

fn normalize_web_search_settings(mut settings: WebSearchSettings) -> WebSearchSettings {
    settings.searxng_url = settings
        .searxng_url
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty());
    settings.anysearch_base_url = settings
        .anysearch_base_url
        .map(|s| normalize_anysearch_base_url(&s))
        .filter(|s| !s.is_empty());
    settings.anysearch_api_key_configured = false;
    settings
}

fn normalize_anysearch_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    if matches!(
        trimmed,
        "https://www.anysearch.com" | "https://anysearch.com"
    ) {
        ANYSEARCH_DEFAULT_BASE_URL.to_string()
    } else {
        trimmed.to_string()
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

    pub fn effective_reasoning_effort(&self) -> Option<String> {
        self.developer
            .get("reasoning")
            .and_then(|value| value.get("effort"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| self.reasoning_effort.clone())
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
            if !matches!(value.as_str(), "low" | "medium" | "high" | "max") {
                return Err(CoreError::invalid(
                    "Responses reasoning_effort must be low, medium, high, or max",
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
        if self.user.as_deref().is_some_and(str::is_empty) {
            return Err(CoreError::invalid("Responses user must not be empty"));
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
            Some("low" | "medium" | "high" | "max")
        ) {
            return Err(CoreError::invalid(
                "Responses reasoning.effort must be low, medium, high, or max",
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
        let valid = choice
            .as_str()
            .is_some_and(|value| matches!(value, "auto" | "none" | "required"))
            || choice.as_object().is_some_and(|choice| {
                choice
                    .keys()
                    .all(|key| matches!(key.as_str(), "type" | "name"))
                    && matches!(
                        choice.get("type").and_then(serde_json::Value::as_str),
                        Some("function" | "custom" | "web_search")
                    )
                    && (choice.get("type").and_then(serde_json::Value::as_str)
                        == Some("web_search")
                        || choice
                            .get("name")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|name| !name.is_empty()))
            });
        if !valid {
            return Err(CoreError::invalid(
                "Responses tool_choice must use the supported string or typed object shape",
            ));
        }
    }
    if object
        .get("user")
        .is_some_and(|value| value.as_str().map(str::is_empty).unwrap_or(true))
    {
        return Err(CoreError::invalid(
            "Responses user must be a non-empty string",
        ));
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VisionMode {
    /// Do not analyze images automatically.
    Off,
    /// Use the configured third-party vision API, then pass the text result to
    /// the main chat model.
    #[default]
    System,
    /// Use a provider model that explicitly supports image input.
    Model,
}

impl VisionMode {
    pub const fn label(&self) -> &'static str {
        match self {
            VisionMode::Off => "off",
            VisionMode::System => "system",
            VisionMode::Model => "model",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "system" => Some(Self::System),
            "model" => Some(Self::Model),
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
    #[serde(default)]
    pub send_original_image_to_model: bool,
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
            send_original_image_to_model: false,
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
    /// User-selected DeepSeek Thinking Mode depth.
    #[serde(default)]
    pub thinking_depth: ThinkingDepth,
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
    /// threshold). `None` uses the Claude-Code-aligned default (13_000; see
    /// `ContextPolicy::autocompact_threshold_tokens`). Persisted as `Option`
    /// so older settings docs round-trip cleanly.
    #[serde(default)]
    pub autocompact_reserve_tokens: Option<usize>,
    /// Built-in system-prompt output style (Claude Code output styles §7.1).
    /// `Default` injects nothing; `Explanatory`/`Learning` add a stable,
    /// DeepSeek-worded style block. `#[serde(default)]` → `Default` for older
    /// settings docs without the field.
    #[serde(default)]
    pub output_style: OutputStyle,
    /// Opt-in advanced execution safeguards (§2.2/§2.3/§6.1/§6.2). Each is a
    /// self-created "从宽" guard (advisory/fail-open) that is OFF by default;
    /// this is the user-facing surface so they can be toggled without setting
    /// environment variables before launch. The matching `DEEPAGENT_*` env
    /// vars still force-enable regardless (power-user / CI override).
    #[serde(default)]
    pub execution_features: ExecutionFeatures,
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
    /// Current DeepSeek Thinking Mode depth (simple / medium / deep).
    pub thinking_depth: String,
    #[serde(default)]
    pub responses: ResponsesApiSettings,
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
    settings_cache: Mutex<Option<Option<AppSettings>>>,
}

impl SettingsService {
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
        // Discover first so we never store a key that doesn't work.
        let discovery = ModelDiscovery::deepseek(self.transport.clone());
        let catalog = discovery.discover(api_key).await?;

        // Key → secret store (NOT the database).
        self.secrets.set(API_KEY_NAME, api_key)?;

        // Public catalog → database.
        let prior = self.load().ok().flatten();
        let settings = AppSettings {
            catalog,
            discovered_at: now_ms(),
            approval_policy: prior
                .as_ref()
                .map(|s| s.approval_policy)
                .unwrap_or_default(),
            sandbox_mode: prior.as_ref().map(|s| s.sandbox_mode).unwrap_or_default(),
            terminal_shell: prior.as_ref().map(|s| s.terminal_shell).unwrap_or_default(),
            permission_rules: prior
                .as_ref()
                .map(|s| s.permission_rules.clone())
                .unwrap_or_default(),
            hooks_json: prior
                .as_ref()
                .map(|s| s.hooks_json.clone())
                .unwrap_or_default(),
            thinking_depth: prior.as_ref().map(|s| s.thinking_depth).unwrap_or_default(),
            responses: prior
                .as_ref()
                .map(|s| s.responses.clone())
                .unwrap_or_default(),
            verification_policy: prior
                .as_ref()
                .map(|s| s.verification_policy)
                .unwrap_or_default(),
            web_search: prior
                .as_ref()
                .map(|s| normalize_web_search_settings(s.web_search.clone()))
                .unwrap_or_default(),
            vision: prior.as_ref().map(|s| s.vision.clone()).unwrap_or_default(),
            tool_search_mode: prior
                .as_ref()
                .map(|s| s.tool_search_mode)
                .unwrap_or(Self::DEFAULT_TOOL_SEARCH_MODE),
            tool_search_auto_threshold_chars: prior
                .as_ref()
                .and_then(|s| s.tool_search_auto_threshold_chars),
            skill_catalog_enabled: prior
                .as_ref()
                .map(|s| s.skill_catalog_enabled)
                .unwrap_or_else(default_skill_catalog_enabled),
            skill_catalog_char_budget: prior
                .as_ref()
                .map(|s| s.skill_catalog_char_budget)
                .unwrap_or_else(default_skill_catalog_char_budget),
            skill_install_ai_review_enabled: prior
                .as_ref()
                .map(|s| s.skill_install_ai_review_enabled)
                .unwrap_or_else(default_skill_install_ai_review_enabled),
            skill_install_ai_review_model: prior
                .as_ref()
                .and_then(|s| s.skill_install_ai_review_model.clone()),
            active_permission_preset: prior
                .as_ref()
                .map(|s| s.active_permission_preset)
                .unwrap_or_default(),
            permission_preset_visibility: prior
                .as_ref()
                .map(|s| s.permission_preset_visibility.clone())
                .unwrap_or_default(),
            welcome_name: prior
                .as_ref()
                .map(|s| s.welcome_name.clone())
                .unwrap_or_default(),
            autocompact_reserve_tokens: prior.as_ref().and_then(|s| s.autocompact_reserve_tokens),
            output_style: prior.as_ref().map(|s| s.output_style).unwrap_or_default(),
            execution_features: prior
                .as_ref()
                .map(|s| s.execution_features)
                .unwrap_or_default(),
        };
        self.save(&settings)?;

        self.view_with_key(Some(api_key), &settings)
    }

    /// Re-run discovery with the stored key (e.g. to pick up new models).
    pub async fn refresh_models(&self) -> Result<SettingsView> {
        let key = self
            .secrets
            .get(API_KEY_NAME)?
            .ok_or_else(|| CoreError::not_found("API key not set; initialize first"))?;
        self.initialize(&key).await
    }

    /// Manually override which model fills a role (must be a discovered id).
    pub fn set_model(
        &self,
        role: deepagent_models::ModelRole,
        model_id: &str,
    ) -> Result<SettingsView> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        if !settings.catalog.has_model(model_id) {
            return Err(CoreError::invalid(format!(
                "model '{model_id}' is not in the discovered set"
            )));
        }
        match role {
            deepagent_models::ModelRole::Chat => settings.catalog.chat_model = model_id.to_string(),
            deepagent_models::ModelRole::Reasoner => {
                settings.catalog.reasoner_model = model_id.to_string()
            }
        }
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Load the public settings (catalog) from the database.
    pub fn load(&self) -> Result<Option<AppSettings>> {
        if let Some(cached) = self
            .settings_cache
            .lock()
            .map_err(|_| CoreError::other("settings cache lock poisoned"))?
            .clone()
        {
            return Ok(cached);
        }

        let store = DocumentStore::new(&self.db);
        let loaded = match store.get(SETTINGS_COLLECTION, SETTINGS_ID)? {
            Some(doc) => Some(serde_json::from_str::<AppSettings>(&doc.body)?),
            None => None,
        };
        *self
            .settings_cache
            .lock()
            .map_err(|_| CoreError::other("settings cache lock poisoned"))? = Some(loaded.clone());
        Ok(loaded)
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

        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.welcome_name = name.to_string();
        self.save(&settings)?;
        Ok(settings.welcome_name)
    }

    /// The raw API key from the secret store (for internal runtime use only).
    pub fn api_key(&self) -> Result<Option<String>> {
        self.secrets.get(API_KEY_NAME)
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
            .secrets
            .get(API_KEY_NAME)?
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
    pub fn thinking_depth(&self) -> Result<ThinkingDepth> {
        Ok(self.load()?.map(|s| s.thinking_depth).unwrap_or_default())
    }

    /// Effective proactive auto-compact reserve buffer override. Precedence:
    /// `DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS` env var (operator escape hatch,
    /// aligned with Claude Code's env overrides) > persisted setting > `None`
    /// (the caller then uses the CC-aligned 13k default).
    pub fn autocompact_reserve_tokens(&self) -> Option<usize> {
        if let Ok(raw) = std::env::var("DEEPAGENT_AUTOCOMPACT_RESERVE_TOKENS") {
            if let Ok(parsed) = raw.trim().parse::<usize>() {
                return Some(parsed);
            }
        }
        self.load()
            .ok()
            .flatten()
            .and_then(|s| s.autocompact_reserve_tokens)
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.output_style = style;
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.execution_features = features;
        self.save(&settings)?;
        Ok(())
    }

    /// Set the post-edit verification policy, persisting it.
    pub fn set_verification_policy(&self, policy: VerificationPolicy) -> Result<()> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.verification_policy = policy;
        self.save(&settings)?;
        Ok(())
    }

    /// Current web-search settings (default DeepSeek-first when uninitialized).
    pub fn web_search_settings(&self) -> Result<WebSearchSettings> {
        let settings =
            normalize_web_search_settings(self.load()?.map(|s| s.web_search).unwrap_or_default());
        self.web_search_settings_view(settings)
    }

    /// Persist web-search settings.
    pub fn set_web_search_settings(&self, settings_next: WebSearchSettings) -> Result<()> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.web_search = normalize_web_search_settings(settings_next);
        self.save(&settings)?;
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

    fn web_search_settings_view(
        &self,
        mut settings: WebSearchSettings,
    ) -> Result<WebSearchSettings> {
        settings.anysearch_api_key_configured = self
            .secrets
            .get(ANYSEARCH_API_KEY_NAME)?
            .map(|key| !key.trim().is_empty())
            .unwrap_or(false);
        Ok(settings)
    }

    /// Current image-analysis settings.
    pub fn vision_settings(&self) -> Result<VisionSettings> {
        let settings = self.load()?.map(|s| s.vision).unwrap_or_default();
        self.vision_settings_view(settings)
    }

    /// Set image-analysis settings.
    pub fn set_vision_settings(&self, settings_next: VisionSettings) -> Result<VisionSettings> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        if let Some(api_key) = settings_next.api_key.as_deref() {
            let api_key = api_key.trim();
            if api_key.is_empty() {
                self.secrets.delete(VISION_API_KEY_NAME)?;
            } else {
                self.secrets.set(VISION_API_KEY_NAME, api_key)?;
            }
        }
        settings.vision = normalize_vision_settings(settings_next);
        let saved = settings.vision.clone();
        self.save(&settings)?;
        self.vision_settings_view(saved)
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.tool_search_mode = mode;
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.tool_search_auto_threshold_chars = value.map(|v| v.max(1));
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.skill_catalog_enabled = enabled;
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.skill_catalog_char_budget = budget;
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.skill_install_ai_review_enabled = enabled;
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.skill_install_ai_review_model = model
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty());
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.active_permission_preset = preset;
        let profile = preset.to_effective_profile();
        settings.approval_policy = profile.approval_policy;
        settings.sandbox_mode = profile.sandbox_mode;
        self.save(&settings)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
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
        settings.permission_preset_visibility = visibility;
        self.save(&settings)?;
        Ok(())
    }

    /// Derive the full effective runtime profile from the persisted fields.
    /// Both the preset setter and the legacy setters (set_approval_policy,
    /// set_sandbox_mode) write to the same underlying fields, so reading them
    /// directly is always authoritative.
    pub fn effective_permission_profile(&self) -> Result<EffectivePermissionProfile> {
        let policy = self.approval_policy()?;
        let sandbox = self.sandbox_mode()?;
        let local_execution_mode = if matches!(policy, ApprovalPolicy::FullAccess)
            && matches!(sandbox, SandboxMode::FullAccess)
        {
            LocalExecutionMode::Direct
        } else {
            LocalExecutionMode::SandboxiePreferred
        };
        let network_always_ask = matches!(policy, ApprovalPolicy::AlwaysAsk);
        Ok(EffectivePermissionProfile {
            approval_policy: policy,
            sandbox_mode: sandbox,
            local_execution_mode,
            network_always_ask,
        })
    }

    /// Set the Thinking Mode depth, persisting it. Returns the redacted view.
    pub fn set_thinking_depth(&self, depth: ThinkingDepth) -> Result<SettingsView> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.thinking_depth = depth;
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.responses = settings_next;
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings)
    }

    pub fn set_responses_developer_json(&self, raw: &str) -> Result<SettingsView> {
        let developer = ResponsesApiSettings::from_developer_json(raw)?;
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.responses.developer = developer.developer;
        settings.responses.ineffective = developer.ineffective;
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Set the approval policy, persisting it. Returns the redacted view.
    pub fn set_approval_policy(&self, policy: ApprovalPolicy) -> Result<SettingsView> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.approval_policy = policy;
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Set the sandbox mode, persisting it. Returns the redacted view.
    pub fn set_sandbox_mode(&self, mode: SandboxMode) -> Result<SettingsView> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.sandbox_mode = mode;
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// Set the preferred integrated-terminal shell, persisting it.
    pub fn set_terminal_shell(&self, shell: TerminalShell) -> Result<SettingsView> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.terminal_shell = shell;
        self.save(&settings)?;
        let key = self.secrets.get(API_KEY_NAME)?;
        self.view_with_key(key.as_deref(), &settings)
    }

    /// The current declarative permission rules (empty when uninitialized).
    pub fn permission_rules(&self) -> Result<PermissionRules> {
        Ok(self.load()?.map(|s| s.permission_rules).unwrap_or_default())
    }

    /// Set the declarative permission rules, persisting them.
    pub fn set_permission_rules(&self, rules: PermissionRules) -> Result<()> {
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.permission_rules = rules;
        self.save(&settings)
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
        let mut settings = self
            .load()?
            .ok_or_else(|| CoreError::not_found("settings not initialized"))?;
        settings.hooks_json = hooks_json.to_string();
        self.save(&settings)
    }

    /// The redacted view for the UI, or `None` if uninitialized.
    pub fn view(&self) -> Result<Option<SettingsView>> {
        let Some(settings) = self.load()? else {
            return Ok(None);
        };
        let key = self.secrets.get(API_KEY_NAME)?;
        Ok(Some(self.view_with_key(key.as_deref(), &settings)?))
    }

    /// Clear the stored key (sign-out). Leaves the catalog in place.
    pub fn clear_key(&self) -> Result<()> {
        self.secrets.delete(API_KEY_NAME)
    }

    fn view_with_key(&self, key: Option<&str>, settings: &AppSettings) -> Result<SettingsView> {
        Ok(SettingsView {
            api_key_masked: key.map(mask_key).unwrap_or_else(|| "(not set)".to_string()),
            base_url: settings.catalog.base_url.clone(),
            available_models: settings
                .catalog
                .available
                .iter()
                .map(|m| m.id.clone())
                .collect(),
            chat_model: settings.catalog.chat_model.clone(),
            reasoner_model: settings.catalog.reasoner_model.clone(),
            configured: key.map(|k| !k.trim().is_empty()).unwrap_or(false)
                && !settings.catalog.available.is_empty(),
            approval_policy: settings.approval_policy.label().to_string(),
            sandbox_mode: settings.sandbox_mode.label().to_string(),
            terminal_shell: settings.terminal_shell.label().to_string(),
            thinking_depth: settings.thinking_depth.label().to_string(),
            responses: settings.responses.clone(),
            web_search: self.web_search_settings_view(normalize_web_search_settings(
                settings.web_search.clone(),
            ))?,
            vision: self.vision_settings_view(settings.vision.clone())?,
            execution_features: settings.execution_features,
            output_style: settings.output_style.as_str().to_string(),
        })
    }

    fn save(&self, settings: &AppSettings) -> Result<()> {
        let store = DocumentStore::new(&self.db);
        let body = serde_json::to_string(settings)?;
        store.put(
            SETTINGS_COLLECTION,
            SETTINGS_ID,
            &body,
            None,
            Timestamp::from_millis(settings.discovered_at),
        )?;
        *self
            .settings_cache
            .lock()
            .map_err(|_| CoreError::other("settings cache lock poisoned"))? =
            Some(Some(settings.clone()));
        Ok(())
    }
}

fn now_ms() -> i64 {
    use deepagent_core::clock::{Clock, SystemClock};
    SystemClock.now().as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::MemorySecretStore;
    use deepagent_models::transport::MockTransport;

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
        assert_eq!(view.thinking_depth, "medium");
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
    async fn thinking_depth_roundtrips_and_survives_refresh() {
        let (svc, _) = service();
        svc.initialize("sk-abcd1234").await.unwrap();
        let view = svc.set_thinking_depth(ThinkingDepth::Deep).unwrap();
        assert_eq!(view.thinking_depth, "deep");
        assert_eq!(svc.thinking_depth().unwrap(), ThinkingDepth::Deep);
        let refreshed = svc.refresh_models().await.unwrap();
        assert_eq!(refreshed.thinking_depth, "deep");
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
        assert!(view.web_search.enabled);
        assert_eq!(view.web_search.provider, WebSearchProvider::DeepSeekFirst);
        assert!(view.web_search.searxng_url.is_none());

        svc.set_web_search_settings(WebSearchSettings {
            enabled: true,
            provider: WebSearchProvider::Searxng,
            searxng_url: Some(" https://search.example.com/ ".into()),
            anysearch_enabled: true,
            anysearch_base_url: Some(" https://api.anysearch.com/ ".into()),
            anysearch_api_key_configured: false,
        })
        .unwrap();
        let persisted = svc.web_search_settings().unwrap();
        assert_eq!(persisted.provider, WebSearchProvider::Searxng);
        assert_eq!(
            persisted.searxng_url.as_deref(),
            Some("https://search.example.com")
        );

        let refreshed = svc.refresh_models().await.unwrap();
        assert_eq!(refreshed.web_search.provider, WebSearchProvider::Searxng);
        assert_eq!(
            refreshed.web_search.searxng_url.as_deref(),
            Some("https://search.example.com")
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
        assert!(parsed.web_search.enabled);
        assert_eq!(parsed.web_search.provider, WebSearchProvider::DeepSeekFirst);
        assert!(parsed.web_search.searxng_url.is_none());
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
                        id: "deepseek-v4-flash".into(),
                        object: "model".into(),
                        owned_by: "deepseek".into(),
                        context_window: None,
                        max_output_tokens: None,
                    },
                    deepagent_models::ModelInfo {
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
            approval_policy: ApprovalPolicy::AlwaysAsk,
            sandbox_mode: SandboxMode::WorkspaceWrite,
            terminal_shell: TerminalShell::default(),
            thinking_depth: ThinkingDepth::Medium,
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
            output_style: OutputStyle::default(),
            execution_features: ExecutionFeatures::default(),
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
        settings.effective_reasoning_effort().as_deref(),
        Some("high")
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
