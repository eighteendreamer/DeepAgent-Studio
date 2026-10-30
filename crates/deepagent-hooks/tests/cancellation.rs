//! H-236: the common dispatcher owns cancellation, including compatibility
//! calls and wrappers. Only temporary resources and loopback HTTP are used.

use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use deepagent_core::{error::Result, id::SessionId};
use deepagent_hooks::{
    Hook, HookAction, HookActionExecutor, HookContext, HookData, HookDefinitions, HookOutcome,
    HookPoint, HookRegistry, OnceHook, SystemHookRunner,
};
use deepagent_tools::ToolExecutionContext;

fn context() -> HookContext {
    HookContext::new(SessionId::nil(), HookPoint::SessionStart, HookData::None)
}

struct CooperatingHook {
    name: &'static str,
    entered: Arc<tokio::sync::Barrier>,
    cleaned: Arc<AtomicUsize>,
}

#[async_trait]
impl Hook for CooperatingHook {
    fn name(&self) -> &str {
        self.name
    }

    async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
        panic!("the dispatcher and OnceHook must forward execution controls")
    }

    async fn run_with_context(
        &self,
        _: &HookContext,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        self.entered.wait().await;
        execution.cancelled().await;
        // Resource cleanup can legitimately take longer than MCP's 50ms
        // notification-write cap. It must not be cut to fit a probe window.
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.cleaned.fetch_add(1, Ordering::AcqRel);
        Ok(HookOutcome::Continue)
    }
}

#[tokio::test]
async fn bound_parent_cancellation_drains_all_parallel_once_hooks() {
    let parent = ToolExecutionContext::default();
    let entered = Arc::new(tokio::sync::Barrier::new(3));
    let cleaned = Arc::new(AtomicUsize::new(0));
    let mut hooks = HookRegistry::new().with_execution_context(parent.clone());
    for name in ["first", "second"] {
        hooks.register(
            HookPoint::SessionStart,
            OnceHook::new(Arc::new(CooperatingHook {
                name,
                entered: entered.clone(),
                cleaned: cleaned.clone(),
            })),
        );
    }
    let ctx = context();
    let stop = async {
        entered.wait().await;
        parent.cancellation_token().cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(hooks.dispatch(&ctx), stop)
    })
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    assert_eq!(cleaned.load(Ordering::Acquire), 2);
}

struct DeadlineHost {
    entered: AtomicBool,
    cleaned: AtomicBool,
}

#[async_trait]
impl HookActionExecutor for DeadlineHost {
    async fn execute(&self, _: &HookAction, _: serde_json::Value) -> Result<HookOutcome> {
        panic!("host must receive invocation-local controls")
    }

    async fn execute_with_context(
        &self,
        _: &HookAction,
        _: serde_json::Value,
        execution: ToolExecutionContext,
    ) -> Result<HookOutcome> {
        assert_eq!(execution.timeout(), Duration::from_secs(1));
        self.entered.store(true, Ordering::Release);
        execution.cancelled().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.cleaned.store(true, Ordering::Release);
        Ok(HookOutcome::Continue)
    }
}

#[tokio::test]
async fn action_deadline_notifies_host_and_preserves_parent_and_sibling() {
    let host = Arc::new(DeadlineHost {
        entered: AtomicBool::new(false),
        cleaned: AtomicBool::new(false),
    });
    let definitions = HookDefinitions::parse(
        r#"{"hooks":{"SessionStart":[{"once":true,
        "hooks":[{"type":"mcp_tool","command":"mcp__test__gate","timeout":1}]}]}}"#,
    )
    .unwrap();
    let parent = ToolExecutionContext::default();
    let sibling = parent.child();
    let mut hooks = HookRegistry::new().with_execution_context(parent.clone());
    assert_eq!(
        definitions.register_into_with_host(&mut hooks, Arc::new(SystemHookRunner), host.clone()),
        1
    );
    let error = tokio::time::timeout(Duration::from_secs(3), hooks.dispatch(&context()))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(host.entered.load(Ordering::Acquire));
    assert!(host.cleaned.load(Ordering::Acquire));
    assert!(!parent.is_cancelled());
    assert!(!sibling.is_cancelled());
}

#[tokio::test]
async fn already_cancelled_scope_does_not_start_a_host_action() {
    let host = Arc::new(DeadlineHost {
        entered: AtomicBool::new(false),
        cleaned: AtomicBool::new(false),
    });
    let definitions = HookDefinitions::parse(
        r#"{"hooks":{"SessionStart":[{
        "hooks":[{"type":"mcp_tool","command":"mcp__test__gate","timeout":1}]}]}}"#,
    )
    .unwrap();
    let parent = ToolExecutionContext::default();
    parent.cancellation_token().cancel();
    let mut hooks = HookRegistry::new().with_execution_context(parent);
    definitions.register_into_with_host(&mut hooks, Arc::new(SystemHookRunner), host.clone());
    assert!(hooks
        .dispatch(&context())
        .await
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    assert!(!host.entered.load(Ordering::Acquire));
}

#[tokio::test]
async fn http_body_wait_is_cancelled_without_waiting_for_the_server() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let server_entered = entered.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0; 8192];
        let read = socket.read(&mut buf).await.unwrap();
        assert!(
            read > 0,
            "the HTTP client must send a request before body wait"
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n{")
            .await
            .unwrap();
        server_entered.notify_one();
        std::future::pending::<()>().await;
    });
    let definitions = HookDefinitions::parse(
        &serde_json::json!({"hooks":{"SessionStart":[{
        "hooks":[{"type":"http","url":format!("http://{address}"),"timeout":5}]}]}})
        .to_string(),
    )
    .unwrap();
    let parent = ToolExecutionContext::default();
    let mut hooks = HookRegistry::new().with_execution_context(parent.clone());
    definitions.register_into(&mut hooks, Arc::new(SystemHookRunner));
    let ctx = context();
    let stop = async {
        entered.notified().await;
        parent.cancellation_token().cancel();
    };
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(hooks.dispatch(&ctx), stop)
    })
    .await;
    server.abort();
    let _ = server.await;
    assert!(result
        .unwrap()
        .0
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
}

#[tokio::test]
async fn command_cancellation_stops_the_real_process_before_its_late_write() {
    let root = tempfile::tempdir().unwrap();
    let entered = root.path().join("entered");
    let late = root.path().join("late");
    let (shell, command) = if cfg!(windows) {
        ("powershell", format!(
            "[IO.File]::WriteAllText('{}','ready'); Start-Sleep -Seconds 2; [IO.File]::WriteAllText('{}','late')",
            entered.display().to_string().replace('\'', "''"),
            late.display().to_string().replace('\'', "''"),
        ))
    } else {
        (
            "sh",
            format!(
                "printf ready > '{}'; sleep 2; printf late > '{}'",
                entered.display().to_string().replace('\'', "'\\''"),
                late.display().to_string().replace('\'', "'\\''")
            ),
        )
    };
    let definitions = HookDefinitions::parse(
        &serde_json::json!({"hooks":{"SessionStart":[{
        "hooks":[{"type":"command","command":command,"shell":shell,"timeout":5}]}]}})
        .to_string(),
    )
    .unwrap();
    let parent = ToolExecutionContext::default();
    let mut hooks = HookRegistry::new().with_execution_context(parent.clone());
    definitions.register_into(&mut hooks, Arc::new(SystemHookRunner));
    let ctx = context();
    let stop = async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        parent.cancellation_token().cancel();
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(4), async {
        tokio::join!(hooks.dispatch(&ctx), stop)
    })
    .await
    .unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(
        !late.exists(),
        "cancelled process must not execute its late write"
    );
}
