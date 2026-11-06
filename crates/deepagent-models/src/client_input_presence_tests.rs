//! H-150: validate the final request, including public edits and refreshed history.
use super::*;
use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(
        &self,
        request: TransportRequest,
        sink: &mut dyn crate::transport::EventSink,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        let events = if request.url.ends_with("/responses") {
            vec![
                r#"{"type":"response.completed","response":{"id":"fixture","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
            ]
        } else {
            vec![
                r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
                "[DONE]",
            ]
        };
        let mut sequence = crate::test_sequence::FixtureSequence::default();
        for event in events {
            if sink.on_event(&sequence.event(event))? {
                break;
            }
        }
        Ok(())
    }
}

async fn send(
    client: &ModelClient,
    request: ResponseRequest,
    entry: usize,
    cancelled: bool,
) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(cancelled));
    let mut observer = crate::stream::NoopObserver;
    match entry {
        0 => client.stream_response(request).await,
        1 => {
            client
                .stream_response_observed(request, &mut observer)
                .await
        }
        2 => client.stream_response_cancelled(request, cancel).await,
        3 => {
            client
                .stream_response_observed_cancelled(request, &mut observer, cancel)
                .await
        }
        _ => unreachable!(),
    }
}

fn invalid(error: CoreError) {
    assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    assert_eq!(
        crate::classify_model_error(&error),
        crate::ModelFailureKind::InvalidRequest
    );
    assert!(!crate::classify_model_error(&error).retryable());
    assert!(!error.to_string().contains("PRIVATE"));
}

#[tokio::test]
async fn h150_empty_final_input_is_rejected_by_all_response_entries_and_routes() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        );
        let mut edited =
            ResponseRequest::new("PRIVATE_MODEL", vec![Message::user("PRIVATE_HISTORY")]);
        edited.input.clear(); // Cached Chat projection must not bypass final public input.
        let mut refreshed = edited.clone();
        refreshed.replace_messages(&[]);
        let mut replaced = edited.clone();
        replaced.replace_history_from(&ResponseRequest::new("PRIVATE_MODEL", vec![]));
        let mut requests = vec![edited, refreshed, replaced];
        for instructions in [None, Some(""), Some(" \t\n\r"), Some("\u{2003}")] {
            requests.push(ResponseRequest::from_response_items(
                "PRIVATE_MODEL",
                instructions.map(str::to_owned),
                vec![],
            ));
        }
        for request in requests {
            invalid(request.validate().unwrap_err());
            for entry in 0..4 {
                invalid(
                    send(
                        &client,
                        client.prepare_response_request(request.clone()),
                        entry,
                        false,
                    )
                    .await
                    .unwrap_err(),
                );
            }
            // Validation precedes even a pre-cancelled transport invocation.
            for entry in [2, 3] {
                invalid(
                    send(&client, request.clone(), entry, true)
                        .await
                        .unwrap_err(),
                );
            }
        }
        assert!(transport.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn h150_instructions_only_user_only_and_repaired_tools_remain_sendable() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        );
        let system =
            ResponseRequest::new("deepseek-flash", vec![Message::system("synthetic rules")]);
        assert!(system.input.is_empty());
        let user =
            ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic question")]);
        let blank_instructions = ResponseRequest::from_response_items(
            "deepseek-flash",
            Some("  ".into()),
            user.input.clone(),
        );
        // The current repair never discards valid orphan outputs: it supplies
        // the matching call. Do not erase this history while fixing presence.
        let orphan = ResponseRequest::from_response_items(
            "deepseek-flash",
            None,
            vec![ResponseInputItem::FunctionCallOutput {
                call_id: "orphan".into(),
                output: "synthetic tool result".into(),
            }],
        );
        assert_eq!(orphan.input.len(), 2);
        for request in [system, user, blank_instructions, orphan] {
            request.validate().unwrap();
            for entry in 0..4 {
                send(&client, request.clone(), entry, false).await.unwrap();
            }
        }
        let bodies = transport.0.lock().unwrap();
        assert_eq!(bodies.len(), 16);
        if mode == WireMode::Responses {
            assert_eq!(bodies[0]["input"], json!([]));
            assert_eq!(bodies[0]["instructions"], "synthetic rules");
            assert_eq!(bodies[4]["input"][0]["content"], "synthetic question");
            assert_eq!(bodies[12]["input"][1]["output"], "synthetic tool result");
        } else {
            assert_eq!(bodies[0]["messages"][0]["role"], "system");
            assert_eq!(bodies[4]["messages"][0]["content"], "synthetic question");
        }
    }
}

#[tokio::test]
async fn h150_invalid_refresh_can_be_replaced_without_sticky_error_or_placeholder() {
    let transport = Arc::new(Recorder::default());
    let client = ModelClient::new(transport.clone(), ModelConfig::deepseek("synthetic"));
    let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("old history")]);
    request.replace_messages(&[]);
    invalid(client.stream_response(request.clone()).await.unwrap_err());
    assert!(transport.0.lock().unwrap().is_empty());
    request.replace_history_from(&ResponseRequest::new(
        "deepseek-flash",
        vec![Message::system("restored instructions")],
    ));
    client.stream_response(request).await.unwrap();
    client
        .stream_chat_completion(ChatCompletionRequest::new(
            "deepseek-flash",
            vec![Message::user("native Chat control")],
        ))
        .await
        .unwrap();
    let bodies = transport.0.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["instructions"], "restored instructions");
    assert_eq!(bodies[0]["input"], json!([]));
    assert_eq!(bodies[1]["messages"][0]["content"], "native Chat control");
}
