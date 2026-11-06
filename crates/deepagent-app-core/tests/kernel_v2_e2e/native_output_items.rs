//! Completed native text items must not be supplemented by their joined delta text.

use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::response_item::ResponseItem;
use deepagent_models::ModelRole;
use deepagent_runtime::events::RuntimeEvent;

struct NativeTextModel {
    parts: Vec<&'static str>,
    include_deltas: bool,
    completion: TextCompletion,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[derive(Clone, Copy)]
enum TextCompletion {
    Full,
    SnapshotOnly,
    DeltaOnly,
    FirstItemOnly,
}

fn text_item_events_with_deltas(parts: &[&str], include_deltas: bool) -> Vec<String> {
    let mut events = vec![serde_json::json!({
        "type":"response.created", "response":{
            "id":"audit-response","status":"in_progress","model":"deepseek-flash"
        }
    })
    .to_string()];
    let mut output = Vec::new();
    for (index, text) in parts.iter().enumerate() {
        let id = format!("audit-message-{index}");
        let item = serde_json::json!({
            "type":"message", "id":id, "role":"assistant", "status":"completed",
            "content":[{"type":"output_text","text":text,"annotations":[]}]
        });
        let mut item_events = vec![
            serde_json::json!({"type":"response.output_item.added", "output_index":index,
                "item":{"type":"message","id":id,"role":"assistant","status":"in_progress","content":[]}}).to_string(),
        ];
        if include_deltas {
            item_events.extend([
                serde_json::json!({"type":"response.content_part.added","item_id":id,
                    "output_index":index,"content_index":0,
                    "part":{"type":"output_text","text":"","annotations":[]}})
                .to_string(),
                serde_json::json!({"type":"response.output_text.delta","item_id":id,
                    "output_index":index,"content_index":0,"delta":text})
                .to_string(),
                serde_json::json!({"type":"response.output_text.done","item_id":id,
                    "output_index":index,"content_index":0,"text":text})
                .to_string(),
                serde_json::json!({"type":"response.content_part.done","item_id":id,
                    "output_index":index,"content_index":0,"part":item["content"][0]})
                .to_string(),
            ]);
        }
        item_events.push(
            serde_json::json!({"type":"response.output_item.done","output_index":index,"item":item}).to_string(),
        );
        events.extend(item_events);
        output.push(item);
    }
    events.push(
        serde_json::json!({"type":"response.completed","response":{"object":"response","created_at":1,
            "id":"audit-response","status":"completed","model":"deepseek-flash","output":output,
            "usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}
        }})
        .to_string(),
    );
    // All fixture events have a unique sequence, including ignored lifecycle events.
    events
        .into_iter()
        .enumerate()
        .map(|(sequence, event)| {
            let mut value: serde_json::Value = serde_json::from_str(&event).unwrap();
            value["sequence_number"] = sequence.into();
            value.to_string()
        })
        .collect()
}

#[async_trait]
impl HttpTransport for NativeTextModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len() == 1
        };
        let parts: Vec<_> = if first {
            self.parts.clone()
        } else {
            vec!["AUDIT_CONTINUED"]
        };
        let mut events = text_item_events_with_deltas(&parts, self.include_deltas);
        if first {
            events = events
                .into_iter()
                .filter_map(|event| {
                    let mut value: serde_json::Value = serde_json::from_str(&event).unwrap();
                    if value["type"] == "response.output_item.done"
                        && match self.completion {
                            TextCompletion::Full => false,
                            TextCompletion::FirstItemOnly => value["output_index"] != 0,
                            TextCompletion::SnapshotOnly | TextCompletion::DeltaOnly => true,
                        }
                    {
                        return None;
                    }
                    if value["type"] == "response.completed"
                        && matches!(
                            self.completion,
                            TextCompletion::DeltaOnly | TextCompletion::FirstItemOnly
                        )
                    {
                        // No terminal text snapshot, but the envelope still
                        // carries the required output array (H-208).
                        value["response"]["output"] = serde_json::json!([]);
                    }
                    Some(value.to_string())
                })
                .collect();
        }
        emit_responses_events(sink, &events)
    }
}

async fn audit_settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
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
}

async fn observe_native_text(
    parts: Vec<&'static str>,
    include_deltas: bool,
) -> (Vec<String>, Vec<String>, String, String) {
    observe_native_text_completion(parts, include_deltas, TextCompletion::Full).await
}

async fn observe_native_text_completion(
    parts: Vec<&'static str>,
    include_deltas: bool,
    completion: TextCompletion,
) -> (Vec<String>, Vec<String>, String, String) {
    let workspace = tempfile::tempdir().unwrap();
    let db_path = workspace.path().join("state.sqlite");
    let model = Arc::new(NativeTextModel {
        parts,
        include_deltas,
        completion,
        requests: Mutex::new(Vec::new()),
    });
    let streamed = Arc::new(Mutex::new(String::new()));
    let (session_id, persisted, final_answer) = {
        let db = Arc::new(Database::open(&db_path).unwrap());
        let settings = audit_settings(db.clone()).await;
        let chat = ChatService::new(db.clone(), settings, model.clone(), workspace.path());
        let deltas = streamed.clone();
        let session_id = chat
            .run_in_session(
                "Audit native text item retention.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("audit-native-text-first"),
                move |event| {
                    if let RuntimeEvent::ContentDelta { text } = event {
                        deltas.lock().unwrap().push_str(&text);
                    }
                },
                |_| {},
            )
            .await
            .unwrap();
        let events = deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(session_id.parse().unwrap())
            .unwrap();
        let persisted = events
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message { role, content },
                } if role == "assistant" => Some(content.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let final_answer = events
            .iter()
            .rev()
            .find_map(|event| match &event.payload {
                EventPayload::MessageAppended { message }
                    if message.role == deepagent_core::message::Role::Assistant =>
                {
                    Some(message.content.clone())
                }
                _ => None,
            })
            .unwrap();
        let terminal = chat.run_events("audit-native-text-first", None).unwrap();
        let terminal = terminal
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0].data["kind"], "succeeded");
        (session_id, persisted, final_answer)
    };
    // Reopen real disk storage after all original ChatService/Database owners drop.
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = audit_settings(db.clone()).await;
    let chat = ChatService::new(db, settings, model.clone(), workspace.path());
    chat.run_in_session(
        "Continue after reopening storage.",
        Some(&session_id),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-native-text-reopen"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let resumed_events = chat.run_events("audit-native-text-reopen", None).unwrap();
    let resumed_terminal = resumed_events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(resumed_terminal.len(), 1);
    assert_eq!(resumed_terminal[0].data["kind"], "succeeded");
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let sent = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "message" && item["role"] == "assistant")
        .map(|item| item["content"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let streamed = streamed.lock().unwrap().clone();
    eprintln!("native-text audit: provider_items={} persisted_items={} resumed_wire_items={} streamed_chars={} final_chars={} reopened=true",
        model.parts.len(), persisted.len(), sent.len(), streamed.len(), final_answer.len());
    (persisted, sent, streamed, final_answer)
}

#[tokio::test]
async fn native_text_control_single_item_retains_one_copy() {
    let parts = vec!["AUDIT_FIRST|AUDIT_SECOND"];
    let (persisted, sent, streamed, final_answer) = observe_native_text(parts.clone(), true).await;
    assert_eq!(persisted, parts);
    assert_eq!(sent, parts);
    assert_eq!(streamed, parts.concat());
    assert_eq!(final_answer, streamed);
}

#[tokio::test]
async fn audit_native_text_multiple_items_preserve_exact_provider_content() {
    let parts = vec!["AUDIT_FIRST|", "AUDIT_SECOND"];
    let (persisted, sent, streamed, final_answer) = observe_native_text(parts.clone(), true).await;
    assert_eq!(
        streamed,
        parts.concat(),
        "the provider sent exactly these deltas"
    );
    assert_eq!(
        sent, persisted,
        "recovery retains the actual stored native items"
    );
    assert_eq!(
        persisted, parts,
        "joined fallback duplicated completed native messages"
    );
    assert_eq!(
        final_answer, streamed,
        "final text must agree with streamed text"
    );
}

#[tokio::test]
async fn audit_native_text_done_only_does_not_append_empty_fallback() {
    let parts = vec!["AUDIT_DONE_ONLY"];
    let (persisted, sent, streamed, final_answer) = observe_native_text(parts.clone(), false).await;
    assert_eq!(persisted, parts);
    assert_eq!(sent, parts);
    assert!(
        streamed.is_empty(),
        "done-only responses have no live deltas"
    );
    assert_eq!(final_answer, parts[0]);
}

#[tokio::test]
async fn h81_native_text_reopened_matrix_keeps_distinct_empty_and_fallback_items() {
    for completion in [
        TextCompletion::Full,
        TextCompletion::SnapshotOnly,
        TextCompletion::DeltaOnly,
        TextCompletion::FirstItemOnly,
    ] {
        for parts in [vec!["SAME", "SAME"], vec!["", "MIDDLE", ""]] {
            let (persisted, sent, streamed, final_answer) =
                observe_native_text_completion(parts.clone(), true, completion).await;
            assert_eq!(persisted, parts);
            assert_eq!(sent, parts);
            assert_eq!(streamed, parts.concat());
            assert_eq!(final_answer, streamed);
        }
    }
}

#[tokio::test]
async fn audit_responses_lifecycle_retains_provider_identity_and_sequence() {
    let workspace = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = audit_settings(db.clone()).await;
    let model = Arc::new(NativeTextModel {
        parts: vec!["AUDIT_IDENTITY"],
        include_deltas: true,
        completion: TextCompletion::Full,
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model, workspace.path());
    let run_id = "audit-responses-provider-identity";
    chat.run_in_session(
        "Audit Responses provider identity retention.",
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

    let lifecycle = chat
        .run_events(run_id, None)
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.event_type == "responses_stream_event"
                && matches!(
                    event.data["event_type"].as_str(),
                    Some("response.created" | "response.completed")
                )
        })
        .map(|event| event.data)
        .collect::<Vec<_>>();
    eprintln!("responses-identity audit: lifecycle={lifecycle:?}");
    assert_eq!(
        lifecycle.len(),
        2,
        "created and completed must both be durable"
    );
    assert!(
        lifecycle.iter().all(|event| {
            event["response_id"] == "audit-response"
                && event["provider_model"] == "deepseek-flash"
                && event["sequence_number"].is_u64()
        }),
        "provider identity and monotonic sequence must survive the formal run-event projection"
    );
    assert_eq!(lifecycle[0]["sequence_number"], 0);
    assert_eq!(lifecycle[1]["sequence_number"], 7);
}
