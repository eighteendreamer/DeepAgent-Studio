//! Audit DeepSeek Responses numeric parameter validation at the final provider seam.

#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig, ResponseRequest};

#[derive(Default)]
struct RecordingTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for RecordingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn EventSink = &mut sequence_fixture::sink(sink);
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        for event in [
            r#"{"type":"response.output_text.delta","delta":"offline audit"}"#,
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

fn request() -> ResponseRequest {
    ResponseRequest::new(
        "deepseek-flash",
        vec![Message::user("Reply briefly for an offline audit.")],
    )
}

#[tokio::test]
#[ignore = "audit reproducer: out-of-range Responses numeric parameters must fail before transport"]
async fn audit_out_of_range_responses_parameters_are_rejected_before_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    let temperature_result = client
        .stream_response(request().with_temperature(2.1))
        .await;
    let top_p_result = client.stream_response(request().with_top_p(1.1)).await;
    let top_logprobs_result = client
        .stream_response(request().with_top_logprobs(21))
        .await;

    let requests = transport.requests.lock().unwrap();
    let observed: Vec<serde_json::Value> = requests
        .iter()
        .map(|wire| {
            serde_json::json!({
                "temperature": wire.get("temperature"),
                "top_p": wire.get("top_p"),
                "top_logprobs": wire.get("top_logprobs"),
            })
        })
        .collect();
    eprintln!(
        "responses numeric bounds audit: temperature_error={} top_p_error={} top_logprobs_error={} transport_calls={} observed={observed:?}",
        temperature_result.is_err(),
        top_p_result.is_err(),
        top_logprobs_result.is_err(),
        requests.len(),
    );

    assert!(
        temperature_result.is_err() && top_p_result.is_err() && top_logprobs_result.is_err(),
        "all out-of-range Responses parameters must be rejected at the final provider boundary"
    );
    assert_eq!(
        requests.len(),
        0,
        "invalid Responses parameters must not reach the provider transport"
    );
}

#[tokio::test]
#[ignore = "audit reproducer: out-of-range public ResponseDefaults must fail before transport"]
async fn audit_out_of_range_response_defaults_are_rejected_before_transport() {
    let cases = [
        (
            "temperature",
            ModelConfig::deepseek("offline").with_defaults(deepagent_models::ResponseDefaults {
                temperature: Some(2.1),
                ..Default::default()
            }),
        ),
        (
            "top_p",
            ModelConfig::deepseek("offline").with_defaults(deepagent_models::ResponseDefaults {
                top_p: Some(1.1),
                ..Default::default()
            }),
        ),
        (
            "top_logprobs",
            ModelConfig::deepseek("offline").with_defaults(deepagent_models::ResponseDefaults {
                top_logprobs: Some(21),
                ..Default::default()
            }),
        ),
    ];

    let mut all_rejected = true;
    let mut total_calls = 0;
    for (name, config) in cases {
        let transport = Arc::new(RecordingTransport::default());
        let client = ModelClient::new(transport.clone(), config);
        let result = client.stream_response(request()).await;
        let calls = transport.requests.lock().unwrap().len();
        eprintln!(
            "responses default bounds audit: field={name} error={} transport_calls={calls}",
            result.is_err()
        );
        all_rejected &= result.is_err();
        total_calls += calls;
    }
    assert!(
        all_rejected,
        "all invalid ResponseDefaults must be rejected"
    );
    assert_eq!(
        total_calls, 0,
        "invalid ResponseDefaults must not reach transport"
    );
}

#[tokio::test]
#[ignore = "audit control: documented Responses numeric boundary values remain serializable"]
async fn audit_documented_responses_parameter_boundaries_reach_transport() {
    let transport = Arc::new(RecordingTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder"),
    );

    client
        .stream_response(
            request()
                .with_temperature(0.0)
                .with_top_p(0.0)
                .with_top_logprobs(0),
        )
        .await
        .unwrap();
    client
        .stream_response(
            request()
                .with_temperature(2.0)
                .with_top_p(1.0)
                .with_top_logprobs(20),
        )
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    eprintln!("responses numeric bounds control: {requests:?}");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["temperature"], 0.0);
    assert_eq!(requests[0]["top_p"], 0.0);
    assert_eq!(requests[0]["top_logprobs"], 0);
    assert_eq!(requests[1]["temperature"], 2.0);
    assert_eq!(requests[1]["top_p"], 1.0);
    assert_eq!(requests[1]["top_logprobs"], 20);
}
