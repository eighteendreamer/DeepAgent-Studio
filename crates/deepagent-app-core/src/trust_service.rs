//! Per-project workspace trust (§6.2, main source Claude Code).
//!
//! Claude Code shows a trust dialog the first time a project directory is
//! entered; until the user confirms, nothing that can execute arbitrary code
//! runs automatically (`config.ts::checkHasTrustDialogAccepted`,
//! `auth.ts` gating apiKeyHelper/awsAuthRefresh on trust). Trust is stored per
//! project path in the global config and resolved by walking up parent
//! directories — trusting a directory implies trust for its children.
//!
//! This module ports that model:
//! - [`TrustService`] persists the set of trusted (canonicalized) project roots
//!   in the document store and answers [`TrustService::is_trusted`] by the same
//!   ancestor-walk (a trusted ancestor trusts all descendants).
//! - [`TrustGuardHook`] is a `BeforeToolUse` hook that, in an **untrusted**
//!   project, escalates `bash`/`shell` to approval (`Ask`) — so even
//!   allow-listed commands that would otherwise auto-run wait for the user,
//!   satisfying "未信任项目禁自动 bash".
//!
//! # Safety posture
//!
//! - **Ask, never Deny** — the gate raises to approval, never hard-blocks
//!   (误杀更糟 baseline); once the user trusts the project it disappears.
//! - **Enforcement is opt-in** via `DEEPAGENT_PROJECT_TRUST` (default off) so a
//!   build without the trust-granting UI never strands the user in a
//!   can't-run-anything state. The store + `set_trusted` API are always
//!   available for the UI/command layer to build on.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::Result;
use deepagent_hooks::{DecisionSource, Hook, HookContext, HookData, HookOutcome};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::Database;

/// Document-store location for the trust set.
const TRUST_COLLECTION: &str = "trust";
const TRUST_ID: &str = "projects";
/// Environment flag enabling trust *enforcement* (the gate). The store and
/// grant API work regardless; only the `BeforeToolUse` escalation is gated.
const ENFORCE_ENV: &str = "DEEPAGENT_PROJECT_TRUST";

/// Whether untrusted-project bash escalation is enforced (env-gated).
pub fn project_trust_enforced() -> bool {
    std::env::var(ENFORCE_ENV)
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes" || v == "on"
        })
        .unwrap_or(false)
}

/// Persisted trust set: canonicalized project-root path strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct TrustState {
    #[serde(default)]
    trusted: BTreeSet<String>,
}

/// UI-facing trust status for one project path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectTrustDto {
    /// The queried project path (as given).
    pub project: String,
    /// Whether the project (or a trusted ancestor) is trusted.
    pub trusted: bool,
    /// Whether trust enforcement is currently active (env-gated). When false,
    /// the gate is not applied even for untrusted projects.
    pub enforced: bool,
}

/// Normalize a path for stable comparison: canonicalize when the path exists
/// (resolves symlinks + relative segments), else fall back to the given path.
/// Both stored roots and queried paths go through this so the ancestor walk
/// compares like-for-like.
fn normalize(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Per-project trust store + resolver over the document store.
#[derive(Clone)]
pub struct TrustService {
    db: Arc<Database>,
}

impl TrustService {
    /// Build over the shared database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn load(&self) -> TrustState {
        let store = DocumentStore::new(&self.db);
        match store.get(TRUST_COLLECTION, TRUST_ID) {
            Ok(Some(doc)) => serde_json::from_str(&doc.body).unwrap_or_default(),
            _ => TrustState::default(),
        }
    }

    fn save(&self, state: &TrustState) -> Result<()> {
        let store = DocumentStore::new(&self.db);
        let body = serde_json::to_string(state)?;
        store.put(TRUST_COLLECTION, TRUST_ID, &body, None, SystemClock.now())
    }

    /// Whether `path` (or any ancestor) is trusted. Mirrors Claude Code's
    /// parent-directory traversal: a trusted ancestor trusts all descendants.
    pub fn is_trusted(&self, path: &Path) -> bool {
        let state = self.load();
        if state.trusted.is_empty() {
            return false;
        }
        let normalized = normalize(path);
        PathBuf::from(&normalized).ancestors().any(|ancestor| {
            state
                .trusted
                .contains(&ancestor.to_string_lossy().to_string())
        })
    }

    /// Mark `path` trusted (idempotent). Trusting a directory trusts its
    /// descendants via [`Self::is_trusted`]'s ancestor walk.
    pub fn set_trusted(&self, path: &Path) -> Result<()> {
        let mut state = self.load();
        if state.trusted.insert(normalize(path)) {
            self.save(&state)?;
        }
        Ok(())
    }

    /// Revoke trust for exactly `path` (does not affect ancestors/descendants
    /// trusted under their own entries).
    pub fn set_untrusted(&self, path: &Path) -> Result<()> {
        let mut state = self.load();
        if state.trusted.remove(&normalize(path)) {
            self.save(&state)?;
        }
        Ok(())
    }

    /// Status DTO for a project path (trusted + whether enforcement is active).
    pub fn status(&self, path: &Path) -> ProjectTrustDto {
        ProjectTrustDto {
            project: path.to_string_lossy().to_string(),
            trusted: self.is_trusted(path),
            enforced: project_trust_enforced(),
        }
    }

    /// Set trust for `path` and return the resulting status (for the UI/command
    /// layer to render after a grant/revoke).
    pub fn set_and_status(&self, path: &Path, trusted: bool) -> Result<ProjectTrustDto> {
        if trusted {
            self.set_trusted(path)?;
        } else {
            self.set_untrusted(path)?;
        }
        Ok(self.status(path))
    }

    /// All explicitly-trusted project roots (normalized), sorted. Powers the
    /// settings revoke list — each entry is a directory the user granted trust
    /// to (descendants are trusted implicitly and are NOT listed).
    pub fn list_trusted(&self) -> Vec<String> {
        self.load().trusted.into_iter().collect()
    }
}

/// `BeforeToolUse` hook that escalates `bash`/`shell` to approval while the
/// project is untrusted. Additive alongside `BashGuardHook`: its `Ask` raises
/// even an allow-listed command that would otherwise auto-run. Registered only
/// when enforcement is on and the project is untrusted.
#[derive(Debug, Default)]
pub struct TrustGuardHook;

impl TrustGuardHook {
    /// Build the gate.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Hook for TrustGuardHook {
    fn name(&self) -> &str {
        "project_trust_guard"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        let HookData::Tool { name, .. } = &ctx.data else {
            return Ok(HookOutcome::Continue);
        };
        if name != "bash" && name != "shell" {
            return Ok(HookOutcome::Continue);
        }
        Ok(HookOutcome::ask_from(
            "This project is not yet trusted. Review the workspace, then approve to run this \
             command. Trust the project to stop being asked for every command."
                .to_string(),
            DecisionSource::Policy,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::id::SessionId;
    use deepagent_hooks::HookPoint;

    fn db() -> Arc<Database> {
        Arc::new(Database::open_in_memory().unwrap())
    }

    #[test]
    fn untrusted_by_default_then_trust_and_descendants() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proj");
        let sub = root.join("crates").join("core");
        std::fs::create_dir_all(&sub).unwrap();

        let svc = TrustService::new(db());
        assert!(!svc.is_trusted(&root), "fresh project is untrusted");
        assert!(!svc.is_trusted(&sub));

        svc.set_trusted(&root).unwrap();
        assert!(svc.is_trusted(&root), "trusted after set_trusted");
        // Trusting the root trusts a descendant (parent-traversal semantics).
        assert!(svc.is_trusted(&sub), "descendant inherits ancestor trust");
    }

    #[test]
    fn revoke_trust() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("p");
        std::fs::create_dir_all(&root).unwrap();
        let svc = TrustService::new(db());
        svc.set_trusted(&root).unwrap();
        assert!(svc.is_trusted(&root));
        svc.set_untrusted(&root).unwrap();
        assert!(!svc.is_trusted(&root));
    }

    #[test]
    fn list_trusted_reflects_grants_and_revokes() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let svc = TrustService::new(db());
        assert!(svc.list_trusted().is_empty(), "empty by default");
        svc.set_trusted(&a).unwrap();
        svc.set_trusted(&b).unwrap();
        let listed = svc.list_trusted();
        assert_eq!(listed.len(), 2, "both grants listed");
        assert!(listed.iter().any(|p| p.ends_with("a")));
        assert!(listed.iter().any(|p| p.ends_with("b")));
        // Descendants are implicitly trusted but NOT listed as explicit grants.
        let sub = a.join("child");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(svc.is_trusted(&sub));
        assert!(!svc.list_trusted().iter().any(|p| p.ends_with("child")));
        // Revoke removes it from the list.
        svc.set_untrusted(&a).unwrap();
        let listed = svc.list_trusted();
        assert_eq!(listed.len(), 1);
        assert!(listed.iter().any(|p| p.ends_with("b")));
    }

    #[test]
    fn sibling_is_not_trusted() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let svc = TrustService::new(db());
        svc.set_trusted(&a).unwrap();
        assert!(svc.is_trusted(&a));
        assert!(!svc.is_trusted(&b), "sibling must not inherit trust");
    }

    fn tool_ctx(name: &str) -> HookContext {
        HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool(name, serde_json::json!({"command": "cargo test"})),
        )
    }

    #[tokio::test]
    async fn trust_guard_asks_on_bash_and_ignores_others() {
        let guard = TrustGuardHook::new();
        assert!(guard.run(&tool_ctx("bash")).await.unwrap().is_ask());
        assert!(guard.run(&tool_ctx("shell")).await.unwrap().is_ask());
        assert_eq!(
            guard.run(&tool_ctx("read_file")).await.unwrap(),
            HookOutcome::Continue
        );
    }

    #[test]
    fn enforcement_env_parsing() {
        // Not set → not enforced (default preserves current UX).
        std::env::remove_var(ENFORCE_ENV);
        assert!(!project_trust_enforced());
    }
}
