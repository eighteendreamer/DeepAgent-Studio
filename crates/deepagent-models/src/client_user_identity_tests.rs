//! H-146: defaults and overrides reach the protocol-specific identity boundary.
use super::*;
use async_trait::async_trait;
use deepagent_core::{error::CoreError, message::Message};
use serde_json::Value;
use std::sync::Mutex;

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(
        &self,
        request: TransportRequest,
        _: &mut dyn crate::transport::EventSink,
    ) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        Err(CoreError::other("synthetic-dispatched"))
    }
}

async fn dispatch(
    client: &ModelClient,
    user: Option<String>,
    native_chat: bool,
    cancel_entry: bool,
) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(false));
    if native_chat {
        let mut request =
            ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")]);
        request.user = user;
        if cancel_entry {
            client
                .stream_chat_completion_cancelled(request, cancel)
                .await
        } else {
            client.stream_chat_completion(request).await
        }
    } else {
        let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")]);
        request.user = user;
        if cancel_entry {
            client.stream_response_cancelled(request, cancel).await
        } else {
            client.stream_response(request).await
        }
    }
}

#[tokio::test]
async fn h146_user_identity_all_chat_entries_preserve_defaults_overrides_and_utf8() {
    // The official limit is bytes, not Unicode scalar values. ASCII is a
    // recommendation, not a reason to reject valid <=512-byte UTF-8 values.
    for native_chat in [false, true] {
        for cancel_entry in [false, true] {
            for (configured, requested) in [
                (None, None),
                (Some("default-user".into()), None),
                (Some("default-user".into()), Some("override-user".into())),
                (Some("x".repeat(513)), Some("override-user".into())),
                (None, Some("x".repeat(512))),
                (None, Some("界".repeat(170) + "ab")),
                (None, Some(String::new())),
            ] {
                let expected = requested.clone().or(configured.clone());
                let transport = Arc::new(Recorder::default());
                let mut config = ModelConfig::deepseek_chat("synthetic");
                config.defaults.user = configured;
                let client = ModelClient::new(transport.clone(), config);
                let result = dispatch(&client, requested, native_chat, cancel_entry).await;
                assert_eq!(result.unwrap_err().to_string(), "synthetic-dispatched");
                let bodies = transport.0.lock().unwrap();
                assert_eq!(bodies.len(), 1);
                assert_eq!(
                    bodies[0].get("user_id"),
                    expected
                        .as_ref()
                        .map(|value| Value::String(value.clone()))
                        .as_ref()
                );
                assert!(bodies[0].get("user").is_none());
            }
        }
    }
}

#[tokio::test]
async fn h146_user_identity_oversized_chat_values_fail_before_transport_without_value_leak() {
    for native_chat in [false, true] {
        for cancel_entry in [false, true] {
            for defaults in [false, true] {
                for value in ["x".repeat(513), "界".repeat(171)] {
                    let transport = Arc::new(Recorder::default());
                    let mut config = ModelConfig::deepseek_chat("synthetic");
                    if defaults {
                        config.defaults.user = Some(value.clone());
                    }
                    let client = ModelClient::new(transport.clone(), config);
                    let result = dispatch(
                        &client,
                        (!defaults).then(|| value.clone()),
                        native_chat,
                        cancel_entry,
                    )
                    .await;
                    let error = result.unwrap_err();
                    assert!(matches!(error, CoreError::Invalid(_)));
                    assert!(!error.to_string().contains(&value));
                    assert!(transport.0.lock().unwrap().is_empty());
                }
            }
        }
    }
}

#[tokio::test]
async fn h146_user_identity_responses_transport_keeps_user_and_omits_user_id() {
    for cancel_entry in [false, true] {
        for (configured, requested) in [
            (None, None),
            (Some("responses-default".into()), None),
            (
                Some("responses-default".into()),
                Some("responses-request".into()),
            ),
            (None, Some("x".repeat(512))),
        ] {
            let expected = requested.clone().or(configured.clone());
            let transport = Arc::new(Recorder::default());
            let mut config = ModelConfig::deepseek("synthetic");
            config.defaults.user = configured;
            let client = ModelClient::new(transport.clone(), config);
            let result = dispatch(&client, requested, false, cancel_entry).await;
            assert_eq!(result.unwrap_err().to_string(), "synthetic-dispatched");
            let bodies = transport.0.lock().unwrap();
            assert_eq!(bodies.len(), 1);
            assert_eq!(
                bodies[0].get("user"),
                expected
                    .as_ref()
                    .map(|value| Value::String(value.clone()))
                    .as_ref()
            );
            assert!(bodies[0].get("user_id").is_none());
        }
    }
}
