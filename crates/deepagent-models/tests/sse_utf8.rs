//! H-257: byte corruption must not depend on HTTP chunking or model protocol.
#![cfg(feature = "http")]

#[path = "support/sse_http_fixture.rs"]
mod fixture;

use deepagent_core::{error::CoreError, message::Message};
use deepagent_models::{
    classify_model_error, DeltaObserver, FinishReason, ModelClient, ModelFailureKind,
    ModelStreamEvent, Response, ResponseRequest, WireMode,
};
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Default)]
struct Observed(Vec<ModelStreamEvent>);

impl DeltaObserver for Observed {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(event);
    }
}

impl Observed {
    fn text(&self) -> String {
        self.0
            .iter()
            .filter_map(|event| match event {
                ModelStreamEvent::ContentDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn assert_no_terminal_or_usage(&self) {
        assert!(!self.0.iter().any(|event| matches!(
            event,
            ModelStreamEvent::Usage { .. } | ModelStreamEvent::Finished { .. }
        )));
    }
}

fn delta(mode: WireMode, text: &str) -> Vec<u8> {
    let payload = match mode {
        WireMode::Responses => serde_json::json!({
            "type":"response.output_text.delta","sequence_number":0,"item_id":"h257-output",
            "output_index":0,"content_index":0,"delta":text}),
        WireMode::ChatCompletions => serde_json::json!({"choices":[{
            "index":0,"delta":{"content":text},"finish_reason":null}]}),
    };
    format!("data: {payload}\n\n").into_bytes()
}

fn terminal(mode: WireMode, text: &str) -> Vec<u8> {
    match mode {
        WireMode::Responses => {
            let mut value: serde_json::Value =
                serde_json::from_str(&fixture::response_terminal()).unwrap();
            value["sequence_number"] = 1.into();
            value["response"]["output"][0]["id"] = "h257-output".into();
            value["response"]["output"][0]["content"][0]["text"] = text.into();
            value["response"]["usage"] = serde_json::json!({
                "input_tokens":9,"output_tokens":2,"total_tokens":11});
            format!("data: {value}\n\n").into_bytes()
        }
        WireMode::ChatCompletions => b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2,\"total_tokens\":11}}\n\ndata: [DONE]\n\n".to_vec(),
    }
}

async fn observe(
    mode: WireMode,
    cancel_aware: bool,
    chunks: Vec<Vec<u8>>,
) -> (deepagent_core::error::Result<Response>, Observed) {
    let server = fixture::Fixture::start(chunks).await;
    let client = ModelClient::new(Arc::new(fixture::Fixture::transport()), server.config(mode));
    let request = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("synthetic")]);
    let mut observed = Observed::default();
    let result = if cancel_aware {
        client
            .stream_response_observed_cancelled(
                request,
                &mut observed,
                Arc::new(AtomicBool::new(false)),
            )
            .await
    } else {
        client
            .stream_response_observed(request, &mut observed)
            .await
    };
    assert_eq!(server.finish().await.len(), 1);
    (result, observed)
}

#[tokio::test]
async fn h257_invalid_bytes_preserve_valid_prefix_and_absolute_offset_across_http_chunks() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for before_delta in [false, true] {
                for invalid in [
                    vec![0xff],
                    vec![0xe2, 0x28, 0xa1],
                    vec![0xf0, 0x9f, 0x99, 0x28],
                ] {
                    let mut prefix = ": synthetic é中🙂\n\n".as_bytes().to_vec();
                    if before_delta {
                        prefix.extend(delta(mode, "before|"));
                    }
                    let offset = prefix.len();
                    let mut wire = prefix.clone();
                    wire.extend(&invalid);
                    wire.extend(delta(mode, "after|"));
                    wire.extend(terminal(mode, "after|"));
                    for chunks in [
                        vec![wire.clone()],
                        wire.iter().map(|byte| vec![*byte]).collect(),
                        vec![
                            prefix.clone(),
                            invalid[..1].to_vec(),
                            wire[offset + 1..].to_vec(),
                        ],
                    ] {
                        let (result, observed) = observe(mode, cancel_aware, chunks).await;
                        let error = result.unwrap_err();
                        assert!(matches!(error, CoreError::Serialization(_)), "{error}");
                        assert!(
                            error
                                .to_string()
                                .contains(&format!("invalid UTF-8 at byte {offset} ")),
                            "{mode:?}: {error}"
                        );
                        assert_eq!(
                            classify_model_error(&error),
                            ModelFailureKind::InvalidRequest
                        );
                        assert!(!classify_model_error(&error).retryable());
                        assert!(!classify_model_error(&error).should_fallback());
                        assert_eq!(observed.text(), if before_delta { "before|" } else { "" });
                        observed.assert_no_terminal_or_usage();
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn h257_eof_rejects_each_incomplete_tail_without_fabricating_a_terminal() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for tail in [vec![0xc3], vec![0xe2, 0x82], vec![0xf0, 0x9f, 0x99]] {
                let mut wire = delta(mode, "before|é中🙂");
                wire.extend(&tail);
                let (result, observed) = observe(mode, cancel_aware, vec![wire]).await;
                let error = result.unwrap_err();
                assert!(matches!(error, CoreError::Serialization(_)), "{error}");
                assert!(
                    error.to_string().contains(&format!(
                        "incomplete UTF-8 sequence ({} trailing bytes)",
                        tail.len()
                    )),
                    "{error}"
                );
                assert_eq!(observed.text(), "before|é中🙂");
                observed.assert_no_terminal_or_usage();
            }
        }
    }
}

#[tokio::test]
async fn h257_complete_unicode_and_terminal_short_circuit_keep_the_existing_contract() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        for cancel_aware in [false, true] {
            for extra_bytes_after_terminal in [false, true] {
                let mut wire = delta(mode, "é中文🙂");
                wire.extend(terminal(mode, "é中文🙂"));
                if extra_bytes_after_terminal {
                    wire.push(0xff);
                }
                for chunks in [
                    vec![wire.clone()],
                    wire.iter().map(|byte| vec![*byte]).collect(),
                ] {
                    let (result, observed) = observe(mode, cancel_aware, chunks).await;
                    let response = result.unwrap();
                    assert_eq!(response.output_text_projection(), "é中文🙂");
                    assert_eq!(observed.text(), "é中文🙂");
                    assert_eq!(response.finish_reason, Some(FinishReason::Stop));
                    let usage = response.usage.unwrap();
                    assert_eq!((usage.prompt_tokens, usage.completion_tokens), (9, 2));
                }
            }
        }
    }
}
