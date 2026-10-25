//! A failed native move must not recursively delete its own source and siblings.

use super::*;
use deepagent_models::ModelRole;
use deepagent_persistence::checkpoint_store::CheckpointStore;
use deepagent_runtime::CheckpointManager;
use serde_json::{json, Value};

const RUN_ID: &str = "audit-move-failure";
const CALL_ID: &str = "audit-controlled-move";
const SOURCE: &str = "synthetic-source-canary\n";
const SIBLING: &str = "synthetic-unrelated-sibling-canary\n";
const TARGET: &str = "synthetic-original-target-canary\n";

struct MoveModel {
    destination: &'static str,
    overwrite: bool,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for MoveModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body.clone());
            requests.len()
        };
        let item = match attempt {
            1 => {
                assert!(body["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| { tool["type"] == "function" && tool["name"] == "move_path" }));
                super::responses_fixture::call(
                    CALL_ID,
                    "move_path",
                    json!({"source":"payload/source.txt", "destination":self.destination,
                        "overwrite":self.overwrite}),
                )
            }
            2 => {
                let outputs = body["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|item| {
                        item["type"] == "function_call_output" && item["call_id"] == CALL_ID
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    outputs.len(),
                    1,
                    "the real tool result must reach the model"
                );
                super::responses_fixture::text("audit-move-answer", "Controlled audit finished.")
            }
            _ => panic!("unexpected extra request in controlled move fixture"),
        };
        let mut events = super::responses_fixture::completed(
            &format!("audit-move-response-{attempt}"),
            "deepseek-flash",
            item.clone(),
            json!({"input_tokens":10,"output_tokens":4,"total_tokens":14}),
        );
        if item["type"] == "message" {
            // Match the done item with its delta; avoid H-81's done-only fallback.
            events.insert(
                2,
                json!({"type":"response.output_text.delta","item_id":item["id"],
                    "output_index":0,"content_index":0,"delta":item["content"][0]["text"]})
                .to_string(),
            );
        }
        for (sequence, event) in events.iter_mut().enumerate() {
            let mut value: Value = serde_json::from_str(event).unwrap();
            value["sequence_number"] = sequence.into();
            *event = value.to_string();
        }
        emit_responses_events(sink, &events)
    }
}

struct Observation {
    tool_ok: bool,
    source: Option<String>,
    sibling: Option<String>,
    destination: Option<String>,
    wire_output: Value,
    restored: bool,
}

async fn observe(destination: &'static str, overwrite: bool) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    let payload = workspace.join("payload");
    std::fs::create_dir_all(&payload).unwrap();
    let source = payload.join("source.txt");
    let sibling = payload.join("sibling.txt");
    let target = workspace.join(destination);
    std::fs::write(&source, SOURCE).unwrap();
    std::fs::write(&sibling, SIBLING).unwrap();
    if destination == "target.txt" {
        std::fs::write(&target, TARGET).unwrap();
    }
    // The recursive fault's exact target is this disposable fixture subdirectory,
    // never the workspace/data root or an external path. No junctions or symlinks.
    assert!(source.starts_with(&payload));
    assert!(payload.starts_with(&workspace) && payload != workspace);
    assert!(workspace.starts_with(temporary.path()));
    assert!(!std::fs::symlink_metadata(&payload)
        .unwrap()
        .file_type()
        .is_symlink());

    let db_path = temporary.path().join("state.sqlite");
    let model = Arc::new(MoveModel {
        destination,
        overwrite,
        requests: Mutex::new(Vec::new()),
    });
    let (tool_ok, checkpoint_id) = {
        let db = Arc::new(Database::open(&db_path).unwrap());
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
        let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            chat.run_in_session(
                "Execute the single supplied audit tool call and report its result.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(RUN_ID),
                |_| {},
                |_| {},
            ),
        )
        .await
        .expect("a local move must finish within the fixture deadline")
        .unwrap();
        let events = chat.run_events(RUN_ID, None).unwrap();
        let tools = events
            .iter()
            .filter(|event| {
                event.event_type == "tool_completed" && event.data["call_id"] == CALL_ID
            })
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 1);
        let terminal = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
        let checkpoints = CheckpointStore::new(&db).list_for_run(RUN_ID).unwrap();
        assert_eq!(checkpoints.len(), 1);
        (
            tools[0].data["ok"].as_bool().unwrap(),
            checkpoints[0].id.clone(),
        )
    };
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let wire_output = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == CALL_ID)
        .unwrap()["output"]
        .as_str()
        .map(|text| serde_json::from_str(text).unwrap())
        .unwrap();
    let mut observation = Observation {
        tool_ok,
        source: std::fs::read_to_string(&source).ok(),
        sibling: std::fs::read_to_string(&sibling).ok(),
        destination: std::fs::read_to_string(&target).ok(),
        wire_output,
        restored: false,
    };
    // Check recoverability, not just the failure: reopen the real database and
    // restore the production checkpoint (not a hand-authored backup).
    let db = Database::open(&db_path).unwrap();
    let replayed = deepagent_persistence::run_store::RunStore::new(&db)
        .events_after(RUN_ID, None)
        .unwrap();
    assert!(replayed.iter().any(|event| {
        event.event_type == "tool_completed"
            && event.data["call_id"] == CALL_ID
            && event.data["ok"] == json!(tool_ok)
    }));
    CheckpointManager::restore(&db, &checkpoint_id).unwrap();
    observation.restored = std::fs::read_to_string(&source).unwrap() == SOURCE
        && std::fs::read_to_string(&sibling).unwrap() == SIBLING;
    if destination == "target.txt" {
        assert_eq!(std::fs::read_to_string(&target).unwrap(), TARGET);
    }
    eprintln!(
        "move-failure audit: destination={destination} overwrite={overwrite} tool_ok={} source_present={} sibling_present={} target_text={:?} restored={} requests={} wire_output={}",
        observation.tool_ok, observation.source.is_some(), observation.sibling.is_some(),
        observation.destination, observation.restored, requests.len(), observation.wire_output,
    );
    observation
}

#[tokio::test]
async fn move_controls_preserve_data_on_rejection_and_support_normal_replacement() {
    let denied = observe("target.txt", false).await;
    assert!(!denied.tool_ok && denied.restored);
    assert_eq!(denied.source.as_deref(), Some(SOURCE));
    assert_eq!(denied.sibling.as_deref(), Some(SIBLING));
    assert_eq!(denied.destination.as_deref(), Some(TARGET));

    let moved = observe("target.txt", true).await;
    assert!(moved.tool_ok && moved.restored);
    assert!(moved.source.is_none());
    assert_eq!(moved.destination.as_deref(), Some(SOURCE));
    assert_eq!(moved.sibling.as_deref(), Some(SIBLING));

    let ancestor_denied = observe("payload", false).await;
    assert!(!ancestor_denied.tool_ok && ancestor_denied.restored);
    assert_eq!(ancestor_denied.source.as_deref(), Some(SOURCE));
    assert_eq!(ancestor_denied.sibling.as_deref(), Some(SIBLING));
}

#[tokio::test]
#[ignore = "audit: move failure must not delete the source's ancestor directory"]
async fn audit_move_to_ancestor_does_not_destroy_source_and_unrelated_sibling() {
    let result = observe("payload", true).await;
    assert!(
        !result.tool_ok,
        "the file cannot replace its own parent directory"
    );
    assert!(
        result.restored,
        "production checkpoints remain a recovery path"
    );
    assert_eq!(
        result.source.as_deref(),
        Some(SOURCE),
        "failed move erased its source"
    );
    assert_eq!(
        result.sibling.as_deref(),
        Some(SIBLING),
        "failed move erased an unrelated sibling"
    );
}
