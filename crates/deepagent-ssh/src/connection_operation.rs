//! Per-connection admission for bounded network phases, independent of input leases.
//!
//! Weak entries retain neither a transport nor credentials. A dropped owner releases
//! its mutex normally; disconnect cancels owners and waiters in that exact epoch.
use crate::{SshError, SshResult};
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub(crate) struct ConnectionOperations(Mutex<OperationState>);

#[derive(Default)]
struct OperationState {
    stopped: bool,
    entries: HashMap<String, Weak<Operation>>,
}

#[derive(Default)]
struct Operation {
    gate: Arc<AsyncMutex<()>>,
    retired: CancellationToken,
}

pub(crate) struct ConnectionPermit {
    id: String,
    operation: Arc<Operation>,
    _guard: OwnedMutexGuard<()>,
}

impl ConnectionOperations {
    pub(crate) async fn acquire(&self, id: &str) -> SshResult<ConnectionPermit> {
        let operation = self.operation(id)?;
        let guard = tokio::select! {
            biased;
            _ = operation.retired.cancelled() => return Err(retired(id)),
            guard = Arc::clone(&operation.gate).lock_owned() => guard,
        };
        let permit = ConnectionPermit {
            id: id.to_owned(),
            operation,
            _guard: guard,
        };
        permit.ensure_current()?;
        Ok(permit)
    }

    pub(crate) fn try_acquire(&self, id: &str) -> Option<ConnectionPermit> {
        let operation = self.operation(id).ok()?;
        let guard = Arc::clone(&operation.gate).try_lock_owned().ok()?;
        let permit = ConnectionPermit {
            id: id.to_owned(),
            operation,
            _guard: guard,
        };
        permit.ensure_current().ok()?;
        Some(permit)
    }

    fn operation(&self, id: &str) -> SshResult<Arc<Operation>> {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped {
            return Err(SshError::ServiceStopped);
        }
        state.entries.retain(|_, entry| entry.strong_count() > 0);
        Ok(match state.entries.get(id).and_then(Weak::upgrade) {
            Some(operation) if !operation.retired.is_cancelled() => operation,
            _ => {
                let operation = Arc::new(Operation::default());
                state
                    .entries
                    .insert(id.to_owned(), Arc::downgrade(&operation));
                operation
            }
        })
    }

    pub(crate) fn retire(&self, id: &str) {
        let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(operation) = state.entries.get(id).and_then(Weak::upgrade) {
            operation.retired.cancel();
        }
    }

    pub(crate) fn stop_admission(&self) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.stopped = true;
        state.entries.retain(|_, entry| {
            if let Some(operation) = entry.upgrade() {
                operation.retired.cancel();
                true
            } else {
                false
            }
        });
    }

    pub(crate) fn ensure_running(&self) -> SshResult<()> {
        let state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped {
            Err(SshError::ServiceStopped)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn wait_for_quiescence(&self) {
        let operations = {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            state.entries.retain(|_, entry| entry.strong_count() > 0);
            state
                .entries
                .values()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for operation in operations {
            let _guard = Arc::clone(&operation.gate).lock_owned().await;
        }
    }
}

impl ConnectionPermit {
    pub(crate) fn ensure_current(&self) -> SshResult<()> {
        if self.operation.retired.is_cancelled() {
            Err(retired(&self.id))
        } else {
            Ok(())
        }
    }

    pub(crate) async fn run<T>(&self, work: impl Future<Output = SshResult<T>>) -> SshResult<T> {
        tokio::select! {
            biased;
            _ = self.operation.retired.cancelled() => Err(retired(&self.id)),
            result = work => result,
        }
    }
}

fn retired(id: &str) -> SshError {
    SshError::ConnectionLost(format!("{id}: SSH connection operation was retired"))
}
