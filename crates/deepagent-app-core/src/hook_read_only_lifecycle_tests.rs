//! H234: real private RuntimeEngine dispatch, not a simulated tool callback.
use super::*;
use crate::responses_fixture::{call, completed, text};
use deepagent_builtins::todo_tool::{TodoItem, TodoStatus, TodoStore, TodoWriteTool};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::ModelConfig;
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::Notify;

fn host(registry: ToolRegistry, transport: Arc<dyn HttpTransport>) -> AppHookActionExecutor {
    let sink: Arc<dyn RuntimeEventSink> = Arc::new(NoopEventSink);
    AppHookActionExecutor {
        client: Arc::new(ModelClient::new(
            transport,
            ModelConfig::deepseek("h234-synthetic"),
        )),
        model: "deepseek-flash".into(),
        reasoning_effort: ReasoningEffort::None,
        mcp: None,
        agent_registry: Arc::new(registry),
        agent_policy: Arc::new(HookRegistry::new()),
        agent_permissions: PermissionSet::developer(),
        events: Arc::downgrade(&sink),
        usage_owner: None,
    }
}

fn action() -> HookAction {
    HookAction {
        action_type: HookActionType::Agent,
        prompt: "H234 synthetic observer".into(),
        timeout: Some(5),
        ..Default::default()
    }
}

fn result(body: &Value) -> Option<Value> {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .map(|item| serde_json::from_str(item["output"].as_str().unwrap()).unwrap())
}

fn emit(sink: &mut dyn EventSink, item: Value) -> Result<()> {
    for event in completed(
        "h234-response",
        "deepseek-flash",
        item,
        json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
    ) {
        if sink.on_event(&event)? {
            break;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Finish {
    Allow,
    Deny,
    Malformed,
    Cancel,
    Drop,
}

struct Attempt {
    finish: Finish,
    entered: Notify,
    waiting_dropped: Arc<AtomicBool>,
    rejected: AtomicUsize,
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl HttpTransport for Attempt {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert!(!body["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "todo_write")));
        if let Some(output) = result(&body) {
            assert_eq!(output["status"], "error");
            assert!(output.to_string().contains("unknown tool"));
            self.rejected.fetch_add(1, Ordering::SeqCst);
            let decision = match self.finish {
                Finish::Allow => r#"{"ok":true}"#,
                Finish::Deny => r#"{"ok":false,"reason":"H234 controlled denial"}"#,
                Finish::Malformed => "not a JSON decision",
                Finish::Cancel | Finish::Drop => {
                    let _guard = Dropped(self.waiting_dropped.clone());
                    self.entered.notify_one();
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            };
            emit(sink, text("h234-decision", decision))
        } else {
            // A provider can return a call that was not advertised. Real
            // RuntimeEngine validation must reject it before any state write.
            emit(
                sink,
                call(
                    "h234-attempt",
                    "todo_write",
                    json!({"todos":[{
                        "content":"H234 private canary","status":"pending","active_form":"Changing parent"
                    }]}),
                ),
            )
        }
    }
}

#[tokio::test]
async fn h234_parent_state_survives_allow_deny_invalid_decision_cancel_and_drop() {
    for finish in [
        Finish::Allow,
        Finish::Deny,
        Finish::Malformed,
        Finish::Cancel,
        Finish::Drop,
    ] {
        let store = TodoStore::new();
        let original = vec![TodoItem {
            content: "H234 parent".into(),
            active_form: "Tracking parent".into(),
            status: TodoStatus::Pending,
        }];
        store.replace(original.clone());
        store.mark_snapshot_pending();
        let mut source = ToolRegistry::new();
        source
            .register(Arc::new(TodoWriteTool::new(store.clone())))
            .unwrap();
        let model = Arc::new(Attempt {
            finish,
            entered: Notify::new(),
            waiting_dropped: Default::default(),
            rejected: AtomicUsize::new(0),
        });
        let host = Arc::new(host(source, model.clone()));
        let parent = ToolExecutionContext::default();
        let execution = parent.child();
        // Exercise the production lifecycle owner, which signals both child
        // controls and bounds cleanup. Calling the host alone bypasses that
        // owner and is not the public cancellation path.
        let definitions = HookDefinitions::parse(
            r#"{"hooks":{"UserPromptSubmit":[{"hooks":[{"type":"agent","prompt":"H234 synthetic observer","timeout":5}]}]}}"#,
        )
        .unwrap();
        let mut hooks = HookRegistry::new().with_execution_context(execution.clone());
        definitions.register_into_with_host(&mut hooks, Arc::new(SystemHookRunner), host);
        let context = HookContext::new(
            deepagent_core::id::SessionId::nil(),
            deepagent_hooks::HookPoint::UserPromptSubmit,
            HookData::Prompt {
                text: "H234 synthetic input".into(),
            },
        );
        let mut run = Box::pin(hooks.dispatch(&context));
        if matches!(finish, Finish::Cancel | Finish::Drop) {
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    _ = model.entered.notified() => {},
                    result = &mut run => panic!("must reach rejected-call decision wait: {result:?}"),
                }
            }).await.unwrap();
            if matches!(finish, Finish::Cancel) {
                execution.cancellation_token().cancel();
                assert!(tokio::time::timeout(Duration::from_secs(2), &mut run)
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("cancel"));
            }
            drop(run);
            assert!(model.waiting_dropped.load(Ordering::SeqCst));
            assert!(!parent.is_cancelled());
        } else {
            let result = tokio::time::timeout(Duration::from_secs(5), run)
                .await
                .unwrap();
            match finish {
                Finish::Allow => assert_eq!(result.unwrap(), HookOutcome::Continue),
                Finish::Deny => assert!(matches!(result.unwrap(), HookOutcome::Deny { .. })),
                Finish::Malformed => assert!(result.is_err()),
                _ => unreachable!(),
            }
        }
        assert_eq!(model.rejected.load(Ordering::SeqCst), 1, "{finish:?}");
        assert_eq!(store.snapshot(), original, "{finish:?}");
        assert!(store.is_snapshot_pending(), "{finish:?}");
    }
}

// These counters are synthetic instrumentation for materialization lifetime,
// not a production capability whose state is classified as read-only.
struct Probe {
    factories: Arc<AtomicUsize>,
    local: AtomicUsize,
}
#[async_trait]
impl Tool for Probe {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "h234_probe".into(),
            description: "Synthetic observer lifetime probe".into(),
            parameters: json!({"type":"object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::empty(),
        }
    }
    fn read_only_projection(self: Arc<Self>) -> Option<Arc<dyn Tool>> {
        self.factories.fetch_add(1, Ordering::SeqCst);
        Some(Arc::new(Self {
            factories: self.factories.clone(),
            local: AtomicUsize::new(0),
        }))
    }
    async fn invoke(&self, _: Value) -> Result<ToolOutput> {
        Ok(ToolOutput::success(
            json!({"seen":self.local.fetch_add(1, Ordering::SeqCst)+1}),
        ))
    }
}

struct Observer;
#[async_trait]
impl HttpTransport for Observer {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        if let Some(output) = result(&body) {
            assert_eq!(output["status"], "ok");
            assert_eq!(output["result"]["seen"], 1);
            emit(sink, text("h234-allow", r#"{"ok":true}"#))
        } else {
            emit(sink, call("h234-read", "h234_probe", json!({})))
        }
    }
}

#[tokio::test]
async fn h234_sequential_and_concurrent_hook_agents_materialize_independent_observers() {
    let factories = Arc::new(AtomicUsize::new(0));
    let mut source = ToolRegistry::new();
    source
        .register(Arc::new(Probe {
            factories: factories.clone(),
            local: AtomicUsize::new(0),
        }))
        .unwrap();
    let host = host(source, Arc::new(Observer));
    let action = action();
    for _ in 0..2 {
        assert_eq!(
            host.execute(&action, json!({})).await.unwrap(),
            HookOutcome::Continue
        );
    }
    let (left, right) = tokio::join!(
        host.execute(&action, json!({})),
        host.execute(&action, json!({}))
    );
    assert_eq!(left.unwrap(), HookOutcome::Continue);
    assert_eq!(right.unwrap(), HookOutcome::Continue);
    assert_eq!(factories.load(Ordering::SeqCst), 4);
}
