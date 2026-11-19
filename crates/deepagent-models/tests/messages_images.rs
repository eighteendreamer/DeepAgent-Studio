//! Messages keeps original image sources and ordered tool-result content.
use std::{
    collections::BTreeMap,
    io::Cursor,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use async_trait::async_trait;
use base64::Engine;
use deepagent_core::{
    message::{Message, MessageAttachment},
    tool_content::{ToolResultBody, ToolResultPart},
    Result,
};
use deepagent_models::{
    transport::EventSink, HttpTransport, ModelClient, ModelConfig, ResponseItem, ResponseRequest,
    TransportRequest, WireMode,
};
use serde_json::{json, Value};

#[path = "support/sse_protocol_fixture.rs"]
mod wire;

#[derive(Default)]
struct Capture(Mutex<Vec<TransportRequest>>);

#[async_trait]
impl HttpTransport for Capture {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0.lock().unwrap().push(request);
        for event in wire::start(WireMode::Messages)
            .into_iter()
            .chain(wire::finish(WireMode::Messages, "ok", 0))
        {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

fn attachment() -> MessageAttachment {
    MessageAttachment {
        id: "original".into(),
        kind: "image".into(),
        media_type: Some("image/png".into()),
        path: None,
        sha256: None,
    }
}

fn client(transport: Arc<Capture>) -> ModelClient {
    ModelClient::new(
        transport,
        ModelConfig::deepseek("synthetic").with_wire_mode(WireMode::Messages),
    )
}

fn request(url: &str) -> ResponseRequest {
    ResponseRequest::new(
        "deepseek-flash",
        vec![Message::user("inspect").with_attachments(vec![attachment()])],
    )
    .with_max_output_tokens(1024)
    .with_attachment_data_urls(BTreeMap::from([("original".into(), url.into())]))
}

fn raster(format: image::ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut out, format)
        .unwrap();
    out.into_inner()
}

#[tokio::test]
async fn user_images_preserve_all_four_original_formats_on_both_send_entries() {
    for cancelled in [false, true] {
        for (format, mime) in [
            (image::ImageFormat::Png, "image/png"),
            (image::ImageFormat::Jpeg, "image/jpeg"),
            (image::ImageFormat::Gif, "image/gif"),
            (image::ImageFormat::WebP, "image/webp"),
        ] {
            let bytes = raster(format);
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let mut request = request(&format!("data:{mime};base64,{encoded}"));
            if let ResponseItem::InputMessage { attachments, .. } = &mut request.input[0] {
                attachments[0].media_type = Some(mime.into());
            }
            let capture = Arc::new(Capture::default());
            let client = client(capture.clone());
            if cancelled {
                client
                    .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                    .await
            } else {
                client.stream_response(request).await
            }
            .unwrap();
            let recorded = capture.0.lock().unwrap();
            assert_eq!(recorded.len(), 1);
            let body: Value = serde_json::from_str(&recorded[0].body).unwrap();
            let content = &body["messages"][0]["content"];
            assert_eq!(content[0]["text"], "inspect");
            assert_eq!(
                content[1],
                json!({"type":"image","source":{"type":"base64","media_type":mime,"data":encoded}})
            );
            assert_eq!(
                base64::engine::general_purpose::STANDARD
                    .decode(content[1]["source"]["data"].as_str().unwrap())
                    .unwrap(),
                bytes
            );
        }
    }
}

#[tokio::test]
async fn tool_images_keep_occurrences_text_order_and_nested_tool_result_shape() {
    let url = "https://images.invalid/original.png?version=2";
    let mut request = request(url);
    request.input.extend([
        ResponseItem::FunctionCall {
            call_id: "call-1".into(),
            name: "screenshot".into(),
            arguments: "{}".into(),
        },
        ResponseItem::FunctionCallOutput {
            call_id: "call-1".into(),
            output: ToolResultBody::Parts(vec![
                ToolResultPart::Text {
                    text: "before".into(),
                },
                ToolResultPart::Image {
                    attachment: attachment(),
                },
                ToolResultPart::Text {
                    text: "after".into(),
                },
                ToolResultPart::Image {
                    attachment: attachment(),
                },
            ]),
        },
    ]);
    let capture = Arc::new(Capture::default());
    client(capture.clone())
        .stream_response(request)
        .await
        .unwrap();
    let body: Value = serde_json::from_str(&capture.0.lock().unwrap()[0].body).unwrap();
    assert_eq!(
        body["messages"][0]["content"][1]["source"],
        json!({"type":"url","url":url})
    );
    let result = &body["messages"][2]["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["tool_use_id"], "call-1");
    assert_eq!(
        result["content"],
        json!([
            {"type":"text","text":"before"}, {"type":"image","source":{"type":"url","url":url}},
            {"type":"text","text":"after"}, {"type":"image","source":{"type":"url","url":url}},
        ])
    );
}

#[tokio::test]
async fn image_only_messages_and_tool_results_omit_empty_text_blocks() {
    let mut request = request("https://images.invalid/original.png");
    if let ResponseItem::InputMessage { content, .. } = &mut request.input[0] {
        content.clear();
    }
    request.input.extend([
        ResponseItem::FunctionCall {
            call_id: "image-only".into(),
            name: "screenshot".into(),
            arguments: "{}".into(),
        },
        ResponseItem::FunctionCallOutput {
            call_id: "image-only".into(),
            output: ToolResultBody::Parts(vec![
                ToolResultPart::Text {
                    text: String::new(),
                },
                ToolResultPart::Image {
                    attachment: attachment(),
                },
            ]),
        },
    ]);
    let capture = Arc::new(Capture::default());
    client(capture.clone())
        .stream_response(request)
        .await
        .unwrap();
    let body: Value = serde_json::from_str(&capture.0.lock().unwrap()[0].body).unwrap();
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(
        content.len(),
        1,
        "image-only user input must not include an empty text block"
    );
    assert_eq!(content[0]["type"], "image");
    let content = body["messages"][2]["content"][0]["content"]
        .as_array()
        .unwrap();
    assert_eq!(
        content.len(),
        1,
        "image-only tool results must not include an empty text block"
    );
    assert_eq!(content[0]["type"], "image");
}

#[tokio::test]
async fn invalid_unresolved_unsupported_and_cancelled_images_never_reach_transport() {
    for url in [
        "data:image/png;base64,PHN2Zy8+",
        "data:image/svg+xml;base64,PHN2Zy8+",
        "file:///private.png",
        "",
    ] {
        let capture = Arc::new(Capture::default());
        assert!(client(capture.clone())
            .stream_response(request(url))
            .await
            .is_err());
        assert!(capture.0.lock().unwrap().is_empty());
    }
    let png = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(raster(image::ImageFormat::Png))
    );
    for case in [
        "unknown",
        "text-only",
        "assistant",
        "system",
        "developer",
        "cancelled",
    ] {
        let capture = Arc::new(Capture::default());
        let mut request = request(&png);
        if case == "unknown" {
            request.model = "unknown-model".into();
        }
        if case == "text-only" {
            request.model = "deepseek-v4-pro".into();
        }
        if ["assistant", "system", "developer"].contains(&case) {
            if let ResponseItem::InputMessage { role, .. } = &mut request.input[0] {
                *role = case.into();
            }
        }
        let result = client(capture.clone())
            .stream_response_cancelled(request, Arc::new(AtomicBool::new(case == "cancelled")))
            .await;
        assert!(result.is_err(), "{case}");
        assert!(capture.0.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn final_messages_image_body_is_still_bounded_before_transport() {
    let capture = Arc::new(Capture::default());
    let mut request = request("https://images.invalid/a.png");
    // Valid remote references can exceed the full JSON budget in aggregate.
    request.attachment_data_urls.insert(
        "original".into(),
        format!("https://images.invalid/{}", "a".repeat(49 * 1024 * 1024)),
    );
    let error = client(capture.clone())
        .stream_response(request)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("request_body_too_large"),
        "{error}"
    );
    assert!(capture.0.lock().unwrap().is_empty());
}
