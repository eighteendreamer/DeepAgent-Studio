use super::*;
use crate::chat::ToolSchema;
use crate::transport::{EventSink, HttpTransport, TransportRequest};
use async_trait::async_trait;
use deepagent_core::error::CoreError;
use deepagent_core::message::Message;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counter {
    calls: Arc<AtomicUsize>,
    chat: bool,
}

#[async_trait]
impl HttpTransport for Counter {
    async fn stream(&self, _: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.chat {
            sink.on_event(
                r#"{"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            )?;
            sink.on_event("[DONE]")?;
        } else {
            sink.on_event(r#"{"type":"response.completed","response":{"id":"r","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
        }
        Ok(())
    }
}

fn schemas(names: &[String]) -> Vec<ToolSchema> {
    names
        .iter()
        .map(|name| ToolSchema::function(name, "fixture", serde_json::json!({"type":"object"})))
        .collect()
}

#[tokio::test]
async fn h147_all_request_entrypoints_reject_bad_names_without_transport_or_value_leaks() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for native_chat in [false, true] {
            for cancelled in [false, true] {
                for names in [
                    vec![String::new()],
                    vec!["read.file".into()],
                    vec!["读取".into()],
                    vec!["x".repeat(129)],
                    vec!["same".into(), "same".into()],
                ] {
                    let calls = Arc::new(AtomicUsize::new(0));
                    let client = ModelClient::new(
                        Arc::new(Counter {
                            calls: calls.clone(),
                            chat: native_chat || mode == WireMode::ChatCompletions,
                        }),
                        ModelConfig::deepseek("placeholder").with_wire_mode(mode),
                    );
                    let cancel = Arc::new(AtomicBool::new(false));
                    let result = if native_chat {
                        let request = ChatCompletionRequest::new(
                            "deepseek-flash",
                            vec![Message::user("fixture")],
                        )
                        .with_tools(schemas(&names));
                        if cancelled {
                            client
                                .stream_chat_completion_cancelled(request, cancel)
                                .await
                        } else {
                            client.stream_chat_completion(request).await
                        }
                    } else {
                        let request =
                            ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")])
                                .with_tools(schemas(&names));
                        if cancelled {
                            client.stream_response_cancelled(request, cancel).await
                        } else {
                            client.stream_response(request).await
                        }
                    };
                    let error =
                        result.expect_err("invalid schema must be rejected before transport");
                    assert_eq!(calls.load(Ordering::SeqCst), 0);
                    assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
                    let error = error.to_string();
                    for name in &names {
                        if !name.is_empty() {
                            assert!(!error.contains(name), "diagnostic leaked name: {error}");
                        }
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h147_valid_boundary_length_and_case_sensitive_names_reach_both_wires() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = ModelClient::new(
            Arc::new(Counter {
                calls: calls.clone(),
                chat: mode == WireMode::ChatCompletions,
            }),
            ModelConfig::deepseek("placeholder").with_wire_mode(mode),
        );
        let request =
            ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")]).with_tools(
                schemas(&["Read_09-a".into(), "read_09-a".into(), "x".repeat(128)]),
            );
        client.stream_response(request).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
