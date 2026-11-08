#![cfg(feature = "http")]

#[path = "support/sse_http_fixture.rs"]
mod fixture;

use deepagent_core::message::Message;
use deepagent_models::{ModelClient, ResponseRequest, WireMode};
use std::sync::{atomic::AtomicBool, Arc};

#[tokio::test]
async fn h30_real_http_uses_event_boundaries_for_both_protocols_and_dispatch_entries() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let payload = match mode {
            WireMode::Responses => fixture::response_terminal(),
            WireMode::ChatCompletions => serde_json::json!({"choices":[{"index":0,
                "delta":{"content":"H30 complete 中文🙂"},"finish_reason":"stop"}]})
            .to_string(),
        };
        for newline in ["\n", "\r", "\r\n"] {
            for multiline in [false, true] {
                for cancel_aware in [false, true] {
                    let payload = if multiline {
                        payload.replacen(':', &format!(":{newline}data: "), 1)
                    } else {
                        payload.clone()
                    };
                    let mut wire = format!("\u{feff}: heartbeat{newline}event: message{newline}data: {payload}{newline}{newline}");
                    if mode == WireMode::ChatCompletions {
                        wire.push_str(&format!("data: [DONE]{newline}{newline}"));
                    }
                    // Every byte gets its own HTTP chunk, including UTF-8 and CRLF splits.
                    let server =
                        fixture::Fixture::start(wire.bytes().map(|byte| vec![byte]).collect())
                            .await;
                    let client = ModelClient::new(
                        Arc::new(fixture::Fixture::transport()),
                        server.config(mode),
                    );
                    let request =
                        ResponseRequest::new("deepseek-v4-flash", vec![Message::user("synthetic")]);
                    let result = if cancel_aware {
                        client
                            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                            .await
                    } else {
                        client.stream_response(request).await
                    };
                    let requests = server.finish().await;
                    assert_eq!(requests.len(), 1);
                    let response = result.unwrap_or_else(|error| {
                        panic!("{mode:?}/{newline:?}/{multiline}/{cancel_aware}: {error}")
                    });
                    assert_eq!(response.output_text_projection(), "H30 complete 中文🙂");
                }
            }
        }
    }
}

#[tokio::test]
async fn h30_real_http_rejects_terminal_frames_without_the_final_blank_line() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for tail in ["", "\n", "\r\n", "\r"] {
            for cancel_aware in [false, true] {
                let wire = match mode {
                    WireMode::Responses => format!("data: {}{tail}",fixture::response_terminal()),
                    WireMode::ChatCompletions => format!(
                        "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"partial\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]{tail}"),
                };
                let server = fixture::Fixture::start(vec![wire.into_bytes()]).await;
                let client =
                    ModelClient::new(Arc::new(fixture::Fixture::transport()), server.config(mode));
                let request =
                    ResponseRequest::new("deepseek-v4-flash", vec![Message::user("synthetic")]);
                let result = if cancel_aware {
                    client
                        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                        .await
                } else {
                    client.stream_response(request).await
                };
                let requests = server.finish().await;
                assert_eq!(requests.len(), 1);
                let error = result.unwrap_err();
                assert!(
                    error.to_string().contains("terminal") || error.to_string().contains("[DONE]"),
                    "{error}"
                );
            }
        }
    }
}
