//! The [`Hook`] trait and the [`HookOutcome`] it returns.
//!
//! [`HookOutcome`] models the unified permission-result protocol from the
//! 复刻规范 §13:
//!
//! ```json
//! { "behavior": "allow|deny|ask", "reason": "", "updated_input": {}, "source": "policy|classifier|human|coordinator" }
//! ```
//!
//! mapped to four Rust variants:
//! - [`HookOutcome::Continue`] — `allow` (no input change).
//! - [`HookOutcome::Modify`]   — `allow` with `updated_input` (rewrite the call).
//! - [`HookOutcome::Ask`]      — `ask` (human/approval gate required).
//! - [`HookOutcome::Deny`]     — `deny` (veto the operation).
//!
//! Each non-trivial outcome carries a [`DecisionSource`] so the audit log can
//! record *who* decided (static policy, an auto-classifier, a human, or a
//! coordinating agent).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use deepagent_core::error::Result;

use crate::lifecycle::HookContext;

/// Where a hook decision originated (§13 `source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    /// A static allow/deny policy rule.
    #[default]
    Policy,
    /// An automatic risk classifier.
    Classifier,
    /// A human operator (interactive approval).
    Human,
    /// A coordinating / parent agent.
    Coordinator,
}

impl DecisionSource {
    /// Stable string label.
    pub const fn label(&self) -> &'static str {
        match self {
            DecisionSource::Policy => "policy",
            DecisionSource::Classifier => "classifier",
            DecisionSource::Human => "human",
            DecisionSource::Coordinator => "coordinator",
        }
    }
}

/// The result of running a hook (the §13 permission-result protocol).
#[derive(Debug, Clone, PartialEq)]
pub enum HookOutcome {
    /// Allow the operation to proceed unchanged (`behavior: "allow"`).
    Continue,
    /// Allow, but rewrite the operation's input first (`behavior: "allow"` with
    /// `updated_input`). At [`crate::lifecycle::HookPoint::BeforeToolUse`] the
    /// `updated_input` replaces the tool arguments. Only honored at vetoable
    /// points; elsewhere downgraded to a warning.
    Modify {
        /// The replacement input (JSON arguments).
        updated_input: serde_json::Value,
        /// Who decided.
        source: DecisionSource,
    },
    /// Require explicit approval before proceeding (`behavior: "ask"`). Only
    /// honored at vetoable points. Precedence: a later `Deny` overrides an
    /// `Ask`.
    Ask {
        /// Why approval is needed.
        reason: String,
        /// Who decided.
        source: DecisionSource,
    },
    /// Veto the operation with a reason (`behavior: "deny"`). Terminal: the
    /// registry short-circuits remaining hooks. Only honored at vetoable
    /// points; elsewhere downgraded to a logged warning.
    Deny {
        /// Why the operation was denied.
        reason: String,
        /// Who decided.
        source: DecisionSource,
    },
}

impl HookOutcome {
    /// Deny with a reason, attributed to static policy.
    pub fn deny(reason: impl Into<String>) -> Self {
        HookOutcome::Deny {
            reason: reason.into(),
            source: DecisionSource::Policy,
        }
    }

    /// Deny with a reason and an explicit source.
    pub fn deny_from(reason: impl Into<String>, source: DecisionSource) -> Self {
        HookOutcome::Deny {
            reason: reason.into(),
            source,
        }
    }

    /// Ask for approval with a reason, attributed to the risk classifier.
    pub fn ask(reason: impl Into<String>) -> Self {
        HookOutcome::Ask {
            reason: reason.into(),
            source: DecisionSource::Classifier,
        }
    }

    /// Ask for approval with a reason and an explicit source.
    pub fn ask_from(reason: impl Into<String>, source: DecisionSource) -> Self {
        HookOutcome::Ask {
            reason: reason.into(),
            source,
        }
    }

    /// Allow but rewrite the input, attributed to static policy.
    pub fn modify(updated_input: serde_json::Value) -> Self {
        HookOutcome::Modify {
            updated_input,
            source: DecisionSource::Policy,
        }
    }

    /// Whether this outcome denies the operation.
    pub fn is_deny(&self) -> bool {
        matches!(self, HookOutcome::Deny { .. })
    }

    /// Whether this outcome requests approval.
    pub fn is_ask(&self) -> bool {
        matches!(self, HookOutcome::Ask { .. })
    }

    /// Whether this outcome rewrites the input.
    pub fn is_modify(&self) -> bool {
        matches!(self, HookOutcome::Modify { .. })
    }

    /// The denial reason, if this is a `Deny`.
    pub fn deny_reason(&self) -> Option<&str> {
        match self {
            HookOutcome::Deny { reason, .. } => Some(reason),
            _ => None,
        }
    }

    /// The decision source, if any (None for plain `Continue`).
    pub fn source(&self) -> Option<DecisionSource> {
        match self {
            HookOutcome::Continue => None,
            HookOutcome::Modify { source, .. }
            | HookOutcome::Ask { source, .. }
            | HookOutcome::Deny { source, .. } => Some(*source),
        }
    }
}

/// A hook: a unit of logic that runs at one or more lifecycle points.
#[async_trait]
pub trait Hook: Send + Sync {
    /// A stable, human-readable name (for tracing / debugging).
    fn name(&self) -> &str;

    /// Stable identity used to deduplicate equivalent handlers registered from
    /// overlapping config sources.
    fn dedup_key(&self) -> String {
        self.name().to_string()
    }

    /// Run the hook for the given context.
    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_helpers() {
        assert!(HookOutcome::deny("nope").is_deny());
        assert!(!HookOutcome::Continue.is_deny());
        assert_eq!(HookOutcome::deny("nope").deny_reason(), Some("nope"));
        assert_eq!(HookOutcome::Continue.deny_reason(), None);

        assert!(HookOutcome::ask("approve?").is_ask());
        assert!(HookOutcome::modify(serde_json::json!({"x": 1})).is_modify());
    }

    #[test]
    fn source_is_tracked() {
        assert_eq!(HookOutcome::Continue.source(), None);
        assert_eq!(
            HookOutcome::deny("x").source(),
            Some(DecisionSource::Policy)
        );
        assert_eq!(
            HookOutcome::ask("x").source(),
            Some(DecisionSource::Classifier)
        );
        assert_eq!(
            HookOutcome::deny_from("x", DecisionSource::Human).source(),
            Some(DecisionSource::Human)
        );
    }

    #[test]
    fn source_labels_are_stable() {
        assert_eq!(DecisionSource::Policy.label(), "policy");
        assert_eq!(DecisionSource::Human.label(), "human");
        let json = serde_json::to_string(&DecisionSource::Coordinator).unwrap();
        assert_eq!(json, "\"coordinator\"");
    }
}
