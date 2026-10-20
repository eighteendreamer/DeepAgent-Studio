//! Hierarchical cancellation shared by model streams, hooks, tools and agents.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct CancellationTree {
    token: CancellationToken,
    legacy_flag: Arc<AtomicBool>,
    requested_by: Arc<Mutex<Option<String>>>,
}

impl Default for CancellationTree {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationTree {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
            legacy_flag: Arc::new(AtomicBool::new(false)),
            requested_by: Arc::new(Mutex::new(None)),
        }
    }

    pub fn from_legacy_flag(flag: Arc<AtomicBool>) -> Self {
        Self {
            token: CancellationToken::new(),
            legacy_flag: flag,
            requested_by: Arc::new(Mutex::new(None)),
        }
    }

    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            legacy_flag: Arc::new(AtomicBool::new(false)),
            requested_by: Arc::new(Mutex::new(None)),
        }
    }

    pub fn cancel(&self) -> bool {
        let first = !self.legacy_flag.swap(true, Ordering::SeqCst);
        self.token.cancel();
        first
    }

    /// Record the first control-plane origin before waking the running task.
    /// No persistence is allowed on this path: the lifecycle writer audits it
    /// with the terminal event, even when cancellation happens during setup.
    pub fn request_cancel(&self, alias: &str) {
        self.requested_by
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_or_insert_with(|| alias.to_owned());
        self.cancel();
    }

    pub fn requested_by(&self) -> Option<String> {
        self.requested_by
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled() || self.legacy_flag.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Derive a token for one invocation. The derived token follows this tree
    /// downward, while cancelling it remains local to the invocation.
    pub fn child_token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// Bridge used while the legacy model/tool interfaces still accept an
    /// `AtomicBool`. Cancelling the root always updates this flag immediately.
    pub fn legacy_flag(&self) -> Arc<AtomicBool> {
        self.legacy_flag.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parent_cancels_child_and_legacy_bridge() {
        let root = CancellationTree::new();
        let child = root.child();
        let flag = root.legacy_flag();
        assert!(root.cancel());
        assert!(!root.cancel());
        child.cancelled().await;
        assert!(child.is_cancelled());
        assert!(flag.load(Ordering::Acquire));
    }

    #[test]
    fn cancelling_child_does_not_cancel_parent_or_siblings() {
        let root = CancellationTree::new();
        let child = root.child();
        let sibling = root.child();

        assert!(child.cancel());
        assert!(child.is_cancelled());
        assert!(!root.is_cancelled());
        assert!(!sibling.is_cancelled());
    }
}
