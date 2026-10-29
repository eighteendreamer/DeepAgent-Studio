//! Identity of the model context that received a tool observation.
//!
//! This is ephemeral read evidence, not a permission or cancellation mechanism.
//! Only the scope owns the current generation. Invocations and caches hold weak
//! tokens, so late work cannot keep a replaced generation alive or authorize its
//! successor. The allocation identity cannot be reused while a token exists.

use std::sync::{Arc, Mutex, Weak};

/// One model context's replaceable observation generation. Clone only for
/// participants in that context; use `default()` for an independent run.
#[derive(Clone, Debug, Default)]
pub struct ToolObservationScope {
    current: Arc<Mutex<Arc<()>>>,
}

impl ToolObservationScope {
    /// Capture before dispatch, not after a delayed tool has completed.
    pub fn capture(&self) -> ToolObservation {
        let current = self.current.lock().unwrap_or_else(|p| p.into_inner());
        ToolObservation(Arc::downgrade(&current))
    }

    /// Revoke only this context's observations after a replacement is adopted.
    pub fn reset(&self) {
        *self.current.lock().unwrap_or_else(|p| p.into_inner()) = Arc::new(());
    }
}

/// Non-owning identity captured by an invocation and retained with its evidence.
#[derive(Clone, Debug)]
pub struct ToolObservation(Weak<()>);

impl ToolObservation {
    pub fn is_current(&self) -> bool {
        self.0.strong_count() != 0
    }

    pub fn same_context(&self, other: &Self) -> bool {
        self.is_current() && self.0.ptr_eq(&other.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolExecutionContext;

    #[test]
    fn reset_and_drop_revoke_weak_tokens_without_affecting_another_context() {
        let parent = ToolObservationScope::default();
        let hook = ToolObservationScope::default();
        let old = parent.capture();
        let other = hook.capture();
        assert!(old.same_context(&parent.clone().capture()));
        assert!(!old.same_context(&other));
        parent.reset();
        assert!(!old.is_current());
        assert!(!old.same_context(&parent.capture()));
        assert!(other.is_current());
        drop(hook);
        assert!(!other.is_current());
    }

    #[test]
    fn child_preserves_observation_but_not_cancellation_ownership() {
        let scope = ToolObservationScope::default();
        let context = ToolExecutionContext::default().with_observation(scope.capture());
        let child = context.child();
        assert!(context
            .observation()
            .unwrap()
            .same_context(child.observation().unwrap()));
        child.cancellation_token().cancel();
        assert!(!context.is_cancelled());
        scope.reset();
        assert!(!context.observation_is_current());
        assert!(!child.observation_is_current());
        assert!(ToolExecutionContext::default().observation().is_none());
    }
}
