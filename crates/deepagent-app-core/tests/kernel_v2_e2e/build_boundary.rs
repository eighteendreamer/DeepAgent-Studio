//! Audit the configured command boundary through the full ChatService path.

use super::*;
use deepagent_app_core::{
    ApprovalPolicy, SandboxBackend, SandboxBackendCommandExecutor, SandboxBackendKind,
    SandboxCapabilities, SandboxExecutionRequest, SandboxExecutionResult, SandboxMode,
};

const REJECTED: &str = "AUDIT_CONFIGURED_BACKEND_REJECTED_BUILD";

struct RejectingBackend(Arc<Mutex<Vec<String>>>);

#[async_trait]
impl SandboxBackend for RejectingBackend {
    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            kind: SandboxBackendKind::Direct,
            available: true,
            supports_one_shot: true,
            supports_interactive_pty: false,
            supports_network_toggle: false,
            supports_readonly_mapping: false,
            message: "audit backend: records and refuses every command; spawns nothing".into(),
        }
    }

    async fn execute(&self, request: SandboxExecutionRequest) -> Result<SandboxExecutionResult> {
        assert_eq!(request.sandbox_mode, SandboxMode::WorkspaceWrite);
        self.0.lock().unwrap().push(request.command);
        Err(CoreError::other(REJECTED))
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

async fn run_with_rejecting_backend(mutation: Mutation) -> (bool, bool) {
    let mutate = !matches!(mutation, Mutation::None);
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
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
    settings
        .initialize("offline-audit-placeholder")
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
    let executor = SandboxBackendCommandExecutor::new(
        Arc::new(RejectingBackend(backend_calls.clone())),
        &root,
        SandboxMode::WorkspaceWrite,
    );
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
    turns.push(response_text("Finished the requested turn."));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let canary_at_request = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CaptureReplay {
        inner: ReplayTransport::new(turns),
        requests: requests.clone(),
        canary: canary.clone(),
        canary_at_request: canary_at_request.clone(),
    });
    let chat = ChatService::new(db.clone(), settings, model, &root)
        .with_local_command_executor(Arc::new(executor));
    let run_id = "audit-auto-build-boundary";
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        chat.run_in_session(
            "Try the build, then finish the turn.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run_id),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("finite real build must finish");
    assert!(result.is_ok(), "{result:?}");
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
    assert_eq!(
        backend_calls.lock().unwrap().as_slice(),
        ["cargo check --offline --quiet"],
        "explicit bash must use the configured rejecting backend exactly once",
    );
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), if mutate { 3 } else { 2 });
    assert!(
        requests[1].contains(REJECTED),
        "the next actual wire request must carry the explicit command's rejection",
    );
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
        "build-boundary mutation={mutation:?} backend_calls={} explicit_rejection_in_wire=true \
         workspace_edit={mutate} outside_workspace_canary={wrote_outside} \
         canary_before_final_request={visible_before_final} run_succeeded=true",
        backend_calls.lock().unwrap().len(),
    );
    (wrote_outside, visible_before_final)
}

#[tokio::test]
async fn explicit_build_rejection_without_mutation_does_not_execute_host_build() {
    assert_eq!(
        run_with_rejecting_backend(Mutation::None).await,
        (false, false)
    );
}

#[tokio::test]
#[ignore = "audit probe: automatic verification must use the configured command boundary"]
async fn audit_auto_build_respects_configured_rejecting_backend() {
    assert_eq!(
        run_with_rejecting_backend(Mutation::None).await,
        (false, false)
    );
    let text = run_with_rejecting_backend(Mutation::Text).await;
    let rust = run_with_rejecting_backend(Mutation::Rust).await;
    assert_eq!(
        [text, rust],
        [(false, false), (false, false)],
        "automatic verification bypassed the rejecting backend and wrote outside the workspace; \
         a write before the final request identifies the post-edit decorator",
    );
}
