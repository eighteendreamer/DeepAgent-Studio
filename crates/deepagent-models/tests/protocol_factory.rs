//! The protocol factory owns both sides of a physical model call.
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{message::Message, Result};
use deepagent_models::{
    ChatCompletionRequest, DeltaObserver, HttpTransport, ModelClient, ModelConfig,
    ModelStreamEvent, TransportRequest, Usage, WireMode,
};
use serde_json::{json, Value};

#[path = "support/sse_protocol_fixture.rs"]
mod wire;

#[derive(Default)]
struct Capture(Mutex<Vec<TransportRequest>>);

#[async_trait]
impl HttpTransport for Capture {
    async fn stream(
        &self,
        request: TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        let mode = wire::MODES
            .into_iter()
            .find(|mode| request.url.ends_with(wire::endpoint(*mode)))
            .expect("factory endpoint");
        self.0.lock().unwrap().push(request);
        for event in wire::start(mode)
            .into_iter()
            .chain([wire::delta(mode, "中文 🦀", 0)])
            .chain(wire::finish(mode, "中文 🦀", 1))
        {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Observer {
    text: String,
    usage: Option<Usage>,
}

impl DeltaObserver for Observer {
    fn on_event(&mut self, event: ModelStreamEvent) {
        if let ModelStreamEvent::ContentDelta { text } = event {
            self.text.push_str(&text);
        }
    }

    fn on_usage(&mut self, usage: Usage, _: &Value) {
        self.usage = Some(usage);
    }
}

#[tokio::test]
async fn every_factory_route_keeps_pricing_send_decode_and_usage_in_agreement() {
    for mode in wire::MODES {
        for cancelled in [false, true] {
            let transport = Arc::new(Capture::default());
            let client = ModelClient::new(
                transport.clone(),
                ModelConfig::deepseek("synthetic").with_wire_mode(mode),
            );
            let request = deepagent_models::ResponseRequest::new(
                "deepseek-v4-flash",
                vec![Message::user("fixture")],
            )
            .with_max_output_tokens(1024)
            .with_tools(vec![deepagent_models::ToolSchema::function(
                "read_file",
                "Read a file",
                json!({"type":"object","properties":{}}),
            )]);
            let size = client.response_prompt_byte_estimate(&request).unwrap();
            let mut observer = Observer::default();
            let response = if cancelled {
                client
                    .stream_response_observed_cancelled(
                        request,
                        &mut observer,
                        Arc::new(AtomicBool::new(false)),
                    )
                    .await
            } else {
                client
                    .stream_response_observed(request, &mut observer)
                    .await
            }
            .unwrap();
            let sent = transport.0.lock().unwrap();
            assert_eq!(sent.len(), 1, "{mode:?}");
            assert_eq!(
                sent[0].auth,
                if mode == WireMode::Messages {
                    deepagent_models::TransportAuth::Messages
                } else {
                    deepagent_models::TransportAuth::Bearer
                }
            );
            assert_eq!(sent[0].body.len(), size, "{mode:?}");
            assert_eq!(response.output_text_projection(), "中文 🦀");
            assert_eq!(observer.text, "中文 🦀");
            assert_eq!(observer.usage, response.usage);
            assert_eq!(response.usage.unwrap().total_tokens, 11);
        }
    }
}

#[tokio::test]
async fn explicit_chat_entry_uses_chat_codec_even_with_a_different_configured_route() {
    for mode in wire::MODES {
        let transport = Arc::new(Capture::default());
        let client = ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        );
        let response = client
            .stream_chat_completion(ChatCompletionRequest::new(
                "deepseek-v4-flash",
                vec![Message::user("fixture")],
            ))
            .await
            .unwrap();
        assert_eq!(response.output_text_projection(), "中文 🦀");
        assert!(transport.0.lock().unwrap()[0]
            .url
            .ends_with("/chat/completions"));
    }
}

#[test]
fn client_and_file_retry_do_not_select_protocol_streams_or_decoders() {
    let client = include_str!("../src/client.rs");
    let client = client.split("fn log_response_input_pairs").next().unwrap();
    let client = client.split("impl ModelClient {").nth(1).unwrap();
    let files = include_str!("../src/client/files.rs");
    for (name, source) in [("client", client), ("file retry", files)] {
        for owned_by_codec in [
            "ResponseAccumulator::new",
            "ChatCompletionAccumulator::new",
            "MessagesAccumulator::new",
            "wire_mode == WireMode::",
            "wire_mode != WireMode::",
            "matches!(self.config.wire_mode",
        ] {
            assert!(
                !source.contains(owned_by_codec),
                "{name} bypasses the factory with {owned_by_codec}"
            );
        }
    }
}
