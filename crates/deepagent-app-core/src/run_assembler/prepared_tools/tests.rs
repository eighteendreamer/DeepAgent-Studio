use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

fn request() -> SubagentRequest {
    serde_json::from_value(serde_json::json!({
        "description":"synthetic forwarding", "prompt":"payload", "subagent_type":"explore",
        "allowed_tools":["read_file"], "model":"offline-model", "effort":"high", "skills":["synthetic"],
        "isolation":"shared", "fork":false, "phase":"review", "label":"probe", "schema":{"type":"object"}
    })).unwrap()
}

#[derive(Default)]
struct Probe {
    calls: Mutex<Vec<&'static str>>,
    flag: Arc<AtomicBool>,
}

impl Probe {
    fn record(&self, method: &'static str) {
        self.calls.lock().unwrap().push(method);
    }

    fn context(&self, context: &ToolExecutionContext) {
        assert!(Arc::ptr_eq(&self.flag, &context.cancel_flag()));
        assert!(
            context.is_cancelled(),
            "do not replace the caller's cancellation context"
        );
        assert_eq!(context.timeout(), Duration::from_secs(7));
    }
}

#[async_trait]
impl SubagentRunner for Probe {
    async fn run(&self, input: SubagentRequest) -> Result<String> {
        assert_eq!(input, request());
        self.record("run");
        Ok("run-output".into())
    }
    async fn run_controlled(
        &self,
        input: SubagentRequest,
        context: ToolExecutionContext,
    ) -> Result<SubagentOutcome> {
        assert_eq!(input, request());
        self.context(&context);
        self.record("run_controlled");
        Ok(SubagentOutcome {
            result: "controlled-output".into(),
            tool_use_count: 3,
            duration_ms: 4,
            tokens: 5,
        })
    }
    async fn start_background(
        &self,
        input: SubagentRequest,
        context: ToolExecutionContext,
    ) -> Result<BackgroundSubagent> {
        assert_eq!(input, request());
        self.context(&context);
        self.record("start_background");
        Ok(BackgroundSubagent {
            id: "child".into(),
            state: "running".into(),
        })
    }
    async fn status(&self, id: &str) -> Result<SubagentStatus> {
        assert_eq!(id, "child");
        self.record("status");
        Ok(SubagentStatus {
            id: id.into(),
            state: "succeeded".into(),
            summary: Some("complete".into()),
            worktree_path: Some("synthetic-worktree".into()),
        })
    }
    async fn cancel(&self, id: &str) -> Result<bool> {
        assert_eq!(id, "child");
        self.record("cancel");
        Ok(true)
    }
    async fn resume(
        &self,
        id: &str,
        prompt: &str,
        context: ToolExecutionContext,
    ) -> Result<String> {
        assert_eq!((id, prompt), ("child", "continuation"));
        self.context(&context);
        self.record("resume");
        Ok("resume-output".into())
    }
    async fn cleanup(&self, id: &str) -> Result<bool> {
        assert_eq!(id, "child");
        self.record("cleanup");
        Ok(true)
    }
}

#[async_trait]
impl PlanExecutor for Probe {
    async fn execute_plan(&self, goal: String) -> Result<PlanExecutionResult> {
        assert_eq!(goal, "goal");
        self.record("execute_plan");
        Ok(PlanExecutionResult {
            report: "plan-output".into(),
            dag_execution: None,
        })
    }
    async fn execute_plan_controlled(
        &self,
        goal: String,
        context: ToolExecutionContext,
    ) -> Result<PlanExecutionResult> {
        assert_eq!(goal, "goal");
        self.context(&context);
        self.record("execute_plan_controlled");
        Ok(PlanExecutionResult {
            report: "controlled-plan-output".into(),
            dag_execution: None,
        })
    }
}

async fn call_every_method(services: &RunToolServices, context: ToolExecutionContext, bound: bool) {
    let run = services.run(request()).await;
    let controlled = services.run_controlled(request(), context.clone()).await;
    let background = services.start_background(request(), context.clone()).await;
    let status = services.status("child").await;
    let cancel = services.cancel("child").await;
    let resume = services
        .resume("child", "continuation", context.clone())
        .await;
    let cleanup = services.cleanup("child").await;
    let plan = services.execute_plan("goal".into()).await;
    let controlled_plan = services
        .execute_plan_controlled("goal".into(), context)
        .await;
    if bound {
        assert_eq!(run.unwrap(), "run-output");
        assert_eq!(
            controlled.unwrap(),
            SubagentOutcome {
                result: "controlled-output".into(),
                tool_use_count: 3,
                duration_ms: 4,
                tokens: 5
            }
        );
        assert_eq!(
            background.unwrap(),
            BackgroundSubagent {
                id: "child".into(),
                state: "running".into()
            }
        );
        assert_eq!(
            status.unwrap().worktree_path.as_deref(),
            Some("synthetic-worktree")
        );
        assert!(cancel.unwrap());
        assert_eq!(resume.unwrap(), "resume-output");
        assert!(cleanup.unwrap());
        assert_eq!(plan.unwrap().report, "plan-output");
        assert_eq!(controlled_plan.unwrap().report, "controlled-plan-output");
    } else {
        for error in [
            run.err(),
            controlled.err(),
            background.err(),
            status.err(),
            cancel.err(),
            resume.err(),
            cleanup.err(),
            plan.err(),
            controlled_plan.err(),
        ] {
            assert!(error.unwrap().to_string().contains("not bound"));
        }
    }
}

#[tokio::test]
async fn h219_prepared_services_reject_before_binding_and_forward_every_existing_method_once() {
    let services = RunToolServices::default();
    let probe = Arc::new(Probe::default());
    probe.flag.store(true, Ordering::Release);
    let context =
        ToolExecutionContext::new(probe.flag.clone()).with_timeout(Duration::from_secs(7));
    call_every_method(&services, context.clone(), false).await;
    assert!(probe.calls.lock().unwrap().is_empty());
    assert!(services
        .0
        .set(BoundRunServices {
            runner: probe.clone(),
            plan: probe.clone()
        })
        .is_ok());
    let replacement = Arc::new(Probe::default());
    assert!(services
        .0
        .set(BoundRunServices {
            runner: replacement.clone(),
            plan: replacement.clone()
        })
        .is_err());
    call_every_method(&services.clone(), context, true).await;
    assert!(replacement.calls.lock().unwrap().is_empty());
    assert_eq!(
        *probe.calls.lock().unwrap(),
        [
            "run",
            "run_controlled",
            "start_background",
            "status",
            "cancel",
            "resume",
            "cleanup",
            "execute_plan",
            "execute_plan_controlled"
        ]
    );
}

#[derive(Default)]
struct NeverTransport(AtomicUsize);

#[async_trait]
impl HttpTransport for NeverTransport {
    async fn stream(
        &self,
        _: deepagent_models::transport::TransportRequest,
        _: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        self.0.fetch_add(1, Ordering::AcqRel);
        panic!("preparation must not issue a model request")
    }
    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        self.0.fetch_add(1, Ordering::AcqRel);
        panic!("preparation must not perform model discovery")
    }
}

#[tokio::test]
async fn h219_real_main_registry_can_prepare_without_model_credentials_admission_or_execution() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let transport = Arc::new(NeverTransport::default());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        transport.clone(),
        Arc::new(crate::secret_store::MemorySecretStore::new()),
    ));
    assert!(settings.load().unwrap().is_none());
    let session = deepagent_session::Session::create(&db, &SystemClock, None)
        .unwrap()
        .id();
    let session_id = session.to_string();
    let before = EventStore::new(&db).load_session(session).unwrap();
    let chat = ChatService::new(db.clone(), settings.clone(), transport.clone(), root.path());
    let assembler = chat.run_assembler();
    let base = || {
        assembler.base_registry_request(
            root.path(),
            deepagent_builtins::FsAccess::ReadOnly,
            None,
            None,
            Some(crate::settings::LocalExecutionMode::Direct),
            false,
            None,
            true,
            &session_id,
        )
    };
    let prepared = assembler
        .prepare_main_run_tools(MainRunPreparationRequest {
            base: base(),
            sub_base: base(),
            plugin_projection: None,
            plugin_registry: None,
            plan: deepagent_builtins::PlanMode::for_session(db.clone(), &session_id).unwrap(),
            discovered: Arc::default(),
            search_mode: deepagent_builtins::ToolSearchMode::Disabled,
            search_threshold: 100,
        })
        .unwrap();
    for name in ["task", "plan_execute", "snip_history", "read_file"] {
        assert!(
            prepared.toolset.registry.get(name).is_some(),
            "missing real tool {name}"
        );
        assert!(prepared
            .toolset
            .manifest
            .tools
            .iter()
            .any(|tool| tool.function.name == name));
    }
    assert!(prepared.sub_registry.get("task").is_none());
    for (name, arguments) in [
        (
            "task",
            serde_json::json!({"description":"probe", "prompt":"must not execute"}),
        ),
        (
            "plan_execute",
            serde_json::json!({"goal":"must not execute"}),
        ),
    ] {
        let output = prepared
            .toolset
            .registry
            .get(name)
            .unwrap()
            .tool
            .invoke(arguments)
            .await
            .unwrap();
        assert!(!output.ok);
        assert!(output.value["error"]
            .as_str()
            .unwrap()
            .contains("not bound"));
    }
    assert_eq!(transport.0.load(Ordering::Acquire), 0);
    assert!(settings.load().unwrap().is_none());
    assert!(deepagent_persistence::run_store::RunStore::new(&db)
        .list_for_session(&session_id)
        .unwrap()
        .is_empty());
    assert_eq!(EventStore::new(&db).load_session(session).unwrap(), before);
    assert!(!root.path().join(".deepagent").exists());
}
