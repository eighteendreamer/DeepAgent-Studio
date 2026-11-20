//! H-258: feed production ChatService events into the production terminal state.

use super::*;
use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::EventPayload;
use deepagent_core::response_item::ResponseItem;
use deepagent_models::transport::EventSink;
use deepagent_models::{HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::{event_store::EventStore, Database};
use serde_json::json;
use std::sync::{Arc, Mutex};

const STALE: &str = "已废弃答案：甲。";
const FINAL: &str = "有效答案：乙。";
const STALE_REASONING: &str = "已废弃推理。";
const FINAL_REASONING: &str = "有效推理。";

#[derive(Clone, Copy, Debug)]
pub(crate) enum FirstAttempt {
    Success,
    DisconnectBeforeDelta,
    DisconnectAfterDelta,
}

struct Model {
    first: FirstAttempt,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait::async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let attempt = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len()
        };
        assert!(attempt <= 2, "unexpected extra provider request");
        let failed = attempt == 1 && !matches!(self.first, FirstAttempt::Success);
        if failed && matches!(self.first, FirstAttempt::DisconnectBeforeDelta) {
            return Err(CoreError::other(
                "connection interrupted by synthetic fixture",
            ));
        }
        let (text, reasoning) = if failed {
            (STALE, STALE_REASONING)
        } else {
            (FINAL, FINAL_REASONING)
        };
        let item = json!({
            "type":"message", "id":format!("message_{attempt}"),
            "role":"assistant", "status":"completed",
            "content":[{"type":"output_text","text":text,"annotations":[]}],
        });
        let mut events = vec![
            json!({"type":"response.created","response":{
                "id":format!("response_{attempt}"),"status":"in_progress","model":"deepseek-flash"}}),
            json!({"type":"response.reasoning_text.delta","delta":reasoning}),
            json!({"type":"response.output_text.delta","item_id":format!("message_{attempt}"),
                "output_index":0,"content_index":0,"delta":text}),
        ];
        if !failed {
            events.extend([
                json!({"type":"response.output_item.done","output_index":0,"item":item}),
                json!({"type":"response.completed","response":{"object":"response","created_at":1,
                    "id":format!("response_{attempt}"),"status":"completed","model":"deepseek-flash",
                    "output":[item],"usage":{"input_tokens":12,"output_tokens":6,"total_tokens":18}}}),
            ]);
        }
        for (sequence, mut event) in events.into_iter().enumerate() {
            event["sequence_number"] = sequence.into();
            if sink.on_event(&event.to_string())? {
                return Ok(());
            }
        }
        Err(CoreError::other(
            "connection interrupted by synthetic fixture",
        ))
    }
}

pub(crate) struct Observation {
    assistant: String,
    reasoning: String,
    rendered: String,
    pub(crate) runtime: Vec<RuntimeEvent>,
}

pub(crate) async fn observe(first: FirstAttempt) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let database_path = temporary.path().join("state.sqlite");
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = Arc::new(Database::open(&database_path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "synthetic-audit-placeholder")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", "deepseek-flash")
        .unwrap();
    let model = Arc::new(Model {
        first,
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    let state = Arc::new(Mutex::new(ChatState::new(None)));
    let runtime = Arc::new(Mutex::new(Vec::new()));
    let state_sink = state.clone();
    let runtime_sink = runtime.clone();
    let session = chat
        .run_in_session(
            "Synthetic terminal retry audit.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-terminal-retry"),
            move |event| {
                state_sink.lock().unwrap().on_runtime(event.clone());
                runtime_sink.lock().unwrap().push(event);
            },
            |_| {},
        )
        .await
        .unwrap();
    let events = runtime.lock().unwrap();
    let reset_positions = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(
                event,
                RuntimeEvent::ModelAttemptReset {
                    step: 0,
                    attempt: 1,
                    ..
                }
            )
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let expected_attempts = if matches!(first, FirstAttempt::Success) {
        1
    } else {
        2
    };
    assert_eq!(model.requests.lock().unwrap().len(), expected_attempts);
    assert_eq!(reset_positions.len(), expected_attempts - 1);
    if matches!(first, FirstAttempt::DisconnectAfterDelta) {
        let reset = reset_positions[0];
        assert!(events[..reset]
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ContentDelta { text } if text == STALE)));
        assert!(events[reset + 1..]
            .iter()
            .any(|e| matches!(e, RuntimeEvent::ContentDelta { text } if text == FINAL)));
    }
    let completed = events
        .iter()
        .filter_map(|event| {
            if let RuntimeEvent::RunCompleted { message, .. } = event {
                Some(message.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(completed, [FINAL]);
    let durable = chat.run_events("audit-terminal-retry", None).unwrap();
    let terminal = durable
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect::<Vec<_>>();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].data["kind"], "succeeded");
    assert_eq!(
        durable
            .iter()
            .filter(|e| e.event_type == "model_attempt_reset")
            .count(),
        expected_attempts - 1
    );
    let stored = EventStore::new(&db)
        .load_session(session.parse().unwrap())
        .unwrap();
    let persisted = stored
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::ResponseItemAppended {
                item: ResponseItem::Message { role, content },
                ..
            } if role == "assistant" => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(persisted, [FINAL]);
    let mut state = state.lock().unwrap();
    assert!(!state.busy);
    assert!(state.streaming.is_empty());
    let cells = state.drain_flushable();
    let assistant = cells
        .iter()
        .filter_map(|cell| match cell {
            HistoryCell::Assistant { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let reasoning = cells
        .iter()
        .filter_map(|cell| match cell {
            HistoryCell::Reasoning { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let rendered = cells
        .iter()
        .flat_map(|cell| cell.to_lines(100))
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let runtime = events.clone();
    eprintln!("terminal retry audit: first={first:?} attempts={expected_attempts} resets={} completed={completed:?} persisted={persisted:?} assistant={assistant:?} reasoning={reasoning:?}", reset_positions.len());
    drop(state);
    drop(events);
    drop(chat);
    drop(db);
    let reopened = Database::open(&database_path).unwrap();
    assert_eq!(
        deepagent_persistence::run_store::RunStore::new(&reopened)
            .events_after("audit-terminal-retry", None)
            .unwrap(),
        durable
    );
    assert_eq!(
        EventStore::new(&reopened)
            .load_session(session.parse().unwrap())
            .unwrap(),
        stored
    );
    Observation {
        assistant,
        reasoning,
        rendered,
        runtime,
    }
}

#[tokio::test]
async fn audit_terminal_normal_and_pre_delta_retry_preserve_one_answer() {
    for first in [FirstAttempt::Success, FirstAttempt::DisconnectBeforeDelta] {
        let observation = observe(first).await;
        assert_eq!(observation.assistant, FINAL);
        assert_eq!(observation.reasoning, FINAL_REASONING);
        assert!(!observation.rendered.contains(STALE));
    }
}

#[tokio::test]
async fn audit_terminal_retry_discards_failed_attempt_text_and_reasoning() {
    let observation = observe(FirstAttempt::DisconnectAfterDelta).await;
    assert!(
        observation.assistant == FINAL
            && observation.reasoning == FINAL_REASONING
            && !observation.rendered.contains(STALE),
        "TUI transcript must agree with the committed answer; assistant={:?} reasoning={:?}",
        observation.assistant,
        observation.reasoning,
    );
}
