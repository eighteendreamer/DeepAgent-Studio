//! Scope ownership only: execution/cancellation/terminal writes stay in the kernel.
use super::*;
use deepagent_core::error::CoreError;
use deepagent_persistence::run_store::RunStore;
use std::collections::BTreeSet;
use std::sync::Mutex;
use tokio_util::task::TaskTracker;

// Same local shutdown budget as the existing stdio connection owner. This is
// not a run deadline and never authorizes fabricating a terminal on timeout.
const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Default)]
struct State {
    active: BTreeSet<String>,
    failure: Option<String>,
}

pub(super) struct OwnedRuns {
    chat: Arc<ChatService>,
    state: Arc<Mutex<State>>,
    tasks: TaskTracker,
}

impl OwnedRuns {
    pub(super) fn new(chat: Arc<ChatService>) -> Self {
        Self {
            chat,
            state: Default::default(),
            tasks: TaskTracker::new(),
        }
    }

    pub(super) fn spawn(
        &self,
        run_id: String,
        future: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .insert(run_id.clone());
        let guard = CompletionGuard {
            chat: self.chat.clone(),
            state: self.state.clone(),
            run_id,
        };
        self.tasks.spawn(async move {
            let _completion = guard;
            future.await;
        });
    }

    fn cancel_owned(&self) {
        let ids = self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .active
            .clone();
        for id in ids {
            if let Err(error) = self.chat.request_cancel(&id) {
                let message = format!("cancel scheduled run {id}: {error}");
                tracing::error!(%message, "scheduled run cancellation failed");
                self.state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .failure
                    .get_or_insert(message);
            }
        }
    }

    pub(super) async fn drain(&self) -> Result<()> {
        self.cancel_owned();
        self.tasks.close();
        if tokio::time::timeout(DRAIN_BUDGET, self.tasks.wait())
            .await
            .is_err()
        {
            let active = self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .active
                .clone();
            tracing::error!(
                ?active,
                "scheduled shutdown unresolved after 10s; no synthetic terminal or task abort"
            );
            return Err(CoreError::other(format!(
                "scheduled shutdown unresolved after 10s: {} owned run(s) still active",
                active.len()
            )));
        }
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        match &state.failure {
            Some(error) => Err(CoreError::other(error.clone())),
            None => Ok(()),
        }
    }
}

impl Drop for OwnedRuns {
    fn drop(&mut self) {
        // Also covers an aborted/dropped loop future. Detached tasks continue
        // their normal cancellation finalization; never abort the kernel writer.
        self.cancel_owned();
        self.tasks.close();
        if !self.tasks.is_empty() {
            tracing::warn!(
                active = self.tasks.len(),
                "scheduled owner dropped; cancellation sent, finalization still pending"
            );
        }
    }
}

struct CompletionGuard {
    chat: Arc<ChatService>,
    state: Arc<Mutex<State>>,
    run_id: String,
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        // A finished/panicked worker is not itself evidence of durable outcome.
        // Retain the failure for shutdown instead of silently pruning the handle.
        let db = self.chat.database();
        let outcome = RunStore::new(&db).get(&self.run_id).and_then(|run| {
            if run.is_some_and(|run| run.finished_at.is_some()) {
                Ok(())
            } else {
                Err(CoreError::Persistence(
                    "scheduled worker exited without durable terminal".into(),
                ))
            }
        });
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.active.remove(&self.run_id);
        if let Err(error) = outcome {
            let message = format!("scheduled run {} finalization: {error}", self.run_id);
            tracing::error!(%message, "scheduled shutdown has an unresolved durable outcome");
            state.failure.get_or_insert(message);
        }
    }
}
