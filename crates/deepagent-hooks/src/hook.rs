//! The [`Hook`] trait and the [`HookOutcome`] it returns.

use async_trait::async_trait;

use deepagent_core::error::Result;

use crate::lifecycle::HookContext;

/// The result of running a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// Allow the operation to proceed.
    Continue,
    /// Veto the operation with a reason. Only honored at vetoable points
    /// (see [`crate::lifecycle::HookPoint::is_vetoable`]); at non-vetoable
    /// points a `Deny` is downgraded to a logged warning by the registry.
    Deny(String),
}

impl HookOutcome {
    /// Whether this outcome denies the operation.
    pub fn is_deny(&self) -> bool {
        matches!(self, HookOutcome::Deny(_))
    }

    /// The denial reason, if any.
    pub fn deny_reason(&self) -> Option<&str> {
        match self {
            HookOutcome::Deny(r) => Some(r),
            HookOutcome::Continue => None,
        }
    }
}

/// A hook: a unit of logic that runs at one or more lifecycle points.
#[async_trait]
pub trait Hook: Send + Sync {
    /// A stable, human-readable name (for tracing / debugging).
    fn name(&self) -> &str;

    /// Run the hook for the given context.
    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_helpers() {
        assert!(HookOutcome::Deny("nope".into()).is_deny());
        assert!(!HookOutcome::Continue.is_deny());
        assert_eq!(HookOutcome::Deny("nope".into()).deny_reason(), Some("nope"));
        assert_eq!(HookOutcome::Continue.deny_reason(), None);
    }
}
