//! A successful temporary create/delete sequence is not an unmet final-state goal.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_core::event::EventPayload;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use deepagent_persistence::run_store::RunStore;

const SCRATCH: &str = "audit-scratch.txt";
const CONTENT: &str = "temporary audit bytes\n";
const RUN: &str = "audit-completion-net-effect";

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Create,
    Delete,
    CreateThenDelete,
    ReadThenSameWrite,
    Modify,
    ModifyThenRestore,
    MoveTwice,
    MoveThenDelete,
    MultipleWrites,
}

impl Scenario {
    fn initial(self) -> Option<&'static str> {
        match self {
            Self::Create | Self::CreateThenDelete | Self::MultipleWrites => None,
            _ => Some(CONTENT),
        }
    }

    fn script(self) -> Vec<serde_json::Value> {
        let write = |id, content| {
            call(
                id,
                "write_file",
                serde_json::json!({"path":SCRATCH,"content":content}),
            )
        };
        let delete = |path| {
            call(
                "audit-delete",
                "delete_path",
                serde_json::json!({"path":path}),
            )
        };
        let move_to = |id, source, destination| {
            call(
                id,
                "move_path",
                serde_json::json!({"source":source,"destination":destination}),
            )
        };
        // Real overwrite tools require fresh read authority after each write.
        let read = |id| call(id, "read_file", serde_json::json!({"path":SCRATCH}));
        match self {
            Self::Create => vec![write("audit-create", CONTENT)],
            Self::Delete => vec![delete(SCRATCH)],
            Self::CreateThenDelete => vec![write("audit-create", CONTENT), delete(SCRATCH)],
            Self::ReadThenSameWrite => vec![
                call(
                    "audit-read",
                    "read_file",
                    serde_json::json!({"path":SCRATCH}),
                ),
                write("audit-write", CONTENT),
            ],
            Self::Modify => vec![
                read("read-original"),
                write("audit-modify", "modified bytes"),
            ],
            Self::ModifyThenRestore => vec![
                read("read-original"),
                write("audit-modify", "modified bytes"),
                read("read-modified"),
                write("audit-restore", CONTENT),
            ],
            Self::MoveTwice => vec![
                move_to("move-one", SCRATCH, "middle.txt"),
                move_to("move-two", "middle.txt", "final.txt"),
            ],
            Self::MoveThenDelete => vec![
                move_to("move-one", SCRATCH, "middle.txt"),
                delete("middle.txt"),
            ],
            Self::MultipleWrites => vec![
                write("write-one", "first"),
                read("read-first"),
                write("write-two", "second"),
                read("read-second"),
                write("write-three", CONTENT),
            ],
        }
    }

    fn final_files(self) -> [(&'static str, Option<&'static str>); 3] {
        let scratch = match self {
            Self::Create
            | Self::ReadThenSameWrite
            | Self::ModifyThenRestore
            | Self::MultipleWrites => Some(CONTENT),
            Self::Modify => Some("modified bytes"),
            _ => None,
        };
        [
            (SCRATCH, scratch),
            ("middle.txt", None),
            (
                "final.txt",
                matches!(self, Self::MoveTwice).then_some(CONTENT),
            ),
        ]
    }
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
            assert!(
                index < self.scenario.script().len() + 4,
                "bounded fixture: unexpected continuation loop"
            );
            requests.push(body);
            index
        };
        let script = self.scenario.script();
        if let Some(previous) = index
            .checked_sub(1)
            .and_then(|previous| script.get(previous))
        {
            let args: serde_json::Value =
                serde_json::from_str(previous["arguments"].as_str().unwrap()).unwrap();
            match previous["name"].as_str().unwrap() {
                "write_file" => assert_eq!(
                    std::fs::read_to_string(self.workspace.join(SCRATCH)).unwrap(),
                    args["content"].as_str().unwrap()
                ),
                "delete_path" => {
                    assert!(!self.workspace.join(args["path"].as_str().unwrap()).exists())
                }
                "move_path" => {
                    assert!(!self
                        .workspace
                        .join(args["source"].as_str().unwrap())
                        .exists());
                    assert_eq!(
                        std::fs::read_to_string(
                            self.workspace.join(args["destination"].as_str().unwrap())
                        )
                        .unwrap(),
                        CONTENT
                    );
                }
                _ => {}
            }
        }
        let item = script.get(index).cloned().unwrap_or_else(|| {
            text(
                &format!("audit-final-{index}"),
                "The requested audit operation is complete.",
            )
        });
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
    if let Some(initial) = scenario.initial() {
        std::fs::write(workspace.join(SCRATCH), initial).unwrap();
    }
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", "deepseek-flash")
        .unwrap();
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
        Scenario::Modify => "Change audit-scratch.txt to the requested modified bytes and finish.",
        Scenario::ModifyThenRestore => "Modify audit-scratch.txt as a temporary probe then restore its original exact bytes and finish.",
        Scenario::MoveTwice => "Move audit-scratch.txt to middle.txt then final.txt, keeping its bytes, and finish.",
        Scenario::MoveThenDelete => "Move audit-scratch.txt to middle.txt then delete middle.txt and finish.",
        Scenario::MultipleWrites => "Write first, second, then the final requested bytes to audit-scratch.txt and finish.",
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
    for (path, expected) in scenario.final_files() {
        if let Some(expected) = expected {
            assert_eq!(
                std::fs::read_to_string(workspace.join(path)).unwrap(),
                expected
            );
        } else {
            assert!(!workspace.join(path).exists());
        }
    }
    let actions = RunControlStore::new(&db).list_actions(RUN).unwrap();
    let expected_actions = scenario.script().len();
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
    let expected_kinds: &[(&str, &str)] = match scenario {
        Scenario::Create | Scenario::MultipleWrites => &[(SCRATCH, "created")],
        Scenario::Delete => &[(SCRATCH, "deleted")],
        Scenario::Modify => &[(SCRATCH, "modified")],
        Scenario::CreateThenDelete | Scenario::ReadThenSameWrite | Scenario::ModifyThenRestore => {
            &[(SCRATCH, "unchanged")]
        }
        Scenario::MoveTwice => &[
            (SCRATCH, "deleted"),
            ("middle.txt", "unchanged"),
            ("final.txt", "created"),
        ],
        Scenario::MoveThenDelete => &[(SCRATCH, "deleted"), ("middle.txt", "unchanged")],
    };
    assert!(
        !evidence.is_empty(),
        "real net evidence must still be published"
    );
    for mutations in &evidence {
        let actual: std::collections::BTreeMap<_, _> = mutations
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    PathBuf::from(item["path"].as_str().unwrap())
                        .file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                    item["kind"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        let expected = expected_kinds
            .iter()
            .map(|(path, kind)| (path.to_string(), kind.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "{scenario:?}: never invent a retained change"
        );
    }
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
    let observation = Observation {
        requests: requests.len(),
        blocked_inputs,
        evidence_failures,
        failed_open,
    };
    drop(requests);
    drop(chat);
    drop(db);
    let reopened = Database::open(temporary.path().join("state.sqlite")).unwrap();
    assert_eq!(
        RunStore::new(&reopened).events_after(RUN, None).unwrap(),
        events
    );
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(session.parse().unwrap())
            .unwrap(),
        history
    );
    let reopened_actions = RunControlStore::new(&reopened).list_actions(RUN).unwrap();
    assert_eq!(reopened_actions.len(), expected_actions);
    assert!(reopened_actions
        .iter()
        .all(|action| action.state == RunActionState::Completed));
    assert_eq!(
        model.requests.lock().unwrap().len(),
        observation.requests,
        "reopen must not redispatch"
    );
    observation
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

#[tokio::test]
async fn intermediate_changes_and_moves_do_not_create_unrequested_final_contracts() {
    for scenario in [
        Scenario::Modify,
        Scenario::ModifyThenRestore,
        Scenario::MoveTwice,
        Scenario::MoveThenDelete,
        Scenario::MultipleWrites,
    ] {
        let observed = observe(scenario).await;
        assert_eq!(observed.blocked_inputs, 0, "{scenario:?}");
        assert_eq!(observed.evidence_failures, 0);
        assert!(!observed.failed_open);
        assert_eq!(observed.requests, scenario.script().len() + 1);
    }
}
