//! Bridges runtime tool-approval requests to the UI and back (Phase A-3).
//!
//! The runtime's [`ApprovalGate`] is async: it *awaits* a decision. On the
//! desktop, that decision comes from a human clicking the approval dialog. This
//! module provides:
//!
//! - [`PendingApprovals`] — a shared registry of in-flight requests keyed by
//!   `call_id`, each backed by a `oneshot` channel. Multiple requests can be
//!   pending at once (queued in the UI); each resolves independently. It also
//!   owns the [`SessionGrants`] the human can hand out while answering.
//! - [`ChannelApprovalGate`] — the [`ApprovalGate`] the runtime holds: it first
//!   honors an existing grant (so a granted tool stops prompting), otherwise
//!   registers the pending decision, emits a unified [`ApprovalRequestDto`] to
//!   the UI (via a callback), and awaits the reply.
//! - [`PolicyGate`] — wraps a gate with an [`ApprovalPolicy`]: `AutoReview` /
//!   `FullAccess` resolve automatically (no UI prompt), `AlwaysAsk` delegates to
//!   the inner channel gate.
//!
//! The UI resolves a request by calling [`PendingApprovals::resolve`] with the
//! `call_id` and the user's decision.
//!
//! Grant scope is deliberately narrow: [`ApprovalScope::ToolSession`] keys on
//! `(tool, risk)` with **no arguments**, mirroring codex's
//! `McpToolApprovalKey` (`借鉴/codex/codex-rs/core/src/mcp_tool_call.rs:2246-2253`).
//! It lives in this process only — nothing is persisted, so a restart always asks
//! again. The wider [`ApprovalScope::CommandPrefix`] is persisted, as a
//! `Bash(prefix:*)` permission rule the settings page already lists and can
//! delete; [`PrefixGrants`] reads it back on every prompt, so an edit there
//! bites here immediately.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_builtins::{command_prefix_for, is_allowed, is_command_tool};
use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::{ApprovalDecision, ApprovalGate, ApprovalRequest};
use tokio::sync::oneshot;

use crate::dto::ApprovalRequestDto;
use crate::settings::ApprovalPolicy;

/// How far a "yes" reaches. Persisted on the request as its default; the answer
/// may only widen to a scope the request actually offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalScope {
    /// Approve this one call (the default).
    SingleCall,
    /// Stop asking about this tool at this risk level for the rest of the process.
    ToolSession,
    /// Stop asking about commands with this prefix. Persisted, and only offered
    /// for command-running tools.
    CommandPrefix,
}

impl ApprovalScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SingleCall => "single_call",
            Self::ToolSession => "tool_session",
            Self::CommandPrefix => "command_prefix",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "single_call" => Some(Self::SingleCall),
            "tool_session" => Some(Self::ToolSession),
            "command_prefix" => Some(Self::CommandPrefix),
            _ => None,
        }
    }

    /// Scopes a human may choose for a request.
    ///
    /// One function owns the vocabulary, so the buttons the UI draws and the
    /// validation an answer passes through cannot drift apart. A scope is listed
    /// only once the write behind it exists and the answer would actually be
    /// honored: `command_prefix` appears when the request carries a command a
    /// prefix can stand in for (see [`deepagent_builtins::command_prefix_for`]),
    /// so no button ever offers something that silently does nothing.
    pub fn offered(tool: &str, command: Option<&str>) -> Vec<Self> {
        let mut scopes = vec![Self::SingleCall, Self::ToolSession];
        if is_command_tool(tool) && command.is_some_and(|cmd| command_prefix_for(cmd).is_some()) {
            scopes.push(Self::CommandPrefix);
        }
        scopes
    }

    /// Whether `tool`, asked about `command`, may be granted this scope.
    pub fn offers(scope: Self, tool: &str, command: Option<&str>) -> bool {
        Self::offered(tool, command).contains(&scope)
    }
}

/// Grants handed out for the lifetime of this process.
#[derive(Clone, Default)]
pub struct SessionGrants {
    inner: Arc<Mutex<HashSet<(String, String)>>>,
}

impl SessionGrants {
    /// New empty grant set.
    pub fn new() -> Self {
        Self::default()
    }

    fn key(tool: &str, risk: &str) -> (String, String) {
        (tool.to_lowercase(), risk.to_lowercase())
    }

    /// Remember `(tool, risk)` for this process.
    pub fn grant(&self, tool: &str, risk: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(Self::key(tool, risk));
    }

    /// Whether a matching grant was handed out earlier.
    pub fn contains(&self, tool: &str, risk: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&Self::key(tool, risk))
    }

    /// Drop a grant; returns whether one existed. Used by the settings surface so
    /// a grant is never silently permanent within a session.
    pub fn revoke(&self, tool: &str, risk: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&Self::key(tool, risk))
    }

    /// Current grants, as `(tool, risk)` pairs.
    pub fn list(&self) -> Vec<(String, String)> {
        let mut items: Vec<(String, String)> = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect();
        items.sort();
        items
    }
}

/// Where standing command-prefix allowances are kept.
///
/// The gate reads through on every prompt instead of caching a copy: a rule the
/// user edits or deletes in settings takes effect for the next command, and two
/// hosts in one process cannot drift apart.
pub trait CommandPrefixSource: Send + Sync {
    /// Prefixes the human allowed for `tool`, already minus anything an
    /// `ask`/`deny` rule fences.
    fn prefixes(&self, tool: &str) -> Vec<String>;
    /// Persist one allowance. Errors when it cannot be stored or is fenced.
    fn grant(&self, tool: &str, prefix: &str) -> Result<()>;
}

impl CommandPrefixSource for crate::settings::SettingsService {
    fn prefixes(&self, tool: &str) -> Vec<String> {
        if !is_command_tool(tool) {
            return Vec::new();
        }
        self.command_prefix_grants().unwrap_or_else(|error| {
            // Losing the list must not silently widen what runs: the human just
            // gets asked again, and the read failure is on the log.
            tracing::warn!(error = %error, "could not read command prefix grants; asking again");
            Vec::new()
        })
    }

    fn grant(&self, tool: &str, prefix: &str) -> Result<()> {
        if !is_command_tool(tool) {
            return Err(CoreError::invalid(format!(
                "tool {tool} takes no command, so no prefix can be granted"
            )));
        }
        self.grant_command_prefix(prefix)
    }
}

/// The command prefixes the human allowed to run without being asked again
/// (answering a prompt with [`ApprovalScope::CommandPrefix`]).
#[derive(Clone, Default)]
pub struct PrefixGrants {
    source: Arc<Mutex<Option<Arc<dyn CommandPrefixSource>>>>,
}

impl PrefixGrants {
    /// Grants with nowhere to persist yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Point the grants at their store (the settings service in production).
    pub fn attach(&self, source: Arc<dyn CommandPrefixSource>) {
        *self
            .source
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(source);
    }

    fn source(&self) -> Option<Arc<dyn CommandPrefixSource>> {
        self.source
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Whether a `command_prefix` answer could be honored at all. Offers are only
    /// drawn when the answer would stick.
    pub fn attached(&self) -> bool {
        self.source().is_some()
    }

    /// Whether `request` is covered by an allowance the human already gave.
    ///
    /// A sandbox escalation is *not* covered: granting a command prefix says the
    /// command is fine inside the sandbox, not that it may leave it.
    pub fn covers(&self, request: &ApprovalRequest) -> bool {
        let Some(source) = self.source() else {
            return false;
        };
        if !is_command_tool(&request.tool) || requests_escalation(&request.arguments) {
            return false;
        }
        let Some(command) = command_of(&request.arguments) else {
            return false;
        };
        let prefixes = source.prefixes(&request.tool);
        if prefixes.is_empty() {
            return false;
        }
        is_allowed(command, &prefixes)
    }

    /// Persist the prefix the request asked about, derived from the recorded
    /// arguments — never from anything the answering client sends.
    pub fn grant_for(&self, tool: &str, arguments: &serde_json::Value) -> Result<String> {
        let source = self.source().ok_or_else(|| {
            CoreError::invalid("no prefix store attached, so a command prefix cannot be granted")
        })?;
        let prefix = prefix_of(tool, arguments)?;
        source.grant(tool, &prefix)?;
        Ok(prefix)
    }
}

/// The command line an approval request is about, if any.
fn command_of(arguments: &serde_json::Value) -> Option<&str> {
    arguments.get("command").and_then(|v| v.as_str())
}

/// Whether the model asked to escape the sandbox for this call.
fn requests_escalation(arguments: &serde_json::Value) -> bool {
    arguments
        .get("sandbox_permissions")
        .and_then(|v| v.as_str())
        == Some("require_escalated")
}

/// The prefix a `command_prefix` answer would persist for a request.
fn prefix_of(tool: &str, arguments: &serde_json::Value) -> Result<String> {
    let command = command_of(arguments).ok_or_else(|| {
        CoreError::invalid(format!(
            "tool {tool} carries no command, so no prefix can be granted"
        ))
    })?;
    command_prefix_for(command).ok_or_else(|| {
        CoreError::invalid(format!(
            "no command prefix can stand in for this command, so it cannot be granted \
             for future calls: {command}"
        ))
    })
}

/// One request awaiting a human decision.
struct Pending {
    sender: oneshot::Sender<ApprovalDecision>,
    tool: String,
    risk: String,
    /// Kept so a widening answer's grant is derived from what was actually asked,
    /// not from anything the answering client sends.
    arguments: serde_json::Value,
}

/// Shared registry of in-flight approval requests, keyed by `call_id`.
#[derive(Clone, Default)]
pub struct PendingApprovals {
    inner: Arc<Mutex<HashMap<String, Pending>>>,
    grants: SessionGrants,
    prefixes: PrefixGrants,
}

impl PendingApprovals {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The grants this registry honors (and writes when the human grants).
    pub fn grants(&self) -> SessionGrants {
        self.grants.clone()
    }

    /// The command-prefix allowances this registry reads through and writes.
    pub fn prefixes(&self) -> PrefixGrants {
        self.prefixes.clone()
    }

    /// Register a pending request, returning the receiver to await.
    fn register(&self, request: &ApprovalRequest) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().expect("approvals poisoned").insert(
            request.call_id.clone(),
            Pending {
                sender: tx,
                tool: request.tool.clone(),
                risk: request.risk.clone(),
                arguments: request.arguments.clone(),
            },
        );
        rx
    }

    /// What this process is still waiting on for `call_id`.
    fn live(&self, call_id: &str) -> Option<(String, serde_json::Value)> {
        let inner = self.inner.lock().expect("approvals poisoned");
        inner
            .get(call_id)
            .map(|pending| (pending.tool.clone(), pending.arguments.clone()))
    }

    /// The scopes a request may widen to. A prefix scope is listed only while a
    /// prefix store exists, so the offer and the honoring cannot drift apart.
    fn offered_for(&self, tool: &str, arguments: &serde_json::Value) -> Vec<ApprovalScope> {
        let mut scopes = ApprovalScope::offered(tool, command_of(arguments));
        if !self.prefixes.attached() {
            scopes.retain(|scope| *scope != ApprovalScope::CommandPrefix);
        }
        scopes
    }

    /// The unified request a UI draws, advertising exactly what this gate will
    /// honor for it.
    pub fn dto(&self, request: &ApprovalRequest) -> ApprovalRequestDto {
        let scopes = self.offered_for(&request.tool, &request.arguments);
        let command_prefix = scopes
            .contains(&ApprovalScope::CommandPrefix)
            .then(|| command_of(&request.arguments))
            .flatten()
            .and_then(command_prefix_for);
        ApprovalRequestDto {
            call_id: request.call_id.clone(),
            tool: request.tool.clone(),
            risk: request.risk.clone(),
            arguments: serde_json::to_string_pretty(&request.arguments)
                .unwrap_or_else(|_| request.arguments.to_string()),
            reason: request.reason.clone(),
            scopes: scopes
                .into_iter()
                .map(|scope| scope.as_str().to_string())
                .collect(),
            command_prefix,
        }
    }

    /// Resolve a pending request by `call_id` with a decision. Returns `true`
    /// if a matching pending request existed.
    pub fn resolve(&self, call_id: &str, decision: ApprovalDecision) -> bool {
        let sender = self
            .inner
            .lock()
            .expect("approvals poisoned")
            .remove(call_id)
            .map(|pending| pending.sender);
        match sender {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    /// Resolve from a UI boolean (`true` = approved).
    pub fn resolve_approved(&self, call_id: &str, approved: bool) -> bool {
        self.resolve(call_id, ApprovalDecision::from_approved(approved))
    }

    /// Interpret an answer's scope against the live request, without answering it.
    ///
    /// The durable write happens before any grant is recorded, so the caller needs
    /// to be able to ask "would this be accepted?" first.
    pub fn check_scope(&self, call_id: &str, scope: Option<&str>) -> Result<ApprovalScope> {
        let scope = match scope {
            None | Some("") => ApprovalScope::SingleCall,
            Some(value) => ApprovalScope::parse(value)
                .ok_or_else(|| CoreError::invalid(format!("unknown approval scope: {value}")))?,
        };
        if scope == ApprovalScope::SingleCall {
            return Ok(scope);
        }
        match self.live(call_id) {
            Some((tool, arguments)) if self.offered_for(&tool, &arguments).contains(&scope) => {
                Ok(scope)
            }
            Some((tool, _)) => Err(CoreError::invalid(format!(
                "approval scope '{}' is not offered for tool {tool}",
                scope.as_str()
            ))),
            None => Err(CoreError::invalid(format!(
                "no live approval {call_id} to widen; grant scopes are only accepted \
                 while the request is pending in this process"
            ))),
        }
    }

    /// Resolve with the scope the human chose, recording a grant when approval
    /// widens. A scope the request did not offer is refused rather than downgraded.
    pub fn resolve_scoped(
        &self,
        call_id: &str,
        approved: bool,
        scope: Option<&str>,
    ) -> Result<bool> {
        let scope = self.check_scope(call_id, scope)?;
        let pending = {
            let mut inner = self.inner.lock().expect("approvals poisoned");
            match inner.remove(call_id) {
                Some(pending) => pending,
                None => return Ok(false),
            }
        };
        let (tool, risk) = (pending.tool.clone(), pending.risk.clone());
        // A prefix grant is written before the run is woken, so the answer only
        // becomes real once the rule exists. A store that refuses it (a fenced
        // prefix, no store attached) puts the request back unanswered rather than
        // letting the command run on a promise nothing keeps.
        if approved && scope == ApprovalScope::CommandPrefix {
            if let Err(error) = self.prefixes.grant_for(&tool, &pending.arguments) {
                self.inner
                    .lock()
                    .expect("approvals poisoned")
                    .insert(call_id.to_string(), pending);
                return Err(error);
            }
        }
        let resolved = pending
            .sender
            .send(ApprovalDecision::from_approved(approved))
            .is_ok();
        if approved && scope == ApprovalScope::ToolSession {
            self.grants.grant(&tool, &risk);
        }
        Ok(resolved)
    }

    /// How many requests are currently awaiting a decision.
    pub fn pending_count(&self) -> usize {
        self.inner.lock().expect("approvals poisoned").len()
    }
}

/// An [`ApprovalGate`] that emits a unified request DTO to the UI and awaits the
/// user's decision via a `oneshot` channel.
#[derive(Clone)]
pub struct ChannelApprovalGate {
    pending: PendingApprovals,
    /// Called when a new request needs human attention (UI emit). Boxed so the
    /// gate stays transport-agnostic.
    notify: Arc<dyn Fn(ApprovalRequestDto) + Send + Sync>,
}

impl ChannelApprovalGate {
    /// Build a gate over a shared [`PendingApprovals`] and a UI-notify callback.
    pub fn new(
        pending: PendingApprovals,
        notify: Arc<dyn Fn(ApprovalRequestDto) + Send + Sync>,
    ) -> Self {
        Self { pending, notify }
    }
}

#[async_trait]
impl ApprovalGate for ChannelApprovalGate {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        // A decision the human already made answers the call without waking the
        // UI: a grant for this tool this process (`a`), or a standing allowance
        // for this command prefix (`p`). This is the only place a prompt is
        // suppressed, so policy handling stays in `PolicyGate` above it.
        if self.pending.grants().contains(&request.tool, &request.risk) {
            tracing::debug!(
                tool = %request.tool,
                risk = %request.risk,
                "approving from a session grant without asking"
            );
            return ApprovalDecision::Allow;
        }
        if self.pending.prefixes().covers(&request) {
            tracing::debug!(
                tool = %request.tool,
                "approving from a command prefix the human allowed without asking"
            );
            return ApprovalDecision::Allow;
        }
        let rx = self.pending.register(&request);
        (self.notify)(self.pending.dto(&request));
        // Await the human decision. If the sender is dropped (UI closed), deny
        // for safety.
        rx.await.unwrap_or(ApprovalDecision::Deny)
    }
}

/// Wraps an inner gate with an [`ApprovalPolicy`]: automatic policies resolve
/// without prompting; `AlwaysAsk` delegates to the inner (channel) gate.
pub struct PolicyGate {
    policy: ApprovalPolicy,
    inner: Arc<dyn ApprovalGate>,
    /// Safety classifier for the `AutoReview` policy: decides per-call whether
    /// to auto-approve or delegate to the user. When `None`, falls back to the
    /// legacy "bash/high-risk → ask, else allow" heuristic.
    classifier: Option<deepagent_builtins::SafetyClassifier>,
}

impl PolicyGate {
    /// Build with a policy and the inner gate used for `AlwaysAsk`.
    pub fn new(policy: ApprovalPolicy, inner: Arc<dyn ApprovalGate>) -> Self {
        Self {
            policy,
            inner,
            classifier: None,
        }
    }

    /// Attach a safety classifier for the `AutoReview` policy (builder style).
    pub fn with_classifier(mut self, classifier: deepagent_builtins::SafetyClassifier) -> Self {
        self.classifier = Some(classifier);
        self
    }
}

#[async_trait]
impl ApprovalGate for PolicyGate {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        match self.policy {
            // 完全访问: allow everything without prompting.
            ApprovalPolicy::FullAccess => ApprovalDecision::Allow,
            // 自动审核: use the safety classifier (when attached) to decide
            // per-call whether to auto-approve or prompt. Falls back to the
            // legacy heuristic when no classifier is configured.
            ApprovalPolicy::AutoReview => {
                if let Some(classifier) = &self.classifier {
                    match classifier.classify(&request.tool, &request.arguments) {
                        deepagent_builtins::SafetyVerdict::Allow => ApprovalDecision::Allow,
                        deepagent_builtins::SafetyVerdict::Deny(_) => ApprovalDecision::Deny,
                        deepagent_builtins::SafetyVerdict::Ask(_) => {
                            self.inner.request(request).await
                        }
                    }
                } else {
                    // Legacy fallback: bash/shell → ask; high-risk → ask; else allow.
                    let is_computer_op = is_command_tool(&request.tool);
                    let is_high_risk = request.risk.eq_ignore_ascii_case("high");
                    if is_computer_op || is_high_risk {
                        self.inner.request(request).await
                    } else {
                        ApprovalDecision::Allow
                    }
                }
            }
            // 默认权限: ask the user for every approval-requiring call.
            ApprovalPolicy::AlwaysAsk => self.inner.request(request).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(call_id: &str) -> ApprovalRequest {
        ApprovalRequest {
            call_id: call_id.into(),
            tool: "bash".into(),
            reason: "high risk".into(),
            risk: "high".into(),
            arguments: serde_json::json!({"command": "git push"}),
        }
    }

    #[tokio::test]
    async fn channel_gate_resolves_via_ui() {
        let pending = PendingApprovals::new();
        let seen = Arc::new(Mutex::new(Vec::<ApprovalRequestDto>::new()));
        let seen2 = seen.clone();
        let gate = ChannelApprovalGate::new(
            pending.clone(),
            Arc::new(move |dto| seen2.lock().unwrap().push(dto)),
        );

        // Drive the gate concurrently; resolve from the "UI" side.
        let handle = tokio::spawn(async move { gate.request(req("c1")).await });
        // Wait until the request is registered, then approve it.
        for _ in 0..100 {
            if pending.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert!(pending.resolve_approved("c1", true));
        assert_eq!(handle.await.unwrap(), ApprovalDecision::Allow);
        // The UI was notified with a unified DTO.
        assert_eq!(seen.lock().unwrap()[0].tool, "bash");
        assert!(seen.lock().unwrap()[0].arguments.contains("git push"));
    }

    #[tokio::test]
    async fn resolve_unknown_call_is_false() {
        let pending = PendingApprovals::new();
        assert!(!pending.resolve_approved("nope", true));
    }

    #[tokio::test]
    async fn policy_auto_review_allows_non_computer_ops() {
        // A non-bash request (e.g. out-of-workspace read) auto-approves under
        // AutoReview even though the inner gate would deny.
        let inner: Arc<dyn ApprovalGate> = Arc::new(deepagent_runtime::AutoDenyGate);
        let gate = PolicyGate::new(ApprovalPolicy::AutoReview, inner);
        let mut r = req("c1");
        r.tool = "read_file".into();
        r.risk = "ask".into();
        assert_eq!(gate.request(r).await, ApprovalDecision::Allow);
    }

    #[tokio::test]
    async fn policy_auto_review_delegates_computer_ops() {
        // A bash request (computer operation) still goes to the user under
        // AutoReview: here the inner gate approves, proving it delegated.
        let inner: Arc<dyn ApprovalGate> = Arc::new(deepagent_runtime::AutoApproveGate);
        let gate = PolicyGate::new(ApprovalPolicy::AutoReview, inner);
        let r = req("c1"); // tool = "bash"
        assert_eq!(gate.request(r).await, ApprovalDecision::Allow);

        // And if the inner gate denies, the bash op is denied (not auto-allowed).
        let deny_inner: Arc<dyn ApprovalGate> = Arc::new(deepagent_runtime::AutoDenyGate);
        let deny_gate = PolicyGate::new(ApprovalPolicy::AutoReview, deny_inner);
        assert_eq!(deny_gate.request(req("c2")).await, ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn policy_full_access_allows() {
        let inner: Arc<dyn ApprovalGate> = Arc::new(deepagent_runtime::AutoDenyGate);
        let gate = PolicyGate::new(ApprovalPolicy::FullAccess, inner);
        assert_eq!(gate.request(req("c1")).await, ApprovalDecision::Allow);
    }

    #[tokio::test]
    async fn policy_always_ask_delegates_to_inner() {
        let inner: Arc<dyn ApprovalGate> = Arc::new(deepagent_runtime::AutoApproveGate);
        let gate = PolicyGate::new(ApprovalPolicy::AlwaysAsk, inner);
        // Delegates to inner (auto-approve here) rather than auto-deciding.
        assert_eq!(gate.request(req("c1")).await, ApprovalDecision::Allow);
    }

    async fn wait_until_pending(pending: &PendingApprovals, wanted: usize) {
        for _ in 0..200 {
            if pending.pending_count() == wanted {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("the gate never registered {wanted} pending approval(s)");
    }

    #[tokio::test]
    async fn a_tool_session_grant_stops_the_next_prompt_without_asking() {
        let pending = PendingApprovals::new();
        let asked = Arc::new(Mutex::new(0usize));
        let asked_gate = asked.clone();
        let gate = ChannelApprovalGate::new(
            pending.clone(),
            Arc::new(move |_| {
                *asked_gate.lock().unwrap() += 1;
            }),
        );

        let first = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.request(req("c1")).await })
        };
        wait_until_pending(&pending, 1).await;
        assert!(pending
            .resolve_scoped("c1", true, Some("tool_session"))
            .unwrap());
        assert_eq!(first.await.unwrap(), ApprovalDecision::Allow);
        assert_eq!(*asked.lock().unwrap(), 1, "the first call asked");

        // Second call, same tool and risk: answered from the grant, UI never woken.
        assert_eq!(
            gate.request(req("c2")).await,
            ApprovalDecision::Allow,
            "a granted tool must not prompt again in this process"
        );
        assert_eq!(*asked.lock().unwrap(), 1, "the grant replaced the prompt");
        assert_eq!(pending.pending_count(), 0);
    }

    #[tokio::test]
    async fn a_denied_answer_never_grants_anything() {
        let pending = PendingApprovals::new();
        let gate = Arc::new(ChannelApprovalGate::new(pending.clone(), Arc::new(|_| {})));
        let handle = {
            let pending = pending.clone();
            let gate = gate.clone();
            tokio::spawn(async move {
                let request = req("c1");
                let awaiting = gate.request(request).await;
                let _ = pending;
                awaiting
            })
        };
        wait_until_pending(&pending, 1).await;
        // The human wanted to deny but reached for the grant key anyway.
        pending
            .resolve_scoped("c1", false, Some("tool_session"))
            .unwrap();
        assert_eq!(handle.await.unwrap(), ApprovalDecision::Deny);
        assert!(
            pending.grants().list().is_empty(),
            "denying cannot widen anything"
        );
    }

    #[tokio::test]
    async fn a_grant_can_be_revoked() {
        let pending = PendingApprovals::new();
        pending.grants().grant("bash", "high");
        assert!(pending.grants().contains("BASH", "HIGH"));
        assert!(pending.grants().revoke("bash", "high"));
        assert!(!pending.grants().contains("bash", "high"));
        assert!(!pending.grants().revoke("bash", "high"));
    }

    #[test]
    fn a_scope_is_offered_only_where_the_answer_would_have_effect() {
        assert!(ApprovalScope::offers(
            ApprovalScope::ToolSession,
            "bash",
            None
        ));
        assert!(ApprovalScope::offers(
            ApprovalScope::ToolSession,
            "write_file",
            None
        ));
        // A prefix can only stand in for a command, and only for a command one
        // covers: no command, no tool that takes one, no compound, no sudo.
        assert!(!ApprovalScope::offers(
            ApprovalScope::CommandPrefix,
            "bash",
            None
        ));
        assert!(!ApprovalScope::offers(
            ApprovalScope::CommandPrefix,
            "write_file",
            Some("rm -rf out")
        ));
        assert!(!ApprovalScope::offers(
            ApprovalScope::CommandPrefix,
            "bash",
            Some("cd src && rm -rf out")
        ));
        assert!(!ApprovalScope::offers(
            ApprovalScope::CommandPrefix,
            "bash",
            Some("sudo rm out")
        ));
        assert!(ApprovalScope::offers(
            ApprovalScope::CommandPrefix,
            "bash",
            Some("git status")
        ));
        let scopes: Vec<&str> = ApprovalScope::offered("bash", Some("git status"))
            .iter()
            .map(|scope| scope.as_str())
            .collect();
        assert_eq!(
            scopes,
            vec!["single_call", "tool_session", "command_prefix"]
        );
    }

    /// The store seam, remembered in memory so the tests can watch what `p`
    /// would persist without a database.
    #[derive(Default)]
    struct MemoryPrefixes {
        granted: Mutex<Vec<String>>,
        refuse: bool,
    }

    impl CommandPrefixSource for MemoryPrefixes {
        fn prefixes(&self, tool: &str) -> Vec<String> {
            if !is_command_tool(tool) {
                return Vec::new();
            }
            self.granted.lock().unwrap().clone()
        }

        fn grant(&self, tool: &str, prefix: &str) -> Result<()> {
            if !is_command_tool(tool) || self.refuse {
                return Err(CoreError::invalid("the store refused this grant"));
            }
            self.granted.lock().unwrap().push(prefix.to_string());
            Ok(())
        }
    }

    fn gate_over(
        pending: &PendingApprovals,
        seen: Arc<Mutex<Vec<ApprovalRequestDto>>>,
    ) -> ChannelApprovalGate {
        ChannelApprovalGate::new(
            pending.clone(),
            Arc::new(move |dto| seen.lock().unwrap().push(dto)),
        )
    }

    #[tokio::test]
    async fn a_prefix_answer_stores_the_prefix_the_prompt_showed() {
        let pending = PendingApprovals::new();
        let store = Arc::new(MemoryPrefixes::default());
        pending.prefixes().attach(store.clone());
        let seen = Arc::new(Mutex::new(Vec::<ApprovalRequestDto>::new()));
        let gate = gate_over(&pending, seen.clone());
        let handle = tokio::spawn(async move { gate.request(req("c1")).await });
        wait_until_pending(&pending, 1).await;
        // The prompt names what it is about to allow, and answering `p` stores
        // exactly that — the client never sends a prefix of its own choosing.
        assert_eq!(
            seen.lock().unwrap()[0].command_prefix.as_deref(),
            Some("git push")
        );
        pending
            .resolve_scoped("c1", true, Some("command_prefix"))
            .unwrap();
        assert_eq!(handle.await.unwrap(), ApprovalDecision::Allow);
        assert_eq!(store.granted.lock().unwrap().as_slice(), ["git push"]);
    }

    #[tokio::test]
    async fn a_granted_prefix_stops_the_next_prompt_without_asking() {
        let pending = PendingApprovals::new();
        let store = Arc::new(MemoryPrefixes::default());
        pending.prefixes().attach(store.clone());
        let seen = Arc::new(Mutex::new(Vec::<ApprovalRequestDto>::new()));
        let gate = Arc::new(gate_over(&pending, seen.clone()));
        let asking = gate.clone();
        let first = tokio::spawn(async move { asking.request(req("c1")).await });
        wait_until_pending(&pending, 1).await;
        pending
            .resolve_scoped("c1", true, Some("command_prefix"))
            .unwrap();
        assert_eq!(first.await.unwrap(), ApprovalDecision::Allow);

        // A later, longer command of the same kind: allowed from the rule, and the
        // UI was never consulted a second time.
        let mut second = req("c2");
        second.arguments = serde_json::json!({"command": "git push origin main"});
        assert_eq!(gate.request(second).await, ApprovalDecision::Allow);
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_sandbox_escalation_is_never_covered_by_a_prefix_grant() {
        // Allowing a command says it may run *in* the sandbox, not that it may
        // leave it.
        let pending = PendingApprovals::new();
        let store = Arc::new(MemoryPrefixes::default());
        store
            .granted
            .lock()
            .unwrap()
            .push("cargo build".to_string());
        pending.prefixes().attach(store.clone());
        let mut request = req("c1");
        request.arguments = serde_json::json!({"command": "cargo build", "sandbox_permissions": "require_escalated"});
        assert!(!pending.prefixes().covers(&request));
        request.arguments = serde_json::json!({"command": "cargo build --release"});
        assert!(pending.prefixes().covers(&request));
    }

    #[tokio::test]
    async fn a_plain_answer_never_stores_a_prefix() {
        let pending = PendingApprovals::new();
        let store = Arc::new(MemoryPrefixes::default());
        pending.prefixes().attach(store.clone());
        let seen = Arc::new(Mutex::new(Vec::<ApprovalRequestDto>::new()));
        let gate = gate_over(&pending, seen.clone());
        let handle = tokio::spawn(async move { gate.request(req("c1")).await });
        wait_until_pending(&pending, 1).await;
        pending.resolve_approved("c1", true);
        assert_eq!(handle.await.unwrap(), ApprovalDecision::Allow);
        assert!(store.granted.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unattached_store_does_not_advertise_the_scope_it_cannot_write() {
        let pending = PendingApprovals::new();
        let request = req("c1");
        assert!(!pending
            .dto(&request)
            .scopes
            .contains(&"command_prefix".to_string()));
        let refused = pending
            .check_scope("c1", Some("command_prefix"))
            .unwrap_err();
        assert!(
            refused.to_string().contains("no live approval"),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn a_store_that_refuses_leaves_the_prompt_answerable() {
        let pending = PendingApprovals::new();
        let store = Arc::new(MemoryPrefixes {
            granted: Mutex::new(Vec::new()),
            refuse: true,
        });
        pending.prefixes().attach(store.clone());
        let seen = Arc::new(Mutex::new(Vec::<ApprovalRequestDto>::new()));
        let gate = gate_over(&pending, seen.clone());
        let handle = tokio::spawn(async move { gate.request(req("c1")).await });
        wait_until_pending(&pending, 1).await;
        let failed = pending
            .resolve_scoped("c1", true, Some("command_prefix"))
            .unwrap_err();
        assert!(failed.to_string().contains("refused"), "{failed}");
        // Nothing was stored, and the run is still parked on the same question,
        // so the human can answer it a second way.
        assert!(store.granted.lock().unwrap().is_empty());
        assert_eq!(pending.pending_count(), 1);
        assert!(pending.resolve_approved("c1", true));
        assert_eq!(handle.await.unwrap(), ApprovalDecision::Allow);
    }

    #[test]
    fn an_unofferable_scope_is_refused_before_anything_is_answered() {
        let pending = PendingApprovals::new();
        // Nothing is live, so only the plain answer may pass validation.
        assert!(pending.check_scope("c1", None).is_ok());
        assert!(pending.check_scope("c1", Some("single_call")).is_ok());
        let unknown = pending
            .check_scope("c1", Some("all_future_calls"))
            .unwrap_err();
        assert!(unknown.to_string().contains("unknown"), "{unknown}");
        let widen = pending.check_scope("c1", Some("tool_session")).unwrap_err();
        assert!(widen.to_string().contains("no live approval"), "{widen}");
        assert!(pending
            .resolve_scoped("c1", true, Some("tool_session"))
            .is_err());
        assert!(pending.grants().list().is_empty());
    }

    #[tokio::test]
    async fn the_dto_advertises_what_the_gate_will_honor() {
        let pending = PendingApprovals::new();
        let seen = Arc::new(Mutex::new(Vec::<ApprovalRequestDto>::new()));
        let seen2 = seen.clone();
        let gate = ChannelApprovalGate::new(
            pending.clone(),
            Arc::new(move |dto| seen2.lock().unwrap().push(dto)),
        );
        let handle = tokio::spawn(async move { gate.request(req("c1")).await });
        wait_until_pending(&pending, 1).await;
        pending
            .resolve_scoped("c1", true, Some("tool_session"))
            .unwrap();
        handle.await.unwrap();
        let dto = &seen.lock().unwrap()[0];
        assert!(
            dto.scopes.contains(&"tool_session".to_string()),
            "the answer menu has to be advertised to the UI: {:?}",
            dto.scopes
        );
        assert!(!dto.scopes.contains(&"command_prefix".to_string()));
    }

    #[tokio::test]
    async fn multiple_pending_resolve_independently() {
        let pending = PendingApprovals::new();
        let gate = Arc::new(ChannelApprovalGate::new(pending.clone(), Arc::new(|_| {})));
        let g1 = gate.clone();
        let g2 = gate.clone();
        let h1 = tokio::spawn(async move { g1.request(req("a")).await });
        let h2 = tokio::spawn(async move { g2.request(req("b")).await });
        for _ in 0..100 {
            if pending.pending_count() == 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        // Resolve b first (deny), then a (allow) — order-independent.
        assert!(pending.resolve_approved("b", false));
        assert!(pending.resolve_approved("a", true));
        assert_eq!(h1.await.unwrap(), ApprovalDecision::Allow);
        assert_eq!(h2.await.unwrap(), ApprovalDecision::Deny);
    }
}
