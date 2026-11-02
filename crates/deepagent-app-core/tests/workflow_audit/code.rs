//! Code-node admission regressions. These use no interpreter, network, user
//! SDK or OS sandbox: rejection must happen through the existing Kernel gates.
//! The opt-in diagnostic below only probes an explicitly provided SDK read-only.
//! Real script execution and containment acceptance is a separate required gate.
use super::*;
use deepagent_app_core::{settings::PermissionPreset, ApprovalRequestDto};
use deepagent_core::error::Result;
use deepagent_runtime::workflow::NodeExecutionEvent;
use serde_json::json;

const SCRIPT: &str =
    "function main({ input }) { return { result: input + 'H178_PRIVATE_SOURCE' }; }";

#[path = "code/persistence.rs"]
mod persistence;

fn request(language: &str, script: &str) -> WorkflowRequest {
    serde_json::from_value(json!({
        "definition":{"version":1,"nodes":[
            {"id":"start","kind":"start"},
            {"id":"code","kind":"code","config":{
                "codeLanguage":language,
                "codeScript":script,
                "codeInputVariables":[{"name":"input","value":"H178_PRIVATE_INPUT"}],
                "codeOutputVariables":[{"name":"result","type":"string"}]
            }},
            {"id":"end","kind":"end"}
        ],"edges":[
            {"id":"a","source":"start","target":"code"},
            {"id":"b","source":"code","target":"end"}
        ]}
    }))
    .unwrap()
}

async fn full_access_fixture() -> (tempfile::TempDir, Arc<Database>, ChatService, String) {
    let (root, db, _chat, session) = workflow_fixture().await;
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::default()),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(MockTransport::default()),
        root.path(),
    );
    (root, db, chat, session)
}

async fn run(
    chat: &ChatService,
    request: WorkflowRequest,
    run_id: &str,
    session: String,
    approval: impl Fn(ApprovalRequestDto) + Send + Sync + 'static,
) -> (Result<String>, Vec<NodeExecutionEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let result = chat
        .run_workflow(
            request,
            run_id.into(),
            session,
            move |event| {
                if let RuntimeEvent::WorkflowNode { event } = event {
                    capture.lock().unwrap().push(event);
                }
            },
            approval,
        )
        .await;
    let captured = events.lock().unwrap().clone();
    (result, captured)
}

pub(super) fn assert_failed(
    db: &Database,
    run_id: &str,
    result: &Result<String>,
    events: &[NodeExecutionEvent],
) {
    assert!(
        result.is_err(),
        "code must not fabricate success: {result:?}"
    );
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| event.node_id == "code" && event.status.is_terminal())
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].status, NodeExecutionStatus::Failed);
    assert!(terminal[0].outputs.is_none());
    assert!(terminal[0]
        .error
        .as_deref()
        .is_some_and(|error| !error.is_empty()));
    assert!(!events.iter().any(|event| {
        matches!(event.node_id.as_str(), "code" | "end")
            && event.status == NodeExecutionStatus::Completed
    }));
    let store = RunStore::new(db);
    assert_eq!(
        store.get(run_id).unwrap().unwrap().terminal_kind.as_deref(),
        Some("failed")
    );
    let ledger = store.events_after(run_id, None).unwrap();
    let terminal: Vec<_> = ledger
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "failed");
    assert!(
        !ledger.iter().any(|event| {
            let text = event.data.to_string();
            text.contains("H178_PRIVATE_SOURCE") || text.contains("H178_PRIVATE_INPUT")
        }),
        "script source and input values must not enter the action/audit arguments"
    );
}

#[tokio::test]
async fn h178_code_missing_sdk_never_fabricates_a_completed_output() {
    for language in ["javascript", "python"] {
        let (_root, db, chat, session) = full_access_fixture().await;
        let (result, events) = run(
            &chat,
            request(language, SCRIPT),
            "h178-no-sdk",
            session,
            |_| panic!("FullAccess must not ask"),
        )
        .await;
        assert_failed(&db, "h178-no-sdk", &result, &events);
    }
}

#[tokio::test]
async fn h178_code_empty_or_unknown_language_is_not_silently_accepted() {
    for (language, script) in [("javascript", ""), ("python", " \n\t"), ("ruby", SCRIPT)] {
        let (_root, db, chat, session) = full_access_fixture().await;
        let (result, events) = run(
            &chat,
            request(language, script),
            "h178-invalid-code",
            session,
            |_| panic!("invalid script configuration cannot ask"),
        )
        .await;
        assert_failed(&db, "h178-invalid-code", &result, &events);
    }
}

#[tokio::test]
async fn h178_code_approval_denial_uses_the_kernel_and_never_completes() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let responder = chat.clone();
    let approvals = Arc::new(AtomicUsize::new(0));
    let count = approvals.clone();
    let (result, events) = run(
        &chat,
        request("javascript", SCRIPT),
        "h178-denied",
        session,
        move |approval| {
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(approval.tool, "canvas_code_execute");
            assert!(responder
                .resolve_approval(&approval.call_id, false, "h178 synthetic denial")
                .unwrap());
        },
    )
    .await;
    assert_eq!(approvals.load(Ordering::SeqCst), 1);
    assert_failed(&db, "h178-denied", &result, &events);
    let ledger = RunStore::new(&db)
        .events_after("h178-denied", None)
        .unwrap();
    assert!(ledger
        .iter()
        .any(|event| event.event_type == "approval.requested"));
    assert!(!ledger
        .iter()
        .any(|event| event.event_type == "tool_started"));
}

#[tokio::test]
async fn h178_code_explicit_deny_precedes_full_access_and_sdk_resolution() {
    let (root, db, _chat, session) = full_access_fixture().await;
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::default()),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_permission_rules(deepagent_hooks::PermissionRules::new(
            ["canvas_code_execute".into()],
            ["canvas_code_execute".into()],
            ["canvas_code_execute".into()],
        ))
        .unwrap();
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(MockTransport::default()),
        root.path(),
    );
    let (result, events) = run(
        &chat,
        request("javascript", SCRIPT),
        "h178-rule-denied",
        session,
        |_| panic!("explicit deny must precede approval"),
    )
    .await;
    assert_failed(&db, "h178-rule-denied", &result, &events);
    assert!(events.iter().any(|event| {
        event.node_id == "code"
            && event
                .error
                .as_deref()
                .is_some_and(|error| error.contains("denied by permission rule"))
    }));
    assert!(!RunStore::new(&db)
        .events_after("h178-rule-denied", None)
        .unwrap()
        .iter()
        .any(|event| matches!(
            event.event_type.as_str(),
            "tool_started" | "approval.requested"
        )));
}

#[tokio::test]
async fn h178_code_obeys_durable_plan_mode_before_an_sdk_can_run() {
    let (_root, db, chat, session) = full_access_fixture().await;
    assert!(chat.set_plan_mode(&session, true).unwrap());
    let (result, events) = run(
        &chat,
        request("javascript", SCRIPT),
        "h178-plan-denied",
        session,
        |_| panic!("plan mode must not ask to execute code"),
    )
    .await;
    assert_failed(&db, "h178-plan-denied", &result, &events);
    assert!(events.iter().any(|event| {
        event.node_id == "code"
            && event
                .error
                .as_deref()
                .is_some_and(|error| error.contains("Plan mode: write operations are disabled"))
    }));
    assert!(!RunStore::new(&db)
        .events_after("h178-plan-denied", None)
        .unwrap()
        .iter()
        .any(|event| matches!(
            event.event_type.as_str(),
            "tool_started" | "approval.requested"
        )));
}

#[tokio::test]
async fn h178_code_free_control_keeps_the_existing_successful_run() {
    let (_root, db, chat, session) = workflow_fixture().await;
    let mut request = request("javascript", SCRIPT);
    request.definition.nodes.remove(1);
    request.definition.edges.truncate(1);
    request.definition.edges[0].target = "end".into();
    let (result, events) = run(&chat, request, "h178-normal-control", session, |_| {
        panic!("a code-free workflow has no execution approval")
    })
    .await;
    result.unwrap();
    assert_eq!(
        RunStore::new(&db)
            .get("h178-normal-control")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| {
                event.node_id == "end" && event.status == NodeExecutionStatus::Completed
            })
            .count(),
        1
    );
}

/// Diagnostic, not script-execution acceptance. An explicit fixture root keeps
/// normal CI independent of any host install; the real SDK is a lookup root,
/// never an install/state root, and no download or arbitrary script is run.
#[test]
#[ignore = "requires H178_MANAGED_SDK_FIXTURE_ROOT pointing to an opt-in managed SDK fixture"]
fn h178_existing_broker_resolves_opt_in_managed_node_read_only() {
    use deepagent_app_core::runtime_service::{
        RuntimeBroker, RuntimeKind, RuntimePreference, RuntimeRequirement, RuntimeService,
        RuntimeSource, UnavailableDownloader,
    };

    let lookup = std::path::PathBuf::from(
        std::env::var_os("H178_MANAGED_SDK_FIXTURE_ROOT").expect("explicit SDK fixture required"),
    );
    let active = tempfile::tempdir().unwrap();
    let broker = RuntimeBroker::new(Arc::new(RuntimeService::with_lookup_roots(
        &[active.path().to_path_buf()],
        std::slice::from_ref(&lookup),
        Arc::new(UnavailableDownloader),
    )));
    let found = broker
        .resolve(
            &RuntimeRequirement {
                kind: RuntimeKind::Node,
                version: Some("24.21.0".into()),
                preference: RuntimePreference::ManagedOnly,
            },
            None,
            None,
        )
        .expect("the existing broker must resolve the opted-in managed Node");
    assert_eq!(found.source, RuntimeSource::Managed);
    assert_eq!(found.version, "24.21.0");
    assert!(found.executable.starts_with(&lookup));
    assert!(!found.executable.starts_with(active.path()));
    println!(
        "source={:?} version={} reason={} (version probe only; no script or sandbox acceptance)",
        found.source, found.version, found.reason
    );
}

/// Real execution acceptance, deliberately distinct from the version probe.
/// This must stay red until an actual strict backend executes the SDK child;
/// an unavailable/error-only implementation cannot satisfy this assertion.
#[tokio::test]
#[ignore = "requires explicit H178_MANAGED_SDK_FIXTURE_ROOT; exercises real managed Node"]
async fn h178_real_managed_javascript_computes_via_strict_backend() {
    let (_root, db, chat, session) = full_access_fixture().await;
    let (_active, chat) = with_real_node(chat);
    let request = numeric_workflow("function main({ input }) { return { result: input * 7 }; }");
    let (result, events) = run(&chat, request, "h178-real-compute", session, |_| {
        panic!("FullAccess does not ask, but still requires strict code isolation")
    })
    .await;
    assert!(
        result.is_ok(),
        "real SDK computation is required, not unavailable: {result:?}"
    );
    let code = events
        .iter()
        .filter(|event| event.node_id == "code" && event.status.is_terminal())
        .collect::<Vec<_>>();
    assert_eq!(code.len(), 1);
    assert_eq!(code[0].status, NodeExecutionStatus::Completed);
    assert_eq!(code[0].outputs, Some(json!({"result":42})));
    assert_eq!(
        RunStore::new(&db)
            .get("h178-real-compute")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    let actions = RunStore::new(&db)
        .events_after("h178-real-compute", None)
        .unwrap();
    assert_eq!(
        actions
            .iter()
            .filter(|event| event.event_type == "tool_started")
            .count(),
        1
    );
}

fn with_real_node(chat: ChatService) -> (tempfile::TempDir, ChatService) {
    use deepagent_app_core::runtime_service::{
        RuntimeBroker, RuntimeService, UnavailableDownloader,
    };
    use deepagent_app_core::sandbox_backend::{
        DirectSandboxBackend, SandboxBackendKind, SandboxBackendRegistry,
    };

    let lookup = std::path::PathBuf::from(
        std::env::var_os("H178_MANAGED_SDK_FIXTURE_ROOT").expect("explicit SDK fixture required"),
    );
    let active = tempfile::tempdir().unwrap();
    let broker = Arc::new(RuntimeBroker::new(Arc::new(
        RuntimeService::with_lookup_roots(
            &[active.path().to_path_buf()],
            &[lookup],
            Arc::new(UnavailableDownloader),
        ),
    )));
    let chat = chat
        .with_runtime_broker(broker)
        .with_sandbox_backend_registry(Arc::new(SandboxBackendRegistry::new(
            SandboxBackendKind::Direct,
            [Arc::new(DirectSandboxBackend::new())
                as Arc<
                    dyn deepagent_app_core::sandbox_backend::SandboxBackend,
                >],
        )));
    (active, chat)
}

fn numeric_workflow(script: &str) -> WorkflowRequest {
    let mut request = request("javascript", script);
    request.definition.nodes[1].config.insert(
        "codeInputVariables".into(),
        json!([{"name":"input","value":6}]),
    );
    request.definition.nodes[1].config.insert(
        "codeOutputVariables".into(),
        json!([{"name":"result","type":"number"}]),
    );
    request
}
