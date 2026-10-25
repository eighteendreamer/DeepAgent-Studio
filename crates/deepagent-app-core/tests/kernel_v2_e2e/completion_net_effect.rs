//! A successful temporary create/delete sequence is not an unmet final-state goal.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_core::event::EventPayload;
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};

const SCRATCH: &str = "audit-scratch.txt";
const CONTENT: &str = "temporary audit bytes\n";
const RUN: &str = "audit-completion-net-effect";

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Create,
    Delete,
    CreateThenDelete,
    ReadThenSameWrite,
}

struct NetEffectModel {
    scenario: Scenario,
    workspace: PathBuf,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for NetEffectModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["model"], "deepseek-flash");
        let index = {
            let mut requests = self.requests.lock().unwrap();
            let index = requests.len();
            assert!(index < 6, "bounded fixture: unexpected continuation loop");
            requests.push(body);
            index
        };
        let item = match (self.scenario, index) {
            (Scenario::Create | Scenario::CreateThenDelete, 0)
            | (Scenario::ReadThenSameWrite, 1) => call(
                "audit-create",
                "write_file",
                serde_json::json!({"path":SCRATCH,"content":CONTENT}),
            ),
            (Scenario::ReadThenSameWrite, 0) => call(
                "audit-read",
                "read_file",
                serde_json::json!({"path":SCRATCH}),
            ),
            (Scenario::Delete, 0) | (Scenario::CreateThenDelete, 1) => {
                assert_eq!(
                    std::fs::read_to_string(self.workspace.join(SCRATCH)).unwrap(),
                    CONTENT
                );
                call(
                    "audit-delete",
                    "delete_path",
                    serde_json::json!({"path":SCRATCH}),
                )
            }
            _ => text(
                &format!("audit-final-{index}"),
                "The requested audit operation is complete.",
            ),
        };
        let events = completed(
            &format!("audit-response-{index}"),
            "deepseek-flash",
            item,
            serde_json::json!({"input_tokens":20,"output_tokens":5,"total_tokens":25}),
        );
        emit_responses_events(sink, &events)
    }
}

struct Observation {
    requests: usize,
    blocked_inputs: usize,
    evidence_failures: usize,
    failed_open: bool,
}

async fn observe(scenario: Scenario) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("keep.txt"), "untouched").unwrap();
    if matches!(scenario, Scenario::Delete | Scenario::ReadThenSameWrite) {
        std::fs::write(workspace.join(SCRATCH), CONTENT).unwrap();
    }
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
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
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let model = Arc::new(NetEffectModel {
        scenario,
        workspace: workspace.clone(),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let prompt = match scenario {
        Scenario::Create => "Create audit-scratch.txt with the requested audit content, then finish.",
        Scenario::Delete => "Delete only audit-scratch.txt, then finish.",
        Scenario::CreateThenDelete => "Create audit-scratch.txt as a temporary probe, then delete that same file. The final state must have no audit-scratch.txt. Leave keep.txt unchanged and finish.",
        Scenario::ReadThenSameWrite => "Read audit-scratch.txt, ensure its exact content is the requested audit content by writing it back, then finish. Leave keep.txt unchanged.",
    };
    let session = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        chat.run_in_session(
            prompt,
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("bounded audit run")
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(workspace.join("keep.txt")).unwrap(),
        "untouched"
    );
    if matches!(scenario, Scenario::Create | Scenario::ReadThenSameWrite) {
        assert_eq!(
            std::fs::read_to_string(workspace.join(SCRATCH)).unwrap(),
            CONTENT
        );
    } else {
        assert!(!workspace.join(SCRATCH).exists());
    }
    let actions = RunControlStore::new(&db).list_actions(RUN).unwrap();
    let expected_actions = if matches!(
        scenario,
        Scenario::CreateThenDelete | Scenario::ReadThenSameWrite
    ) {
        2
    } else {
        1
    };
    assert_eq!(actions.len(), expected_actions);
    assert!(actions
        .iter()
        .all(|action| action.state == RunActionState::Completed));
    let events = chat.run_events(RUN, None).unwrap();
    let tools: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed")
        .collect();
    assert_eq!(tools.len(), expected_actions);
    assert!(tools.iter().all(|event| event.data["ok"] == true));
    let terminal: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "succeeded");
    let evidence_failures = events
        .iter()
        .filter(|event| {
            event.event_type == "verification"
                && event.data["passed"] == false
                && event.data["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("required filesystem effect"))
        })
        .count();
    let evidence: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "completion_evidence")
        .map(|event| event.data["mutations"].clone())
        .collect();
    let history = EventStore::new(&db)
        .load_session(session.parse().unwrap())
        .unwrap();
    let failed_open = history.iter().any(|event| match &event.payload {
        EventPayload::Note { text } => text.contains("completion gate failed open"),
        _ => false,
    });
    let requests = model.requests.lock().unwrap();
    // Inspect real serialized continuation input, not only a live UI event.
    let blocked_inputs = requests
        .iter()
        .filter(|body| body["input"].to_string().contains("completion_blocked"))
        .count();
    eprintln!("net-effect audit: scenario={scenario:?} requests={} blocked_inputs={blocked_inputs} evidence_failures={evidence_failures} failed_open={failed_open} completed_actions={} terminal=succeeded mutations={evidence:?}", requests.len(), actions.len());
    Observation {
        requests: requests.len(),
        blocked_inputs,
        evidence_failures,
        failed_open,
    }
}

#[tokio::test]
async fn persistent_create_or_delete_has_no_spurious_completion_retry() {
    for scenario in [Scenario::Create, Scenario::Delete] {
        let observed = observe(scenario).await;
        assert_eq!(observed.requests, 2);
        assert_eq!(observed.blocked_inputs, 0);
        assert_eq!(observed.evidence_failures, 0);
        assert!(!observed.failed_open);
    }
}

#[tokio::test]
#[ignore = "audit: a successful temporary create/delete must not be treated as missing final effects"]
async fn audit_successful_temporary_create_delete_does_not_reopen_completion() {
    let observed = observe(Scenario::CreateThenDelete).await;
    assert_eq!(
        observed.blocked_inputs, 0,
        "completed temporary work must not be reported as incomplete"
    );
    assert_eq!(observed.evidence_failures, 0);
    assert!(!observed.failed_open);
    assert_eq!(observed.requests, 3);
}

#[tokio::test]
#[ignore = "audit: a verified same-content write must not require a different final filesystem state"]
async fn audit_successful_same_content_write_does_not_reopen_completion() {
    let observed = observe(Scenario::ReadThenSameWrite).await;
    assert_eq!(
        observed.blocked_inputs, 0,
        "writing the requested bytes must not require inventing a content change"
    );
    assert_eq!(observed.evidence_failures, 0);
    assert!(!observed.failed_open);
    assert_eq!(observed.requests, 3);
}
