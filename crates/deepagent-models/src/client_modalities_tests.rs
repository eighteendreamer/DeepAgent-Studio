//! H-71: discovered capabilities govern the effective request, not its name.
use super::*;
use async_trait::async_trait;
use deepagent_core::{
    error::CoreError,
    message::{Message, MessageAttachment, ToolCall},
};
use serde_json::{json, Value};
use std::sync::Mutex;

const PIXEL: &str =
    concat!("data:image/png;base64,",
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=");

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
                r#"{"type":"response.completed","response":{"id":"h71","object":"response","created_at":1,"model":"fixture","output":[],"status":"completed"}}"#,
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

fn catalog(model: &str, modalities: Option<Value>) -> crate::ModelCatalog {
    let mut entry = json!({"id":model,"object":"model","owned_by":"deepseek"});
    if let Some(value) = modalities {
        entry["input_modalities"] = value;
    }
    serde_json::from_value(
        json!({"base_url":"https://api.deepseek.com", "available":[entry],
        "chat_model":model,"reasoner_model":model}),
    )
    .unwrap()
}

fn client(
    transport: Arc<Recorder>,
    model: &str,
    modalities: Option<Value>,
    route: usize,
) -> ModelClient {
    let config = ModelConfig::from_catalog(
        "PRIVATE_SYNTHETIC_KEY",
        &catalog(model, modalities),
        crate::ModelRole::Chat,
    )
    .with_wire_mode(if route == 1 {
        WireMode::ChatCompletions
    } else {
        WireMode::Responses
    });
    ModelClient::new(transport, config)
}

fn reference() -> MessageAttachment {
    MessageAttachment {
        id: "PRIVATE_IMAGE_ID".into(),
        kind: "image".into(),
        path: None,
        media_type: Some("image/png".into()),
        sha256: None,
    }
}

fn image(model: &str, role: &str) -> ResponseRequest {
    ResponseRequest::from_response_items(
        model,
        None,
        vec![ResponseInputItem::InputMessage {
            role: role.into(),
            content: "PRIVATE_TEXT".into(),
            attachments: vec![reference()],
        }],
    )
    .with_attachment_data_urls(std::collections::BTreeMap::from([(
        "PRIVATE_IMAGE_ID".into(),
        PIXEL.into(),
    )]))
}

async fn send(
    client: &ModelClient,
    request: ResponseRequest,
    route: usize,
    entry: usize,
    cancelled: bool,
) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(cancelled));
    let mut observer = crate::stream::NoopObserver;
    if route == 2 {
        let chat = response_to_chat_completion(request)?;
        match entry {
            0 => client.stream_chat_completion(chat).await,
            1 => {
                client
                    .stream_chat_completion_observed(chat, &mut observer)
                    .await
            }
            2 => client.stream_chat_completion_cancelled(chat, cancel).await,
            3 => {
                client
                    .stream_chat_completion_observed_cancelled(chat, &mut observer, cancel)
                    .await
            }
            _ => unreachable!(),
        }
    } else {
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
}

fn rejected(error: CoreError) {
    assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    assert!(error.to_string().contains("image_input_"), "{error}");
    assert!(!error.to_string().contains("PRIVATE"));
    assert!(!error.to_string().contains("data:image"));
    let kind = crate::classify_model_error(&error);
    assert_eq!(kind, crate::ModelFailureKind::InvalidRequest);
    assert!(!kind.retryable());
    assert!(!kind.should_fallback());
}

#[tokio::test]
async fn h171_roles_follow_the_selected_protocol_without_losing_images() {
    for route in 0..3 {
        for role in [
            "user",
            "developer",
            "system",
            "assistant",
            "tool",
            "unknown",
        ] {
            for entry in 0..4 {
                let transport = Arc::new(Recorder::default());
                let client = client(
                    transport.clone(),
                    "deepseek-flash",
                    Some(json!(["text", "image"])),
                    route,
                );
                let request = image("deepseek-flash", role);
                let result = send(&client, request, route, entry, false).await;
                let allowed = role == "user" || route == 0 && role == "developer";
                if allowed {
                    result.unwrap();
                    let requests = transport.0.lock().unwrap();
                    assert_eq!(requests.len(), 1);
                    let message = &requests[0][if route == 0 { "input" } else { "messages" }][0];
                    assert_eq!(message["role"], role);
                    let parts = message["content"]
                        .as_array()
                        .expect("legal image must remain typed");
                    assert_eq!(parts.len(), 2);
                    assert_eq!(parts[0]["text"], "PRIVATE_TEXT");
                    if route == 0 {
                        assert_eq!(parts[1]["type"], "input_image");
                        assert_eq!(parts[1]["image_url"], PIXEL);
                    } else {
                        assert_eq!(parts[1]["type"], "image_url");
                        assert_eq!(parts[1]["image_url"]["url"], PIXEL);
                    }
                } else {
                    rejected(result.unwrap_err());
                    assert!(transport.0.lock().unwrap().is_empty());
                }
            }
        }
    }
}

#[test]
fn h171_public_serialization_rejects_illegal_roles_and_unresolved_images() {
    for role in ["user", "developer", "system", "assistant"] {
        let mut request = image("deepseek-flash", role);
        let result = serde_json::to_value(&request);
        if matches!(role, "user" | "developer") {
            assert_eq!(result.unwrap()["input"][0]["role"], role);
        } else {
            assert!(
                result.is_err(),
                "serializer must not erase illegal {role} images"
            );
        }
        request.attachment_data_urls.clear();
        assert!(
            serde_json::to_value(&request).is_err(),
            "a missing URL is not a text-only message"
        );
    }
    for role in [
        deepagent_core::message::Role::System,
        deepagent_core::message::Role::Assistant,
    ] {
        let mut chat = ChatCompletionRequest::new(
            "deepseek-flash",
            vec![Message::text(role, "PRIVATE_TEXT").with_attachments(vec![reference()])],
        );
        chat.attachment_data_urls
            .insert(reference().id, PIXEL.into());
        assert!(
            serde_json::to_value(&chat).is_err(),
            "Chat serializer must also preserve the rejection boundary"
        );
    }
}

#[tokio::test]
async fn h171_image_only_multiple_images_and_text_controls_keep_their_order() {
    for role in ["user", "developer"] {
        for text in ["", "PRIVATE_TEXT"] {
            let transport = Arc::new(Recorder::default());
            let client = client(
                transport.clone(),
                "deepseek-flash",
                Some(json!(["text", "image"])),
                0,
            );
            let mut request = image("deepseek-flash", role);
            if let ResponseInputItem::InputMessage {
                content,
                attachments,
                ..
            } = &mut request.input[0]
            {
                *content = text.into();
                let mut other = reference();
                other.id = "second-image".into();
                attachments.push(other);
            }
            request.attachment_data_urls.insert(
                "second-image".into(),
                "https://images.example.invalid/second.png".into(),
            );
            client.stream_response(request).await.unwrap();
            let requests = transport.0.lock().unwrap();
            let parts = requests[0]["input"][0]["content"].as_array().unwrap();
            let offset = usize::from(!text.is_empty());
            assert_eq!(parts.len(), offset + 2);
            assert_eq!(parts[offset]["image_url"], PIXEL);
            assert_eq!(
                parts[offset + 1]["image_url"],
                "https://images.example.invalid/second.png"
            );
        }
    }
    for role in ["system", "assistant", "user", "developer"] {
        let transport = Arc::new(Recorder::default());
        let client = client(
            transport.clone(),
            "deepseek-flash",
            Some(json!(["text", "image"])),
            0,
        );
        let request = ResponseRequest::from_response_items(
            "deepseek-flash",
            None,
            vec![ResponseInputItem::Message {
                role: role.into(),
                content: "plain control".into(),
            }],
        );
        client.stream_response(request).await.unwrap();
        assert_eq!(
            transport.0.lock().unwrap()[0]["input"][0],
            json!({"type":"message", "role":role, "content":"plain control"})
        );
    }
}

#[tokio::test]
async fn h171_direct_chat_entries_reject_non_user_images_before_transport() {
    for role in [
        deepagent_core::message::Role::System,
        deepagent_core::message::Role::Assistant,
        deepagent_core::message::Role::Tool,
    ] {
        for entry in 0..4 {
            let transport = Arc::new(Recorder::default());
            let client = client(
                transport.clone(),
                "deepseek-flash",
                Some(json!(["text", "image"])),
                0,
            );
            let mut request = ChatCompletionRequest::new(
                "deepseek-flash",
                vec![Message::text(role, "PRIVATE_TEXT").with_attachments(vec![reference()])],
            );
            request
                .attachment_data_urls
                .insert(reference().id, PIXEL.into());
            let mut observer = crate::stream::NoopObserver;
            let cancel = Arc::new(AtomicBool::new(false));
            let result = match entry {
                0 => client.stream_chat_completion(request).await,
                1 => {
                    client
                        .stream_chat_completion_observed(request, &mut observer)
                        .await
                }
                2 => {
                    client
                        .stream_chat_completion_cancelled(request, cancel)
                        .await
                }
                3 => {
                    client
                        .stream_chat_completion_observed_cancelled(request, &mut observer, cancel)
                        .await
                }
                _ => unreachable!(),
            };
            rejected(result.unwrap_err());
            assert!(transport.0.lock().unwrap().is_empty());
        }
    }
}

#[test]
fn h71_modalities_survive_partial_capacity_and_roundtrip_without_name_guessing() {
    for declared in [
        json!(["text"]),
        json!(["text", "image"]),
        json!([]),
        json!(["future-kind"]),
    ] {
        let catalog = catalog("future-model", Some(declared.clone()));
        let encoded = serde_json::to_value(&catalog).unwrap();
        assert_eq!(encoded["available"][0]["input_modalities"], declared);
        let restored: crate::ModelCatalog = serde_json::from_value(encoded).unwrap();
        let transport = Arc::new(Recorder::default());
        let client = ModelClient::new(
            transport,
            ModelConfig::from_catalog("synthetic", &restored, crate::ModelRole::Chat),
        );
        let cap = client.capability("future-model");
        assert_eq!(
            serde_json::to_value(&cap).unwrap()["input_modalities"],
            declared
        );
        assert_eq!(
            (cap.context_window, cap.max_output_tokens),
            (128_000, 32_000)
        );
        let restored: crate::ModelCapability =
            serde_json::from_value(serde_json::to_value(cap).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap()["input_modalities"],
            declared
        );
    }
}

#[tokio::test]
async fn h71_all_entries_reject_unsupported_or_unknown_images_and_keep_text_controls() {
    for route in 0..3 {
        for declared in [
            Some(json!(["text"])),
            Some(json!([])),
            Some(json!(["future-kind"])),
            None,
        ] {
            let transport = Arc::new(Recorder::default());
            let client = client(transport.clone(), "PRIVATE_MODEL", declared, route);
            for entry in 0..4 {
                rejected(
                    send(&client, image("PRIVATE_MODEL", "user"), route, entry, false)
                        .await
                        .unwrap_err(),
                );
            }
            for entry in [2, 3] {
                rejected(
                    send(&client, image("PRIVATE_MODEL", "user"), route, entry, true)
                        .await
                        .unwrap_err(),
                );
            }
            assert!(transport.0.lock().unwrap().is_empty());
            for entry in 0..4 {
                send(
                    &client,
                    ResponseRequest::new("PRIVATE_MODEL", vec![Message::user("text control")]),
                    route,
                    entry,
                    false,
                )
                .await
                .unwrap();
            }
            assert_eq!(transport.0.lock().unwrap().len(), 4);
        }
    }
}

#[tokio::test]
async fn h71_provider_metadata_wins_and_final_model_history_are_rechecked() {
    for route in 0..3 {
        let transport = Arc::new(Recorder::default());
        let vision = client(
            transport.clone(),
            "PRIVATE_VISION",
            Some(json!(["text", "image"])),
            route,
        );
        for entry in 0..4 {
            send(
                &vision,
                image("PRIVATE_VISION", "user"),
                route,
                entry,
                false,
            )
            .await
            .unwrap();
        }
        let mut switched = image("PRIVATE_VISION", "user");
        switched.model = "PRIVATE_UNKNOWN".into();
        rejected(
            send(&vision, switched.clone(), route, 3, false)
                .await
                .unwrap_err(),
        );
        // Unreferenced cached URLs do not make a text request visual.
        switched.replace_messages(&[Message::user("restored text history")]);
        send(&vision, switched.clone(), route, 3, false)
            .await
            .unwrap();
        switched.replace_history_from(&image("PRIVATE_VISION", "user"));
        rejected(send(&vision, switched, route, 3, false).await.unwrap_err());
        let denied = client(
            transport.clone(),
            "deepseek-flash",
            Some(json!(["text"])),
            route,
        );
        rejected(
            send(&denied, image("deepseek-flash", "user"), route, 0, false)
                .await
                .unwrap_err(),
        );
        // Old catalogs remain readable; a documented exact id may use the
        // bundled snapshot, never a substring such as "future-flash-image".
        let legacy = client(transport.clone(), "deepseek-flash", None, route);
        send(&legacy, image("deepseek-flash", "user"), route, 3, false)
            .await
            .unwrap();
        let unknown = client(
            transport.clone(),
            "future-deepseek-flash-image",
            None,
            route,
        );
        rejected(
            send(
                &unknown,
                image("future-deepseek-flash-image", "user"),
                route,
                0,
                false,
            )
            .await
            .unwrap_err(),
        );
        assert_eq!(transport.0.lock().unwrap().len(), 6);
    }
}

#[tokio::test]
async fn h71_nonuser_references_cannot_evade_the_model_gate_by_becoming_text() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(Recorder::default());
        let client = client(
            transport.clone(),
            "PRIVATE_MODEL",
            Some(json!(["text"])),
            usize::from(mode == WireMode::ChatCompletions),
        );
        for role in ["developer", "assistant", "tool"] {
            rejected(
                client
                    .stream_response(image("PRIVATE_MODEL", role))
                    .await
                    .unwrap_err(),
            );
        }
        let tool = Message::tool_result("call-image", "synthetic tool result")
            .with_attachments(vec![reference()]);
        let assistant = Message::assistant("").with_tool_calls(vec![ToolCall {
            id: "call-image".into(),
            name: "synthetic_tool".into(),
            arguments: json!({}),
        }]);
        let mut native = ChatCompletionRequest::new(
            "PRIVATE_MODEL",
            vec![Message::user("control"), assistant, tool],
        );
        native
            .attachment_data_urls
            .insert("PRIVATE_IMAGE_ID".into(), PIXEL.into());
        rejected(client.stream_chat_completion(native).await.unwrap_err());
        assert!(transport.0.lock().unwrap().is_empty());
    }
}

#[cfg(feature = "http")]
#[tokio::test]
async fn h71_real_http_receives_only_a_declared_vision_request() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let transport = Arc::new(crate::ReqwestTransport::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
        ));
        let configuration = |modalities| {
            let mut config = ModelConfig::from_catalog(
                "synthetic-placeholder",
                &catalog("h71-http", Some(modalities)),
                crate::ModelRole::Chat,
            )
            .with_wire_mode(mode);
            config.base_url = format!("http://{}", listener.local_addr().unwrap());
            config
        };
        let denied = ModelClient::new(transport.clone(), configuration(json!(["text"])));
        rejected(
            denied
                .stream_response(image("h71-http", "user"))
                .await
                .unwrap_err(),
        );
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
        let allowed = ModelClient::new(transport, configuration(json!(["text", "image"])));
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let (offset, length) = loop {
                let mut buffer = [0; 2048];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
                assert!(received.len() < 16 * 1024);
                if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&received[..end]).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            assert!(length < 8 * 1024);
            while received.len() < offset + length {
                let mut buffer = [0; 2048];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&buffer[..count]);
            }
            let body: Value = serde_json::from_slice(&received[offset..offset + length]).unwrap();
            let key = if mode == WireMode::Responses {
                "input"
            } else {
                "messages"
            };
            let content = body[key][0]["content"].as_array().unwrap();
            assert_eq!(content.len(), 2);
            let url = if mode == WireMode::Responses {
                &content[1]["image_url"]
            } else {
                &content[1]["image_url"]["url"]
            };
            assert_eq!(url, PIXEL);
            let data = if mode == WireMode::Responses {
                "data: {\"type\":\"response.completed\",\"sequence_number\":0,\"response\":{\"id\":\"h71\",\"object\":\"response\",\"created_at\":1,\"model\":\"fixture\",\"output\":[],\"status\":\"completed\"}}\n\n"
            } else {
                "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len()).as_bytes()).await.unwrap();
        });
        timeout(
            Duration::from_secs(5),
            allowed.stream_response_cancelled(
                image("h71-http", "user"),
                Arc::new(AtomicBool::new(false)),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
