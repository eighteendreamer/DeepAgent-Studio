//! Bounded readiness, not another output store. Each PTY owns its signal;
//! observers share a single revision, never an unbounded queue of byte payloads.
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalOutputNotice {
    pub revision: u64,
    pub closed: bool,
}

#[derive(Debug, Clone)]
pub struct TerminalOutputSignal(watch::Sender<u64>);

impl Default for TerminalOutputSignal {
    fn default() -> Self {
        Self(watch::channel(0).0)
    }
}

impl TerminalOutputSignal {
    pub fn notify(&self) {
        self.0
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    pub fn subscribe(&self) -> TerminalOutputSubscription {
        TerminalOutputSubscription(self.0.subscribe())
    }
}

#[derive(Debug)]
pub struct TerminalOutputSubscription(watch::Receiver<u64>);

impl TerminalOutputSubscription {
    /// A finite wait permits resynchronization and bounds abandoned IPC calls.
    /// Register-and-check is atomic with respect to watch revisions; a notice
    /// arriving before or during subscription cannot strand readable bytes.
    pub async fn wait_after(&mut self, after: u64, max_wait: Duration) -> TerminalOutputNotice {
        let wait = async {
            loop {
                let revision = *self.0.borrow_and_update();
                let closed = self.0.has_changed().is_err();
                if revision != after || closed {
                    return TerminalOutputNotice { revision, closed };
                }
                if self.0.changed().await.is_err() {
                    return TerminalOutputNotice {
                        revision: *self.0.borrow(),
                        closed: true,
                    };
                }
            }
        };
        match tokio::time::timeout(max_wait, wait).await {
            Ok(notice) => notice,
            Err(_) => TerminalOutputNotice {
                revision: *self.0.borrow(),
                closed: self.0.has_changed().is_err(),
            },
        }
    }
}
