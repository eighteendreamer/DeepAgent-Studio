use super::*;
use deepagent_hooks::{Hook, OnceHook};
use deepagent_tools::ToolExecutionContext;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::sync::Notify;

struct PendingHook {
    calls: Arc<AtomicUsize>,
    entered: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

struct Guard(Arc<AtomicBool>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl Hook for PendingHook {
    fn name(&self) -> &str {
        "synthetic_pending_once"
    }

    fn applies(&self, ctx: &HookContext) -> bool {
        matches!(&ctx.data, HookData::Tool {name, ..} if name == "write_file")
    }

    async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
        unreachable!("resource controls must be forwarded")
    }

    async fn run_with_context(
        &self,
        _: &HookContext,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let _guard = Guard(self.dropped.clone());
        self.entered.notify_one();
        execution.cancelled().await;
        Ok(HookOutcome::Continue)
    }
}

#[tokio::test]
async fn h240_pre_cancelled_direct_call_preserves_once_but_admitted_drop_consumes_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let once = OnceHook::new(OnceHook::new(Arc::new(PendingHook {
        calls: calls.clone(),
        entered: entered.clone(),
        dropped: dropped.clone(),
    })));
    let ctx = context("write_file", "allowed.txt");
    assert_eq!(
        once.run(&context("read_file", "primer")).await.unwrap(),
        HookOutcome::Continue
    );
    let cancelled = ToolExecutionContext::default();
    cancelled.cancellation_token().cancel();
    assert!(once
        .run_with_context(&ctx, cancelled)
        .await
        .unwrap_err()
        .to_string()
        .contains("before admission"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut pending = Box::pin(once.run_with_context(&ctx, ToolExecutionContext::default()));
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            _ = entered.notified() => {},
            result = &mut pending => panic!("expected a real pending invocation: {result:?}"),
        }
    })
    .await
    .unwrap();
    drop(pending);
    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(once.run(&ctx).await.unwrap(), HookOutcome::Continue);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h240_cancellation_after_admission_cleans_and_does_not_rearm_cloned_owner() {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let parent = ToolExecutionContext::default();
    let mut registry = HookRegistry::new().with_execution_context(parent.clone());
    registry.register(
        HookPoint::BeforeToolUse,
        OnceHook::new(Arc::new(PendingHook {
            calls: calls.clone(),
            entered: entered.clone(),
            dropped: dropped.clone(),
        })),
    );
    let ctx = context("write_file", "allowed.txt");
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(registry.dispatch(&ctx), async {
            entered.notified().await;
            parent.cancellation_token().cancel();
        })
    })
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert!(dropped.load(Ordering::SeqCst));
    let next = registry
        .clone()
        .with_execution_context(ToolExecutionContext::default());
    assert_eq!(next.dispatch(&ctx).await.unwrap(), HookOutcome::Continue);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct DeadlineHost(Arc<AtomicUsize>);

#[async_trait]
impl HookActionExecutor for DeadlineHost {
    async fn execute(&self, _: &HookAction, _: Value) -> Result<HookOutcome> {
        unreachable!("must receive the deadline controls")
    }

    async fn execute_with_context(
        &self,
        _: &HookAction,
        _: Value,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        execution.cancelled().await;
        Ok(HookOutcome::Continue)
    }
}

#[tokio::test]
async fn h240_native_host_timeout_consumes_once_without_cancelling_caller() {
    let calls = Arc::new(AtomicUsize::new(0));
    let parent = ToolExecutionContext::default();
    let mut registry = HookRegistry::new().with_execution_context(parent.clone());
    definitions(action("mcp_tool"), "both").register_into_with_host(
        &mut registry,
        Arc::new(SystemHookRunner),
        Arc::new(DeadlineHost(calls.clone())),
    );
    registry
        .dispatch(&context("read_file", "primer"))
        .await
        .unwrap();
    let ctx = context("write_file", "allowed.txt");
    let error = tokio::time::timeout(Duration::from_secs(3), registry.dispatch(&ctx))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert_eq!(
        registry.dispatch(&ctx).await.unwrap(),
        HookOutcome::Continue
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(!parent.is_cancelled());
}
