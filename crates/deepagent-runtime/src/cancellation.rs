//! Hierarchical cancellation shared by model streams, hooks, tools and agents.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct CancellationTree {
    token: CancellationToken,
    legacy_flag: Arc<AtomicBool>,
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
        }
    }

    pub fn from_legacy_flag(flag: Arc<AtomicBool>) -> Self {
        Self {
            token: CancellationToken::new(),
            legacy_flag: flag,
        }
    }

    pub fn child(&self) -> Self {
        Self {
            token: self.token.child_token(),
            legacy_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) -> bool {
        let first = !self.legacy_flag.swap(true, Ordering::SeqCst);
        self.token.cancel();
        first
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
}
