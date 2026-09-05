use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Context for a single mobile operation.
///
/// Carries the operation ID, deadline, cancellation token and device ID.
/// Backend implementations must check `is_cancelled()` before each external
/// call and respect the deadline.
#[derive(Debug, Clone)]
pub struct OperationContext {
    pub operation_id: String,
    pub device_id: String,
    pub deadline: Duration,
    cancel: CancellationToken,
}

impl OperationContext {
    pub fn new(operation_id: String, device_id: String, deadline: Duration) -> Self {
        Self {
            operation_id,
            device_id,
            deadline,
            cancel: CancellationToken::new(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Handle returned when an operation is in flight.
///
/// Dropping the handle does **not** cancel the operation; call `cancel()`
/// explicitly.
pub struct OperationHandle {
    ctx: OperationContext,
    join: tokio::task::JoinHandle<()>,
}

impl OperationHandle {
    pub fn new(ctx: OperationContext, join: tokio::task::JoinHandle<()>) -> Self {
        Self { ctx, join }
    }

    pub fn operation_id(&self) -> &str {
        &self.ctx.operation_id
    }

    pub fn cancel(&self) {
        self.ctx.cancel();
    }

    pub async fn wait(self) {
        let _ = self.join.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_not_cancelled_by_default() {
        let ctx = OperationContext::new("op-1".into(), "dev-1".into(), Duration::from_secs(30));
        assert!(!ctx.is_cancelled());
    }

    #[test]
    fn context_cancel_propagates() {
        let ctx = OperationContext::new("op-1".into(), "dev-1".into(), Duration::from_secs(30));
        let token = ctx.cancellation_token();
        ctx.cancel();
        assert!(ctx.is_cancelled());
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_token_wakes_waiter() {
        let ctx = OperationContext::new("op-1".into(), "dev-1".into(), Duration::from_secs(30));
        let token = ctx.cancellation_token();
        ctx.cancel();
        token.cancelled().await;
    }
}
