//! A prepared stream call must not replace a contradictory final call silently.

use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::response_item::ResponseItem;
use deepagent_models::ModelRole;
use deepagent_persistence::event_store::EventStore;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use sha2::{Digest, Sha256};

const CALL: &str = "audit-identity-call";
const RUN: &str = "audit-identity-run";

struct IdentityModel {
    early: String,
    final_args: String,
    workspace: PathBuf,
    requests: Mutex<Vec<serde_json::Value>>,
}

impl IdentityModel {
    fn call_events(&self) -> Vec<String> {
        let item = serde_json::json!({
            "type":"function_call", "id":"audit-identity-item", "call_id":CALL,
            "name":"write_file", "arguments":self.final_args, "status":"completed"
        });
        let events = vec![
            serde_json::json!({"type":"response.created","response":{
                "id":"audit-identity-response","status":"in_progress","model":"deepseek-flash"
            }}),
            serde_json::json!({"type":"response.output_item.added","output_index":0,
                "item":{"type":"function_call","id":"audit-identity-item","call_id":CALL,
                    "name":"write_file","arguments":"","status":"in_progress"}}),
            serde_json::json!({"type":"response.function_call_arguments.delta",
                "item_id":"audit-identity-item","output_index":0,"delta":self.early}),
            serde_json::json!({"type":"response.function_call_arguments.done",
                "item_id":"audit-identity-item","output_index":0,"arguments":self.early}),
            serde_json::json!({"type":"response.output_item.done","output_index":0,"item":item}),
            serde_json::json!({"type":"response.completed","response":{
                "id":"audit-identity-response","status":"completed","model":"deepseek-flash",
                "output":[item],"usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}
            }}),
        ];
        events
            .into_iter()
            .enumerate()
            .map(|(sequence, mut event)| {
                event["sequence_number"] = sequence.into();
                event.to_string()
            })
            .collect()
    }
}

#[async_trait]
impl HttpTransport for IdentityModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["model"], "deepseek-flash");
        assert_eq!(body["stream"], true);
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body);
            requests.len() == 1
        };
        if first {
            let events = self.call_events();
            // Neither event completion is allowed to perform this write early.
            emit_responses_events(sink, &events[..events.len() - 1])?;
            assert!(!self.workspace.join("early.txt").exists());
            assert!(!self.workspace.join("final.txt").exists());
            emit_responses_events(sink, &events[events.len() - 1..])
        } else {
            emit_responses_events(sink, &response_text("Audit write finished."))
        }
    }
}

struct Observation {
    executed: serde_json::Value,
    final_args: serde_json::Value,
    early_exists: bool,
}

async fn observe(early: String, final_args: String) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
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
    let model = Arc::new(IdentityModel {
        early,
        final_args: final_args.clone(),
        workspace: workspace.clone(),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let session_id = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        chat.run_in_session(
            "Create the requested audit text file.",
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
    let events = chat.run_events(RUN, None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let started: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "tool_started" && e.data["call_id"] == CALL)
        .collect();
    let completed: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "tool_completed" && e.data["call_id"] == CALL)
        .collect();
    assert_eq!(started.len(), 1);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].data["ok"], true);
    let executed = started[0].data["arguments"].clone();
    assert_eq!(
        std::fs::read_to_string(workspace.join(executed["path"].as_str().unwrap())).unwrap(),
        executed["content"].as_str().unwrap()
    );

    let history = EventStore::new(&db)
        .load_session(session_id.parse().unwrap())
        .unwrap();
    let calls: Vec<_> = history
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ResponseItemAppended {
                item:
                    ResponseItem::FunctionCall {
                        call_id,
                        name,
                        arguments,
                    },
            } if call_id == CALL => {
                assert_eq!(name, "write_file");
                Some(serde_json::from_str::<serde_json::Value>(arguments).unwrap())
            }
            _ => None,
        })
        .collect();
    let expected_final: serde_json::Value = serde_json::from_str(&final_args).unwrap();
    assert_eq!(calls, vec![expected_final.clone()]);
    let actions = RunControlStore::new(&db).list_actions(RUN).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].call_id, CALL);
    assert_eq!(actions[0].state, RunActionState::Completed);
    assert_eq!(
        actions[0].arguments_hash,
        format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&executed).unwrap())
        )
    );

    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let input = requests[1]["input"].as_array().unwrap();
    let sent_calls: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call" && item["call_id"] == CALL)
        .collect();
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
        .collect();
    assert_eq!(sent_calls.len(), 1);
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(sent_calls[0]["arguments"].as_str().unwrap())
            .unwrap(),
        expected_final
    );
    let output: serde_json::Value =
        serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
    assert_eq!(output["status"], "ok");
    assert_eq!(output["result"], completed[0].data["output"]);
    let early_exists = workspace.join("early.txt").exists();
    eprintln!("identity audit: executed={executed} native={expected_final} early_exists={early_exists} final_exists={} action={:?} wire_calls=1 wire_outputs=1 terminal=succeeded", workspace.join("final.txt").exists(), actions[0].state);
    Observation {
        executed,
        final_args: expected_final,
        early_exists,
    }
}

fn final_arguments() -> String {
    serde_json::json!({"path":"final.txt","content":"FINAL"}).to_string()
}

#[tokio::test]
async fn matching_tool_completions_keep_execution_and_history_consistent() {
    let final_args = final_arguments();
    let observed = observe(final_args.clone(), final_args).await;
    assert_eq!(observed.executed, observed.final_args);
    assert!(!observed.early_exists);
}

#[tokio::test]
async fn equivalent_json_tool_completions_keep_execution_and_history_consistent() {
    let observed = observe(
        "{ \"path\" : \"final.txt\", \"content\" : \"FINAL\" }".into(),
        final_arguments(),
    )
    .await;
    assert_eq!(observed.executed, observed.final_args);
    assert!(!observed.early_exists);
}

#[tokio::test]
#[ignore = "audit: contradictory completed arguments must not silently execute a cached old call"]
async fn audit_conflicting_tool_completions_never_commit_a_different_call() {
    let observed = observe(
        serde_json::json!({"path":"early.txt","content":"EARLY"}).to_string(),
        final_arguments(),
    )
    .await;
    assert_eq!(observed.executed, observed.final_args, "successful execution and native history must describe the same call; rejecting the conflict before any write is also a valid production fix");
    assert!(!observed.early_exists);
}
