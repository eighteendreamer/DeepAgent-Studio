//! Bridges runtime tool-approval requests to the UI and back (Phase A-3).
//!
//! The runtime's [`ApprovalGate`] is async: it *awaits* a decision. On the
//! desktop, that decision comes from a human clicking the approval dialog. This
//! module provides:
//!
//! - [`PendingApprovals`] — a shared registry of in-flight requests keyed by
//!   `call_id`, each backed by a `oneshot` channel. Multiple requests can be
//!   pending at once (queued in the UI); each resolves independently.
//! - [`ChannelApprovalGate`] — the [`ApprovalGate`] the runtime holds: on
//!   `request` it registers the pending decision, emits a unified
//!   [`ApprovalRequestDto`] to the UI (via a callback), and awaits the reply.
//! - [`PolicyGate`] — wraps a gate with an [`ApprovalPolicy`]: `AutoReview` /
//!   `FullAccess` resolve automatically (no UI prompt), `AlwaysAsk` delegates to
//!   the inner channel gate.
//!
//! The UI resolves a request by calling [`PendingApprovals::resolve`] with the
//! `call_id` and the user's decision.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_runtime::{ApprovalDecision, ApprovalGate, ApprovalRequest};
use tokio::sync::oneshot;

use crate::dto::ApprovalRequestDto;
use crate::settings::ApprovalPolicy;

/// Convert a runtime [`ApprovalRequest`] into the unified UI DTO.
fn to_dto(req: &ApprovalRequest) -> ApprovalRequestDto {
    ApprovalRequestDto {
        call_id: req.call_id.clone(),
        tool: req.tool.clone(),
        risk: req.risk.clone(),
        arguments: serde_json::to_string_pretty(&req.arguments)
            .unwrap_or_else(|_| req.arguments.to_string()),
        reason: req.reason.clone(),
    }
}

/// Shared registry of in-flight approval requests, keyed by `call_id`.
#[derive(Clone, Default)]
pub struct PendingApprovals {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<ApprovalDecision>>>>,
}

impl PendingApprovals {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pending request, returning the receiver to await.
    fn register(&self, call_id: String) -> oneshot::Receiver<ApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        self.inner
            .lock()
            .expect("approvals poisoned")
            .insert(call_id, tx);
        rx
    }

    /// Resolve a pending request by `call_id` with a decision. Returns `true`
    /// if a matching pending request existed.
    pub fn resolve(&self, call_id: &str, decision: ApprovalDecision) -> bool {
        let sender = self
            .inner
            .lock()
            .expect("approvals poisoned")
            .remove(call_id);
        match sender {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    /// Resolve from a UI boolean (`true` = approved).
    pub fn resolve_approved(&self, call_id: &str, approved: bool) -> bool {
        self.resolve(call_id, ApprovalDecision::from_approved(approved))
    }

    /// How many requests are currently awaiting a decision.
    pub fn pending_count(&self) -> usize {
        self.inner.lock().expect("approvals poisoned").len()
    }
}

/// An [`ApprovalGate`] that emits a unified request DTO to the UI and awaits the
/// user's decision via a `oneshot` channel.
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
        let rx = self.pending.register(request.call_id.clone());
        (self.notify)(to_dto(&request));
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
                    let tool = request.tool.as_str();
                    let is_computer_op = matches!(tool, "bash" | "shell");
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
