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
pub(crate) struct ConnectionOperations(Mutex<HashMap<String, Weak<Operation>>>);

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
        let operation = {
            let mut entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
            entries.retain(|_, entry| entry.strong_count() > 0);
            match entries.get(id).and_then(Weak::upgrade) {
                Some(operation) if !operation.retired.is_cancelled() => operation,
                _ => {
                    let operation = Arc::new(Operation::default());
                    entries.insert(id.to_owned(), Arc::downgrade(&operation));
                    operation
                }
            }
        };
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

    pub(crate) fn retire(&self, id: &str) {
        let entries = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(operation) = entries.get(id).and_then(Weak::upgrade) {
            operation.retired.cancel();
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
