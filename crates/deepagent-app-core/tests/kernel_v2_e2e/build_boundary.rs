//! Audit the configured command boundary through the full ChatService path.

use super::*;
use deepagent_app_core::{
    ApprovalPolicy, DirectSandboxBackend, HarnessRunOverrides, RuntimeBroker, RuntimeService,
    SandboxBackend, SandboxBackendCommandExecutor, SandboxBackendKind, SandboxBackendRegistry,
    SandboxCapabilities, SandboxExecutionRequest, SandboxExecutionResult, SandboxMode,
    SandboxNetworkPolicy,
};
use deepagent_persistence::run_control::RunControlStore;

const REJECTED: &str = "AUDIT_CONFIGURED_BACKEND_REJECTED_BUILD";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendMode {
    Reject,
    Unavailable,
    Allow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Local,
    Broker,
    Remote,
    Override,
}

struct TestBackend {
    calls: Arc<Mutex<Vec<String>>>,
    mode: BackendMode,
    kind: SandboxBackendKind,
    expect_environment: bool,
}

#[async_trait]
impl SandboxBackend for TestBackend {
    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            kind: self.kind,
            available: self.mode != BackendMode::Unavailable,
            supports_one_shot: true,
            supports_interactive_pty: false,
            supports_network_toggle: false,
            supports_readonly_mapping: false,
            message: format!(
                "fixture backend {:?}; only Allow executes finite local builds",
                self.mode
            ),
        }
    }

    async fn execute(&self, request: SandboxExecutionRequest) -> Result<SandboxExecutionResult> {
        assert_eq!(request.sandbox_mode, SandboxMode::WorkspaceWrite);
        assert_eq!(request.network, SandboxNetworkPolicy::Disabled);
        if self.expect_environment {
            assert!(request
                .environment
                .iter()
                .any(|(key, value)| key == "DEEPAGENT_PROJECT_ROOT"
                    && value == &request.workspace_root.to_string_lossy()));
            assert!(request
                .environment
                .iter()
                .any(|(key, _)| key == "DEEPAGENT_RUNTIME_ROOT"));
        }
        self.calls.lock().unwrap().push(request.command.clone());
        if self.mode == BackendMode::Allow {
            DirectSandboxBackend::new().execute(request).await
        } else {
            Err(CoreError::other(REJECTED))
        }
    }
}

struct CaptureReplay {
    inner: ReplayTransport,
    requests: Arc<Mutex<Vec<String>>>,
    canary: PathBuf,
    canary_at_request: Arc<Mutex<Vec<bool>>>,
}

#[async_trait]
impl HttpTransport for CaptureReplay {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests.lock().unwrap().push(request.body.clone());
        self.canary_at_request
            .lock()
            .unwrap()
            .push(self.canary.exists());
        self.inner.stream(request, sink).await
    }
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    Text,
    Rust,
}

async fn run_with_backend(mutation: Mutation, mode: BackendMode) -> (bool, bool) {
    run_with_route(mutation, mode, Route::Local).await
}

async fn run_with_route(mutation: Mutation, mode: BackendMode, route: Route) -> (bool, bool) {
    let mutate = !matches!(mutation, Mutation::None);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace 空格 $literal 'quote");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'harness-build-boundary-audit'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
    // Only writes to a sibling inside this TempDir. No user files, network,
    // privileged operation, or real sandbox configuration is involved.
    std::fs::write(
        root.join("build.rs"),
        r#"fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    std::fs::write("../outside-workspace-canary", "automatic host build executed").unwrap();
}
"#,
    )
    .unwrap();
    let canary = temp.path().join("outside-workspace-canary");
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    // Command approval and the executor's filesystem boundary are distinct.
    // Automatic approval must not bypass a rejecting configured backend.
    settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    settings
        .set_sandbox_mode(SandboxMode::WorkspaceWrite)
        .unwrap();

    let backend_calls = Arc::new(Mutex::new(Vec::new()));
    let backend = Arc::new(TestBackend {
        calls: backend_calls.clone(),
        mode,
        kind: if route == Route::Override {
            SandboxBackendKind::WindowsSandbox
        } else {
            SandboxBackendKind::Direct
        },
        expect_environment: route == Route::Broker,
    });
    let executor = Arc::new(SandboxBackendCommandExecutor::new(
        backend.clone(),
        &root,
        SandboxMode::WorkspaceWrite,
    ));
    let mut turns = vec![response_function_call(
        "explicit-build",
        "bash",
        serde_json::json!({"command":"cargo check --offline --quiet"}),
    )];
    if mutate {
        // The .txt case isolates completion verification. The .rs case also
        // checks the independent post-edit decorator before the final model
        // request, using the same backend and real write_file tool.
        let (path, content) = match mutation {
            Mutation::Text => ("note.txt", "audited fixture edit"),
            Mutation::Rust => ("src/added.rs", "pub fn changed_fixture() {}\n"),
            Mutation::None => unreachable!(),
        };
        turns.push(response_function_call(
            "write-note",
            "write_file",
            serde_json::json!({"path":path,"content":content}),
        ));
    }
    // Rejected completion checks use the existing reflection budget. Supply
    // final answers for those retries; do not hide rejection by starving replay.
    turns.extend((0..8).map(|_| response_text("Finished the requested turn.")));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let canary_at_request = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CaptureReplay {
        inner: ReplayTransport::new(turns),
        requests: requests.clone(),
        canary: canary.clone(),
        canary_at_request: canary_at_request.clone(),
    });
    let mut chat = ChatService::new(db.clone(), settings, model, &root)
        .with_local_command_executor(executor.clone());
    let unselected_calls = Arc::new(Mutex::new(Vec::new()));
    if route == Route::Remote {
        chat = chat
            .with_local_command_executor(Arc::new(SandboxBackendCommandExecutor::new(
                Arc::new(TestBackend {
                    calls: unselected_calls.clone(),
                    mode: BackendMode::Reject,
                    kind: SandboxBackendKind::Direct,
                    expect_environment: false,
                }),
                &root,
                SandboxMode::WorkspaceWrite,
            )))
            .with_executor_factory(move |id| {
                assert_eq!(id, "fixture-remote");
                executor.clone()
            });
    } else if route == Route::Override {
        chat = chat.with_sandbox_backend_registry(Arc::new(SandboxBackendRegistry::new(
            SandboxBackendKind::Direct,
            vec![
                Arc::new(TestBackend {
                    calls: unselected_calls.clone(),
                    mode: BackendMode::Reject,
                    kind: SandboxBackendKind::Direct,
                    expect_environment: false,
                }) as Arc<dyn SandboxBackend>,
                backend,
            ],
        )));
    } else if route == Route::Broker {
        chat =
            chat.with_runtime_broker(Arc::new(RuntimeBroker::new(Arc::new(RuntimeService::new(
                &[temp.path().join("runtimes")],
                Arc::new(deepagent_app_core::runtime_service::UnavailableDownloader),
            )))));
    }
    let run_id = "audit-auto-build-boundary";
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        chat.run_in_session_with_overrides(
            "Try the build, then finish the turn.",
            None,
            (route == Route::Remote).then_some("remote"),
            (route == Route::Remote).then_some("fixture-remote"),
            Vec::new(),
            None,
            false,
            Some(run_id),
            HarnessRunOverrides {
                sandbox_backend: (route == Route::Override).then(|| "windows_sandbox".into()),
                ..Default::default()
            },
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("finite real build must finish");
    assert!(result.is_ok(), "{result:?}");
    assert!(
        unselected_calls.lock().unwrap().is_empty(),
        "automatic checks must not replace the selected executor with the local/default backend"
    );
    let events = chat.run_events(run_id, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1,
    );
    assert!(events
        .iter()
        .any(|e| e.event_type == "run_terminal" && e.data["kind"] == "succeeded"));
    let calls = backend_calls.lock().unwrap().clone();
    assert_eq!(calls[0], "cargo check --offline --quiet");
    let post_edit = calls
        .iter()
        .filter(|c| c.contains("--manifest-path"))
        .count();
    assert_eq!(post_edit, usize::from(matches!(mutation, Mutation::Rust)));
    if mutate {
        assert!(
            calls.len() >= 2 + post_edit,
            "both automatic paths must reach the configured backend: {calls:?}"
        );
        assert!(events
            .iter()
            .any(|event| event.event_type == "verification"));
    } else {
        assert_eq!(
            calls.len(),
            1,
            "without mutation no automatic build may run"
        );
    }
    assert!(calls.iter().all(|c| c.starts_with("cargo check --offline")));
    let actions = RunControlStore::new(&db).list_actions(run_id).unwrap();
    assert_eq!(
        actions.iter().filter(|a| a.tool_name == "bash").count(),
        calls.len()
    );
    assert_eq!(actions.len(), calls.len() + usize::from(mutate));
    for pair in actions.windows(2) {
        assert_eq!(pair[1].sequence, pair[0].sequence + 1);
    }
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "tool_completed" && e.data["name"] == "bash")
            .count(),
        calls.len()
    );
    let requests = requests.lock().unwrap();
    assert!(requests.len() >= if mutate { 3 } else { 2 });
    if mode != BackendMode::Allow {
        assert!(
            requests[1].contains(REJECTED),
            "explicit rejection must reach the actual next wire"
        );
        if matches!(mutation, Mutation::Rust) {
            assert!(
                requests[2].contains(REJECTED),
                "post-edit rejection must reach the actual next wire"
            );
        }
        if mutate {
            assert!(
                requests.last().unwrap().contains("verification_failed"),
                "completion rejection must be fed back, not silently passed"
            );
        }
    }
    assert_eq!(
        root.join("note.txt").exists(),
        matches!(mutation, Mutation::Text)
    );
    if matches!(mutation, Mutation::Rust) {
        assert_eq!(
            std::fs::read_to_string(root.join("src/added.rs")).unwrap(),
            "pub fn changed_fixture() {}\n",
        );
    }
    let wrote_outside = canary.exists();
    let visible_before_final = *canary_at_request.lock().unwrap().last().unwrap();
    if wrote_outside {
        assert_eq!(
            std::fs::read_to_string(&canary).unwrap(),
            "automatic host build executed",
        );
    }
    eprintln!(
        "build-boundary mutation={mutation:?} mode={mode:?} route={route:?} backend_calls={} post_edit_calls={post_edit} \
         workspace_edit={mutate} outside_workspace_canary={wrote_outside} \
         canary_before_final_request={visible_before_final} run_succeeded=true",
        backend_calls.lock().unwrap().len(),
    );
    (wrote_outside, visible_before_final)
}

#[tokio::test]
async fn explicit_build_rejection_without_mutation_does_not_execute_host_build() {
    assert_eq!(
        run_with_backend(Mutation::None, BackendMode::Reject).await,
        (false, false)
    );
}

#[tokio::test]
async fn audit_auto_build_respects_configured_rejecting_backend() {
    assert_eq!(
        run_with_backend(Mutation::None, BackendMode::Reject).await,
        (false, false)
    );
    let text = run_with_backend(Mutation::Text, BackendMode::Reject).await;
    let rust = run_with_backend(Mutation::Rust, BackendMode::Reject).await;
    assert_eq!(
        [text, rust],
        [(false, false), (false, false)],
        "automatic verification bypassed the rejecting backend and wrote outside the workspace; \
         a write before the final request identifies the post-edit decorator",
    );
}

#[tokio::test]
async fn automatic_checks_do_not_fall_back_when_backend_is_unavailable() {
    for mutation in [Mutation::Text, Mutation::Rust] {
        assert_eq!(
            run_with_backend(mutation, BackendMode::Unavailable).await,
            (false, false)
        );
    }
}

#[tokio::test]
async fn allowed_backend_still_executes_real_builds_and_post_edit_checks() {
    // Explicit build is the positive control and writes the controlled canary.
    // Counts/actions above independently require the post-edit and final checks.
    for mutation in [Mutation::Text, Mutation::Rust] {
        assert_eq!(
            run_with_backend(mutation, BackendMode::Allow).await,
            (true, true)
        );
    }
}

#[tokio::test]
async fn automatic_checks_keep_runtime_environment_remote_route_and_run_override() {
    for route in [Route::Broker, Route::Remote, Route::Override] {
        for mutation in [Mutation::Text, Mutation::Rust] {
            assert_eq!(
                run_with_route(mutation, BackendMode::Reject, route).await,
                (false, false)
            );
        }
    }
}
