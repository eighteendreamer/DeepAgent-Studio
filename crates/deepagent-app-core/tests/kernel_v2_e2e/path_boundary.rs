//! A workspace path must be checked after Windows junction resolution.

use super::*;
use deepagent_app_core::SandboxMode;
use deepagent_models::ModelRole;
use std::process::Command;

const OUTSIDE_CANARY: &str = "AUDIT_JUNCTION_OUTSIDE_6c1e";

struct JunctionGuard(std::path::PathBuf);

impl Drop for JunctionGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

struct JunctionModel {
    requested_path: String,
    calls: AtomicUsize,
    observed: Mutex<Option<bool>>,
}

#[async_trait]
impl HttpTransport for JunctionModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let output = |call_id: &str| {
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
                .expect("paired tool output must be present")["output"]
                .to_string()
        };
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => response_function_call(
                "audit-junction-read",
                "read_file",
                serde_json::json!({"path": self.requested_path}),
            ),
            1 => {
                let result = output("audit-junction-read");
                *self.observed.lock().unwrap() = Some(result.contains(OUTSIDE_CANARY));
                response_text("Offline junction audit completed.")
            }
            _ => panic!("unexpected extra model request in junction audit"),
        };
        emit_responses_events(sink, &events)
    }
}

#[derive(Debug)]
struct ReadObservation {
    content_returned: bool,
    tool_completed_ok: Option<bool>,
    tool_blocked: bool,
}

async fn observe_read(
    workspace: &std::path::Path,
    requested_path: String,
    sandbox_mode: SandboxMode,
    run_id: &str,
) -> ReadObservation {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    settings.set_sandbox_mode(sandbox_mode).unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    let model = Arc::new(JunctionModel {
        requested_path,
        calls: AtomicUsize::new(0),
        observed: Mutex::new(None),
    });
    let chat = ChatService::new(db, settings, model.clone(), workspace);
    chat.run_in_session(
        "Read the specified fixture file and report its contents.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(run_id),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    let events = chat.run_events(run_id, None).unwrap();
    let completed: Vec<_> = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_completed" && event.data["call_id"] == "audit-junction-read"
        })
        .collect();
    let blocked = events
        .iter()
        .any(|event| event.event_type == "tool_blocked" && event.data["name"] == "read_file");
    assert!(completed.len() <= 1);
    let content_returned = model.observed.lock().unwrap().unwrap();
    let observation = ReadObservation {
        content_returned,
        tool_completed_ok: completed
            .first()
            .map(|event| event.data["ok"].as_bool().unwrap()),
        tool_blocked: blocked,
    };
    observation
}

#[tokio::test]
async fn direct_outside_read_obeys_workspace_and_full_access_policies() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.txt");
    std::fs::write(&outside_file, OUTSIDE_CANARY).unwrap();

    let confined = observe_read(
        workspace.path(),
        outside_file.to_string_lossy().into_owned(),
        SandboxMode::WorkspaceWrite,
        "audit-direct-outside-workspace",
    )
    .await;
    assert!(!confined.content_returned);
    assert_eq!(confined.tool_completed_ok, None);
    assert!(confined.tool_blocked);

    let full = observe_read(
        workspace.path(),
        outside_file.to_string_lossy().into_owned(),
        SandboxMode::FullAccess,
        "audit-direct-outside-full",
    )
    .await;
    assert!(full.content_returned);
    assert_eq!(full.tool_completed_ok, Some(true));
    assert!(!full.tool_blocked);
}

#[tokio::test]
#[ignore = "audit: workspace file reads must not follow a junction outside the configured root"]
async fn audit_read_file_does_not_follow_workspace_junction_outside_root() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), OUTSIDE_CANARY).unwrap();
    let link = workspace.path().join("linked");
    let status = Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &outside.path().to_string_lossy(),
        ])
        .status()
        .expect("Windows cmd must be available for junction fixture");
    assert!(status.success(), "mklink /J failed: {status}");
    let junction = JunctionGuard(link);
    let observation = observe_read(
        workspace.path(),
        "linked/outside.txt".into(),
        SandboxMode::WorkspaceWrite,
        "audit-junction-boundary",
    )
    .await;
    drop(junction);
    eprintln!("workspace junction observation: {observation:?}");
    assert!(!observation.content_returned);
    assert_eq!(observation.tool_completed_ok, Some(false));
}
