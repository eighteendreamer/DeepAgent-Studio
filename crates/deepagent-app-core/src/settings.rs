//! Project settings & initialization.
//!
//! The product flow: at project init the user supplies **only an API key**. The
//! system (base URL hard-coded to DeepSeek) discovers available models, auto-
//! selects the latest ones into a [`ModelCatalog`], and persists everything.
//! That persisted model configuration is the key the rest of the system reads
//! to know which model to call.
//!
//! **Credential storage:** the API key is NOT written to the SQLite database.
//! It goes to a [`SecretStore`] (OS keychain in production), keeping plaintext
//! secrets off disk — mirroring Claude Code. Only the public [`ModelCatalog`]
//! is persisted in the `documents` table (collection `"settings"`, id `"app"`).

use std::sync::Arc;

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

/// Persisted, **non-secret** application settings (safe to store on disk).
/// The API key is intentionally absent — it lives in the [`SecretStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Post-edit verification policy (Phase 4C of coding-amplifier spec).
    /// Controls whether failed verifications stay informative reminders or
    /// flip the tool result's `ok` flag to drive automatic retry.
    #[serde(default)]
    pub verification_policy: VerificationPolicy,
}

/// A redacted view of settings safe to send to the UI (no secret material).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Current DeepSeek Thinking Mode depth (simple / medium / deep).
    pub thinking_depth: String,
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
            permission_rules: prior
                .as_ref()
                .map(|s| s.permission_rules.clone())
                .unwrap_or_default(),
            hooks_json: prior
                .as_ref()
                .map(|s| s.hooks_json.clone())
                .unwrap_or_default(),
            thinking_depth: prior.as_ref().map(|s| s.thinking_depth).unwrap_or_default(),
            verification_policy: prior
                .as_ref()
                .map(|s| s.verification_policy)
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
        let store = DocumentStore::new(&self.db);
        match store.get(SETTINGS_COLLECTION, SETTINGS_ID)? {
            Some(doc) => Ok(Some(serde_json::from_str(&doc.body)?)),
            None => Ok(None),
        }
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

    /// The current DeepSeek Thinking Mode depth.
    pub fn thinking_depth(&self) -> Result<ThinkingDepth> {
        Ok(self.load()?.map(|s| s.thinking_depth).unwrap_or_default())
    }

    /// The current post-edit verification policy.
    pub fn verification_policy(&self) -> Result<VerificationPolicy> {
        Ok(self
            .load()?
            .map(|s| s.verification_policy)
            .unwrap_or_default())
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
            thinking_depth: settings.thinking_depth.label().to_string(),
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
        )
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
    async fn initialize_stores_key_in_secret_store_not_db() {
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
        assert!(!json.contains("api_key"));
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
                    },
                    deepagent_models::ModelInfo {
                        id: "deepseek-v4-pro".into(),
                        object: "model".into(),
                        owned_by: "deepseek".into(),
                    },
                ],
            )
            .unwrap(),
            discovered_at: 0,
            approval_policy: ApprovalPolicy::AlwaysAsk,
            sandbox_mode: SandboxMode::WorkspaceWrite,
            thinking_depth: ThinkingDepth::Medium,
            permission_rules: PermissionRules::default(),
            hooks_json: String::new(),
            verification_policy: VerificationPolicy::default(),
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
