//! Model discovery metadata and the provider request boundary, without network.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::{Message, MessageAttachment, ToolCall};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    ModelCatalog, ModelClient, ModelConfig, ModelDiscovery, ModelRole, ResponseInputItem,
    ResponseRequest,
};

// Public /models shape and declared modalities from DeepSeek's documentation
// retrieved 2026-10-22. No account data or live credentials are used.
const CATALOG: &str = r#"{"object":"list","data":[
  {"id":"deepseek-flash","object":"model","owned_by":"deepseek",
   "context_window":1048576,"max_output_tokens":393216,
   "input_modalities":["text","image"],"output_modalities":["text"]},
  {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek",
   "context_window":1048576,"max_output_tokens":393216,
   "input_modalities":["text"],"output_modalities":["text"]}
]}"#;

#[derive(Default)]
struct ModalityTransport {
    requests: Mutex<Vec<(String, usize)>>,
}

#[derive(Default)]
struct RoleImageTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

async fn reject_untrusted_inline_images(
    wire_mode: deepagent_models::WireMode,
    cancelled_entry: bool,
) {
    for url in [
        "data:image/svg+xml;base64,PHN2Zy8+",
        "data:image/png;base64,PHN2Zy8+",
        "data:image/png;base64,iVBORw0KGgo=",
    ] {
        let transport = Arc::new(RoleImageTransport::default());
        let mut config = ModelConfig::deepseek("offline-audit-placeholder");
        config.wire_mode = wire_mode;
        let client = ModelClient::new(transport.clone(), config);
        let request = ResponseRequest::new(
            "deepseek-flash",
            vec![
                Message::user("Inspect this image.").with_attachments(vec![MessageAttachment {
                    id: "h169-untrusted".into(),
                    kind: "image".into(),
                    media_type: Some("image/png".into()),
                    path: None,
                    sha256: None,
                }]),
            ],
        )
        .with_attachment_data_urls(BTreeMap::from([("h169-untrusted".into(), url.into())]));
        let result = if cancelled_entry {
            client
                .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                .await
        } else {
            client.stream_response(request).await
        };
        assert!(
            result.is_err(),
            "invalid inline image reached {wire_mode:?}"
        );
        assert!(transport.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn h169_responses_rejects_untrusted_inline_image_bytes_before_transport() {
    reject_untrusted_inline_images(deepagent_models::WireMode::Responses, true).await;
}

#[tokio::test]
async fn h169_chat_rejects_untrusted_inline_image_bytes_before_transport() {
    reject_untrusted_inline_images(deepagent_models::WireMode::ChatCompletions, true).await;
}

#[tokio::test]
async fn h169_non_cancel_send_entries_have_the_same_raster_gate() {
    reject_untrusted_inline_images(deepagent_models::WireMode::Responses, false).await;
    reject_untrusted_inline_images(deepagent_models::WireMode::ChatCompletions, false).await;
}

#[async_trait]
impl HttpTransport for RoleImageTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        if request.url.ends_with("/chat/completions") {
            sink.on_event(r#"{"id":"fixture-chat","object":"chat.completion.chunk","created":1,"model":"deepseek-flash","choices":[{"index":0,"delta":{"content":"Offline image."},"finish_reason":"stop"}]}"#)?;
            sink.on_event("[DONE]")?;
            return Ok(());
        }
        for event in [
            r#"{"type":"response.output_text.delta","sequence_number":0,"delta":"Offline image-role audit."}"#,
            r#"{"type":"response.completed","sequence_number":1,"response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn h169_both_protocols_keep_all_four_real_original_formats() {
    use base64::Engine;
    use deepagent_models::WireMode;
    use std::io::Cursor;
    for wire_mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for (format, mime) in [
            (image::ImageFormat::Jpeg, "image/jpeg"),
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::Gif, "image/gif"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            let mut bytes = Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(1, 1)
                .write_to(&mut bytes, format)
                .unwrap();
            let url = format!(
                "data:{mime};base64,{}",
                base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
            );
            let transport = Arc::new(RoleImageTransport::default());
            let mut config = ModelConfig::deepseek("offline-audit-placeholder");
            config.wire_mode = wire_mode;
            let client = ModelClient::new(transport.clone(), config);
            let request = ResponseRequest::new(
                "deepseek-flash",
                vec![Message::user("Read this raster.").with_attachments(vec![
                    MessageAttachment {
                        id: "original".into(),
                        kind: "image".into(),
                        media_type: Some(mime.into()),
                        path: None,
                        sha256: None,
                    },
                ])],
            )
            .with_attachment_data_urls(BTreeMap::from([("original".into(), url.clone())]));
            client.stream_response(request).await.unwrap();
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let sent = match wire_mode {
                WireMode::Responses => &requests[0]["input"][0]["content"][1]["image_url"],
                WireMode::ChatCompletions => {
                    &requests[0]["messages"][0]["content"][1]["image_url"]["url"]
                }
                WireMode::Messages => panic!("Messages image projection is not part of this audit"),
            };
            assert_eq!(sent, &serde_json::json!(url));
        }
    }
}

#[async_trait]
impl HttpTransport for ModalityTransport {
    async fn get_json(&self, url: &str, _key: &str) -> Result<String> {
        assert_eq!(url, "https://api.deepseek.com/models");
        Ok(CATALOG.into())
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let images = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .count();
        self.requests
            .lock()
            .unwrap()
            .push((body["model"].as_str().unwrap().into(), images));
        for event in [
            r#"{"type":"response.output_text.delta","sequence_number":0,"delta":"Offline modality audit."}"#,
            r#"{"type":"response.completed","sequence_number":1,"response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

struct Observation {
    accepted: bool,
    requests: Vec<(String, usize)>,
    modalities_retained: bool,
}

#[derive(Debug)]
struct RoleImageObservation {
    accepted: bool,
    transport_calls: usize,
    image_parts: usize,
    content_was_string: bool,
}

async fn sample_image_role(role: &str) -> RoleImageObservation {
    let transport = Arc::new(RoleImageTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );
    let request = ResponseRequest::from_response_items(
        "deepseek-flash",
        None,
        vec![ResponseInputItem::InputMessage {
            role: role.into(),
            content: "Interpret this role-scoped image.".into(),
            attachments: vec![MessageAttachment {

                sha256: None,
                id: "role-image".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
            }],
        }],
    )
    .with_attachment_data_urls(BTreeMap::from([(
        "role-image".into(),
        concat!(
            "data:image/png;base64,",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
        )
        .into(),
    )]));
    let accepted = client
        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
        .await
        .is_ok();
    let requests = transport.requests.lock().unwrap();
    let image_parts = requests
        .iter()
        .filter_map(|body| body["input"].as_array())
        .flatten()
        .filter_map(|item| item["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "input_image")
        .count();
    let content_was_string = requests
        .first()
        .and_then(|body| body["input"].as_array())
        .and_then(|input| input.first())
        .is_some_and(|item| item["content"].is_string());
    RoleImageObservation {
        accepted,
        transport_calls: requests.len(),
        image_parts,
        content_was_string,
    }
}

#[tokio::test]
async fn audit_responses_tool_output_images_are_preserved_before_transport() {
    let transport = Arc::new(RoleImageTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );
    let image_result =
        Message::tool_result("call-image", "screenshot ready").with_attachments(vec![
            MessageAttachment {
                sha256: None,
                id: "tool-image".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
            },
        ]);
    let request = ResponseRequest::new(
        "deepseek-flash",
        vec![
            Message::user("Inspect the screenshot returned by the tool."),
            Message::assistant("").with_tool_calls(vec![ToolCall {
                id: "call-image".into(),
                name: "take_screenshot".into(),
                arguments: serde_json::json!({}),
            }]),
            image_result,
        ],
    )
    .with_attachment_data_urls(BTreeMap::from([(
        "tool-image".into(),
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=".into(),
    )]));
    client
        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    let output = requests[0]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    let output_image_parts = output["output"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part["type"] == "input_image")
                .count()
        })
        .unwrap_or_default();
    eprintln!(
        "Responses tool-output image audit: output_image_parts={output_image_parts}, wire_output={}",
        output["output"]
    );
    assert_eq!(
        output_image_parts, 1,
        "a tool result image attachment was reduced to text before the Responses request"
    );
    assert_eq!(output["output"][0]["type"], "input_text");
    assert_eq!(output["output"][0]["text"], "screenshot ready");
    assert_eq!(
        output["output"][1]["image_url"],
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
    );
}

#[tokio::test]
async fn h127_function_custom_and_replayed_tool_images_use_one_typed_wire_boundary() {
    use deepagent_core::tool_content::{ToolResultBody, ToolResultPart};
    for custom in [false, true] {
        for cancelled_api in [false, true] {
            let transport = Arc::new(RoleImageTransport::default());
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig::deepseek("offline-placeholder"),
            );
            let image = MessageAttachment {
                id: "native-tool-image".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: Some("not-read-by-models.png".into()),
                sha256: Some("a".repeat(64)),
            };
            let body = ToolResultBody::Parts(vec![
                ToolResultPart::Text {
                    text: "before".into(),
                },
                ToolResultPart::Image { attachment: image },
                ToolResultPart::Text {
                    text: "after".into(),
                },
            ]);
            let call = if custom {
                ResponseInputItem::CustomToolCall {
                    call_id: "native-call".into(),
                    name: "apply_patch".into(),
                    input: "*** Begin Patch\n*** End Patch".into(),
                }
            } else {
                ResponseInputItem::FunctionCall {
                    call_id: "native-call".into(),
                    name: "screenshot".into(),
                    arguments: "{}".into(),
                }
            };
            let output = if custom {
                ResponseInputItem::CustomToolCallOutput {
                    call_id: "native-call".into(),
                    output: body,
                }
            } else {
                ResponseInputItem::FunctionCallOutput {
                    call_id: "native-call".into(),
                    output: body,
                }
            };
            let json = serde_json::to_string(&vec![call, output]).unwrap();
            assert!(!json.contains("base64"));
            let items = serde_json::from_str(&json).unwrap();
            let request = ResponseRequest::from_response_items("deepseek-flash", None, items)
                .with_attachment_data_urls(BTreeMap::from([(
                    "native-tool-image".into(),
                    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=".into(),
                )]));
            if cancelled_api {
                client
                    .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                    .await
                    .unwrap();
            } else {
                client.stream_response(request).await.unwrap();
            }
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let expected = if custom {
                "custom_tool_call_output"
            } else {
                "function_call_output"
            };
            let outputs: Vec<_> = requests[0]["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == expected)
                .collect();
            assert_eq!(outputs.len(), 1);
            assert_eq!(outputs[0]["call_id"], "native-call");
            assert_eq!(
                outputs[0]["output"][0],
                serde_json::json!({"type":"input_text","text":"before"})
            );
            assert_eq!(outputs[0]["output"][1]["type"], "input_image");
            assert_eq!(
                outputs[0]["output"][1]["image_url"],
                "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
            );
            assert_eq!(
                outputs[0]["output"][2],
                serde_json::json!({"type":"input_text","text":"after"})
            );
        }
    }
}

#[tokio::test]
async fn h127_tool_image_rejections_are_explicit_before_transport_and_text_stays_compatible() {
    use deepagent_models::WireMode;
    for (wire_mode, model, resolved) in [
        (WireMode::Responses, "deepseek-flash", false),
        (WireMode::Responses, "deepseek-v4-pro", true),
        (WireMode::ChatCompletions, "deepseek-flash", true),
    ] {
        let transport = Arc::new(RoleImageTransport::default());
        let mut config = ModelConfig::deepseek("offline-placeholder");
        config.wire_mode = wire_mode;
        // Verified catalog metadata, not a guess based on the model id.
        let mut capability =
            deepagent_models::ModelCapabilityResolver::new().resolve_model_id(model);
        capability.input_modalities = Some(if model == "deepseek-v4-pro" {
            vec!["text".into()]
        } else {
            vec!["text".into(), "image".into()]
        });
        config.capabilities = vec![capability];
        let client = ModelClient::new(transport.clone(), config);
        let make = |image: bool| {
            let result = Message::tool_result("call", "normal text");
            let result = if image {
                result.with_attachments(vec![MessageAttachment {
                    id: "rejection-image".into(),
                    kind: "image".into(),
                    media_type: Some("image/png".into()),
                    path: None,
                    sha256: None,
                }])
            } else {
                result
            };
            let mut request = ResponseRequest::new(
                model,
                vec![
                    Message::assistant("").with_tool_calls(vec![ToolCall {
                        id: "call".into(),
                        name: "screenshot".into(),
                        arguments: serde_json::json!({}),
                    }]),
                    result,
                ],
            );
            if resolved {
                request.attachment_data_urls.insert(
                    "rejection-image".into(),
                    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=".into(),
                );
            }
            request
        };
        let error = client
            .stream_response(make(true))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("image"), "{error}");
        assert!(transport.requests.lock().unwrap().is_empty());
        // The fixture returns Responses events; the no-image wire compatibility
        // control applies to that protocol, not an invented Chat response.
        if wire_mode == WireMode::Responses {
            client.stream_response(make(false)).await.unwrap();
            let requests = transport.requests.lock().unwrap();
            let output = requests[0]["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "function_call_output")
                .unwrap();
            assert_eq!(output["output"], "normal text");
        }
    }
}

#[tokio::test]
async fn h127_complete_request_counts_repeated_tool_image_occurrences() {
    use deepagent_core::tool_content::{ToolResultBody, ToolResultPart};
    let transport = Arc::new(RoleImageTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-placeholder"),
    );
    let image = MessageAttachment {
        id: "same-image".into(),
        kind: "image".into(),
        media_type: Some("image/png".into()),
        path: None,
        sha256: None,
    };
    let request = ResponseRequest::from_response_items(
        "deepseek-flash",
        None,
        vec![
            ResponseInputItem::FunctionCall {
                call_id: "many".into(),
                name: "screenshot".into(),
                arguments: "{}".into(),
            },
            ResponseInputItem::FunctionCallOutput {
                call_id: "many".into(),
                output: ToolResultBody::Parts(
                    (0..601)
                        .map(|_| ToolResultPart::Image {
                            attachment: image.clone(),
                        })
                        .collect(),
                ),
            },
        ],
    )
    .with_attachment_data_urls(BTreeMap::from([(
        "same-image".into(),
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=".into(),
    )]));
    let error = client
        .stream_response(request)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("image_input_count_exceeded"), "{error}");
    assert!(transport.requests.lock().unwrap().is_empty());
}

async fn sample(model: &str, with_image: bool) -> Observation {
    let transport = Arc::new(ModalityTransport::default());
    let discovered = ModelDiscovery::deepseek(transport.clone())
        .discover("offline-audit-placeholder")
        .await
        .unwrap();
    let encoded = serde_json::to_value(&discovered).unwrap();
    let modalities_retained = encoded["available"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model.get("input_modalities").is_some());
    let restored: ModelCatalog = serde_json::from_value(encoded).unwrap();
    let config = ModelConfig::from_catalog("offline-audit-placeholder", &restored, ModelRole::Chat);
    let client = ModelClient::new(transport.clone(), config);
    // Numeric capacity metadata is already preserved; test the independent
    // media contract without incorrectly claiming all metadata is discarded.
    assert_eq!(client.capability(model).context_window, 1_048_576);
    let item = if with_image {
        ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "Inspect the image.".into(),
            attachments: vec![MessageAttachment {
                sha256: None,
                id: "audit-pixel".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
            }],
        }
    } else {
        ResponseInputItem::Message {
            role: "user".into(),
            content: "Text control.".into(),
        }
    };
    // Supply the valid resolved bytes at the provider seam. This deliberately
    // excludes H-67's current-user attachment assembly and filesystem handling.
    let urls = if with_image {
        BTreeMap::from([("audit-pixel".into(), concat!(
        "data:image/png;base64,",
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
    ).into())])
    } else {
        BTreeMap::new()
    };
    let request = ResponseRequest::from_response_items(model, None, vec![item])
        .with_attachment_data_urls(urls);
    let accepted = client
        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
        .await
        .is_ok();
    let requests = transport.requests.lock().unwrap().clone();
    eprintln!("model-modality model={model} image={with_image} numeric_capacity_retained=true modalities_retained={modalities_retained} transport_requests={requests:?} accepted={accepted}");
    Observation {
        accepted,
        requests,
        modalities_retained,
    }
}

#[tokio::test]
async fn supported_image_and_text_only_requests_reach_transport() {
    let image = sample("deepseek-flash", true).await;
    assert!(image.accepted);
    assert_eq!(image.requests, vec![("deepseek-flash".into(), 1)]);
    let text = sample("deepseek-v4-pro", false).await;
    assert!(text.accepted);
    assert_eq!(text.requests, vec![("deepseek-v4-pro".into(), 0)]);
}

#[tokio::test]
async fn audit_declared_input_modalities_constrain_requests() {
    let text_only = sample("deepseek-v4-pro", true).await;
    assert!(
        text_only.modalities_retained && !text_only.accepted && text_only.requests.is_empty(),
        "declared input modalities were lost and an unsupported image request reached transport"
    );
}

#[tokio::test]
async fn audit_responses_image_roles_preserve_or_reject_content() {
    let developer = sample_image_role("developer").await;
    let assistant = sample_image_role("assistant").await;
    eprintln!("responses image-role audit: developer={developer:?} assistant={assistant:?}");
    assert_eq!(
        (
            developer.accepted,
            developer.transport_calls,
            developer.image_parts,
            developer.content_was_string,
        ),
        (true, 1, 1, false),
        "DeepSeek Responses treats developer as user, so its input_image must not be discarded"
    );
    assert_eq!(
        (
            assistant.accepted,
            assistant.transport_calls,
            assistant.image_parts,
            assistant.content_was_string,
        ),
        (false, 0, 0, false),
        "an assistant image must be rejected before transport instead of becoming a successful text-only request"
    );
}
