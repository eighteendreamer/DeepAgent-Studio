//! The SDK code profile belongs to the selected SandboxBackend. It never
//! delegates source text to a shell and never changes the managed SDK's ACLs.
//! Windows implementation: an invocation-local LPAC, exact named Node registry
//! read capability (no network grants; other runtimes retain zero capabilities), a
//! read-only staged SDK and a private, bounded Job Object. Unsupported OSes
//! fail closed rather than using the ordinary direct command executor.

use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::code::CodeResponse;

use super::SandboxCodeRequest;

#[cfg(any(windows, test))]
mod response;
#[cfg(windows)]
mod windows;

pub(super) async fn execute(request: SandboxCodeRequest) -> Result<CodeResponse> {
    request.code.validate()?;
    #[cfg(windows)]
    {
        run_blocking(move |abandoned| windows::execute(request, abandoned)).await
    }
    #[cfg(not(windows))]
    {
        Err(CoreError::invalid(
            "CodeIsolationUnavailable: strict SDK profile is currently Windows-only",
        ))
    }
}

#[cfg(windows)]
async fn run_blocking(
    worker: impl FnOnce(std::sync::Arc<std::sync::atomic::AtomicBool>) -> Result<CodeResponse>
        + Send
        + 'static,
) -> Result<CodeResponse> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // spawn_blocking is not aborted when an async tool invocation is dropped.
    // Keep this ownership rule shared with the real-child drop regression.
    struct CancelOnDrop(Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let guard = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let abandoned = guard.0.clone();
    tokio::task::spawn_blocking(move || worker(abandoned))
        .await
        .map_err(|_| CoreError::other("CodeExecutionFailed: SDK worker failed"))?
}
