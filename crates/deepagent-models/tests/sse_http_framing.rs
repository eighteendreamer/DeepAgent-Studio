#![cfg(feature = "http")]

#[path = "support/sse_http_fixture.rs"]
mod fixture;

use deepagent_core::message::Message;
use deepagent_models::{ModelClient, ResponseRequest};
use std::sync::{atomic::AtomicBool, Arc};

#[tokio::test]
async fn h30_real_http_uses_event_boundaries_for_all_protocols_and_dispatch_entries() {
    for mode in fixture::protocol::MODES {
        let mut payloads = fixture::protocol::start(mode);
        payloads.push(fixture::protocol::delta(mode, "H30 complete 中文🙂", 0));
        payloads.extend(fixture::protocol::finish(mode, "H30 complete 中文🙂", 1));
        for newline in ["\n", "\r", "\r\n"] {
            for multiline in [false, true] {
                for cancel_aware in [false, true] {
                    let mut wire = format!("\u{feff}: heartbeat{newline}");
                    for payload in &payloads {
                        let payload = if multiline {
                            payload.replacen(':', &format!(":{newline}data: "), 1)
                        } else {
                            payload.clone()
                        };
                        wire.push_str(&format!(
                            "event: message{newline}data: {payload}{newline}{newline}"
                        ));
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
    for mode in fixture::protocol::MODES {
        for tail in ["", "\n", "\r\n", "\r"] {
            for cancel_aware in [false, true] {
                let mut events = fixture::protocol::start(mode);
                events.push(fixture::protocol::delta(mode, "partial", 0));
                let mut terminal = fixture::protocol::finish(mode, "partial", 1);
                let last = terminal.pop().unwrap();
                events.extend(terminal);
                let mut wire = String::from_utf8(fixture::protocol::encode(events)).unwrap();
                wire.push_str(&format!("data: {last}{tail}"));
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
                    error.to_string().contains("terminal")
                        || error.to_string().contains("[DONE]")
                        || error.to_string().contains("message_stop"),
                    "{error}"
                );
            }
        }
    }
}
