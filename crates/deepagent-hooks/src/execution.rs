//! Hook resource owners share the existing invocation controls, not a second
//! cancellation registry. Cancellation signals the child before bounded cleanup.

use std::future::Future;
use std::sync::atomic::Ordering;
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};
use deepagent_tools::ToolExecutionContext;

// Local design: give resource owners a bounded grace, comfortably above MCP's
// existing 50ms notification-write cap. This is not a remote rollback receipt.
const HOOK_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) async fn run_controlled<F, T>(
    parent: &ToolExecutionContext,
    child: &ToolExecutionContext,
    deadline: Option<Duration>,
    operation: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if parent.is_cancelled() {
        return Err(CoreError::other("hook cancelled by user before dispatch"));
    }
    tokio::pin!(operation);
    let deadline = async {
        match deadline {
            Some(duration) => tokio::time::sleep(duration).await,
            None => std::future::pending::<()>().await,
        }
    };
    let reason = tokio::select! {
        biased;
        _ = parent.cancelled() => "hook cancelled by user",
        _ = deadline => "hook timed out",
        result = &mut operation => return result,
    };
    // Child controls have their own flag: never cancel the caller or siblings
    // when a single hook reaches its own deadline.
    child.cancel_flag().store(true, Ordering::Release);
    child.cancellation_token().cancel();
    match tokio::time::timeout(HOOK_CLEANUP_TIMEOUT, &mut operation).await {
        Ok(Ok(_)) => Err(CoreError::other(reason)),
        Ok(Err(error)) => Err(CoreError::other(format!("{reason}: {error}"))),
        Err(_) => {
            tracing::warn!(
                reason,
                "hook cleanup exceeded its budget; remote effects are unknown"
            );
            Err(CoreError::other(format!(
                "{reason}; cleanup exceeded its budget; external effects may still be finishing"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[tokio::test]
    async fn deadline_signals_child_without_cancelling_parent_or_sibling() {
        let parent = ToolExecutionContext::default();
        let child = parent.child();
        let sibling = parent.child();
        let cleaned = Arc::new(AtomicBool::new(false));
        let result = run_controlled(&parent, &child, Some(Duration::from_millis(20)), async {
            child.cancelled().await;
            assert!(child.cancel_flag().load(Ordering::Acquire));
            cleaned.store(true, Ordering::Release);
            Ok(())
        })
        .await;
        assert!(result.unwrap_err().to_string().contains("timed out"));
        assert!(cleaned.load(Ordering::Acquire));
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
    }

    #[tokio::test]
    async fn legacy_flag_cancellation_is_forwarded_to_child_token() {
        let flag = Arc::new(AtomicBool::new(false));
        let parent = ToolExecutionContext::new(flag.clone());
        let child = parent.child();
        let operation = run_controlled(&parent, &child, None, async {
            flag.store(true, Ordering::Release);
            child.cancelled().await;
            Ok(())
        });
        let result = tokio::time::timeout(Duration::from_secs(2), operation)
            .await
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(child.cancellation_token().is_cancelled());
    }
}
