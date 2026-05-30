//! Human-in-the-loop tool approval (Phase A-3).
//!
//! When a tool call needs approval — because a `BeforeToolUse` hook returned
//! `Ask`, or the tool's [`RiskLevel`](deepagent_tools::RiskLevel) requires it —
//! the runtime consults an [`ApprovalGate`] and **awaits** a decision before
//! proceeding. This is the mechanism behind the desktop approval dialog: the
//! gate bridges the pause to the UI and resolves when the user clicks
//! allow/deny.
//!
//! The decision is *not* shipped over the live event stream (that is for
//! display); it flows back through the gate's awaited future so the loop can
//! block precisely on it. A request carries a stable `call_id` so multiple
//! pending approvals can be queued and resolved independently.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A request for a human (or policy) to approve a tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Correlates the decision back to the blocked call.
    pub call_id: String,
    /// Tool name.
    pub tool: String,
    /// Why approval is required (risk / hook reason).
    pub reason: String,
    /// Risk label (e.g. "high", "medium").
    pub risk: String,
    /// JSON arguments under review.
    pub arguments: serde_json::Value,
}

/// The outcome of an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Allow the call to proceed.
    Allow,
    /// Reject the call (becomes a failed observation).
    Deny,
}

impl ApprovalDecision {
    /// Whether the decision permits the call.
    pub fn is_allowed(&self) -> bool {
        matches!(self, ApprovalDecision::Allow)
    }

    /// From a UI boolean (`true` = approved).
    pub fn from_approved(approved: bool) -> Self {
        if approved {
            ApprovalDecision::Allow
        } else {
            ApprovalDecision::Deny
        }
    }
}

/// Resolves tool-approval requests. Implementations bridge to a human (desktop
/// dialog), a policy (auto-approve / auto-deny), or a coordinator agent.
#[async_trait]
pub trait ApprovalGate: Send + Sync {
    /// Await a decision for `request`. Must eventually resolve (a gate that
    /// never resolves would hang the run).
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision;
}

/// A gate that approves everything (used for the "auto review" / "full access"
/// policy and in tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoApproveGate;

#[async_trait]
impl ApprovalGate for AutoApproveGate {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Allow
    }
}

/// A gate that denies everything (the safe default when no UI is attached).
#[derive(Debug, Clone, Copy, Default)]
pub struct AutoDenyGate;

#[async_trait]
impl ApprovalGate for AutoDenyGate {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_helpers() {
        assert!(ApprovalDecision::Allow.is_allowed());
        assert!(!ApprovalDecision::Deny.is_allowed());
        assert_eq!(
            ApprovalDecision::from_approved(true),
            ApprovalDecision::Allow
        );
        assert_eq!(
            ApprovalDecision::from_approved(false),
            ApprovalDecision::Deny
        );
    }

    #[tokio::test]
    async fn auto_gates() {
        let req = ApprovalRequest {
            call_id: "c1".into(),
            tool: "bash".into(),
            reason: "high risk".into(),
            risk: "high".into(),
            arguments: serde_json::json!({"command": "git push"}),
        };
        assert_eq!(
            AutoApproveGate.request(req.clone()).await,
            ApprovalDecision::Allow
        );
        assert_eq!(AutoDenyGate.request(req).await, ApprovalDecision::Deny);
    }
}
