//! Audit the model-specific reasoning metadata at the Responses request seam.

#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::Message;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    ModelCatalog, ModelClient, ModelConfig, ModelDiscovery, ModelRole, ReasoningEffort,
    ResponseDefaults, ResponseRequest, ToolSchema,
};

const CATALOG: &str = r#"{"object":"list","data":[{
  "id":"deepseek-effort-limited","object":"model","owned_by":"deepseek",
  "context_window":1048576,"max_output_tokens":393216,
  "input_modalities":["text"],"output_modalities":["text"],
  "effort":{"supported_levels":["low"],"default_level":"low"},
  "api_capabilities":{}
}]}"#;

#[derive(Default)]
struct AuditTransport {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for AuditTransport {
    async fn get_json(&self, url: &str, _key: &str) -> Result<String> {
        assert_eq!(url, "https://api.deepseek.com/models");
        Ok(CATALOG.to_string())
    }

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

#[tokio::test]
#[ignore = "audit reproducer: model-advertised effort levels must constrain the Responses request"]
async fn audit_model_effort_metadata_constrains_responses_request() {
    let transport = Arc::new(AuditTransport::default());
    let catalog = ModelDiscovery::deepseek(transport.clone())
        .discover("offline-audit-placeholder")
        .await
        .unwrap();
    let serialized_catalog = serde_json::to_value(&catalog).unwrap();
    let effort_metadata_retained = serialized_catalog["available"][0].get("effort").is_some();
    let restored: ModelCatalog = serde_json::from_value(serialized_catalog).unwrap();
    let config = ModelConfig::from_catalog("offline-audit-placeholder", &restored, ModelRole::Chat)
        .with_defaults(ResponseDefaults {
            reasoning_effort: Some(ReasoningEffort::High),
            ..ResponseDefaults::default()
        });
    let client = ModelClient::new(transport.clone(), config);
    let request = ResponseRequest::new(
        "deepseek-effort-limited",
        vec![Message::user("Reply briefly.")],
    );
    client.stream_response(request).await.unwrap();

    let requests = transport.requests.lock().unwrap();
    let actual_effort = requests[0]["reasoning"]["effort"].as_str();
    eprintln!(
        "model-effort audit: metadata_retained={effort_metadata_retained} advertised=[low] default=low wire_effort={actual_effort:?}"
    );
    assert!(
        effort_metadata_retained && actual_effort != Some("high"),
        "the discovered model declares only low effort, but the catalog discarded that capability and the Responses request still sent high"
    );
}

#[tokio::test]
#[ignore = "audit reproducer: DeepSeek Flash thinking-mode top_p below 0.95 is normalized by the provider"]
async fn audit_flash_thinking_mode_top_p_below_floor_reaches_wire_unchanged() {
    let transport = Arc::new(AuditTransport::default());
    let config =
        ModelConfig::deepseek("offline-audit-placeholder").with_defaults(ResponseDefaults {
            reasoning_effort: Some(ReasoningEffort::High),
            top_p: Some(0.2),
            ..ResponseDefaults::default()
        });
    let client = ModelClient::new(transport.clone(), config);
    client
        .stream_response(ResponseRequest::new(
            "deepseek-flash",
            vec![Message::user("Reply briefly.")],
        ))
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    let wire_effort = requests[0]["reasoning"]["effort"].as_str();
    let wire_top_p = requests[0]["top_p"].as_f64();
    eprintln!(
        "flash top_p audit: wire_effort={wire_effort:?}, configured_top_p=0.2, wire_top_p={wire_top_p:?}, documented_effective_top_p=0.95"
    );
    assert_eq!(wire_effort, Some("high"));
    assert_ne!(
        wire_top_p,
        Some(0.2),
        "in DeepSeek Flash thinking mode, a configured top_p below 0.95 is normalized by the provider; local config and wire should not imply that 0.2 is effective"
    );
}

#[tokio::test]
#[ignore = "audit reproducer: DeepSeek model/reasoning/tool combinations do not all honor explicit top_p"]
async fn audit_model_specific_top_p_rules_reach_wire() {
    let transport = Arc::new(AuditTransport::default());
    let config =
        ModelConfig::deepseek("offline-audit-placeholder").with_defaults(ResponseDefaults {
            reasoning_effort: Some(ReasoningEffort::High),
            top_p: Some(0.2),
            ..ResponseDefaults::default()
        });
    let client = ModelClient::new(transport.clone(), config);

    let without_tools =
        ResponseRequest::new("deepseek-v4-pro", vec![Message::user("Reply briefly.")]);
    client.stream_response(without_tools).await.unwrap();

    let with_tool = ResponseRequest::new("deepseek-v4-pro", vec![Message::user("Reply briefly.")])
        .with_tools(vec![ToolSchema::function(
            "read_file",
            "Read a file.",
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        )]);
    client.stream_response(with_tool).await.unwrap();

    let pro_without_thinking =
        ResponseRequest::new("deepseek-v4-pro", vec![Message::user("Reply briefly.")])
            .with_reasoning_effort(ReasoningEffort::None);
    client.stream_response(pro_without_thinking).await.unwrap();

    let flash_without_thinking =
        ResponseRequest::new("deepseek-flash", vec![Message::user("Reply briefly.")])
            .with_reasoning_effort(ReasoningEffort::None);
    client
        .stream_response(flash_without_thinking)
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    let observed: Vec<_> = requests
        .iter()
        .map(|request| {
            (
                request["model"].as_str(),
                request["tools"]
                    .as_array()
                    .is_some_and(|tools| !tools.is_empty()),
                request["reasoning"]["effort"].as_str(),
                request["top_p"].as_f64(),
            )
        })
        .collect();
    let documented_effective_top_p = [1.0, 0.95, 0.2, 1.0];
    eprintln!(
        "thinking top_p audit: configured_top_p=0.2, requests={observed:?}, documented_effective_top_p=[pro_without_tools:{}, pro_with_tools:{}, pro_none:{}, flash_none:{}]",
        documented_effective_top_p[0],
        documented_effective_top_p[1],
        documented_effective_top_p[2],
        documented_effective_top_p[3]
    );

    assert_eq!(observed.len(), 4);
    assert_eq!(
        observed[0],
        (Some("deepseek-v4-pro"), false, Some("high"), Some(0.2))
    );
    assert_eq!(
        observed[1],
        (Some("deepseek-v4-pro"), true, Some("high"), Some(0.2))
    );
    assert_eq!(
        observed[2],
        (Some("deepseek-v4-pro"), false, Some("none"), Some(0.2))
    );
    assert_eq!(
        observed[3],
        (Some("deepseek-flash"), false, Some("none"), Some(0.2))
    );
    assert!(
        observed
            .iter()
            .zip(documented_effective_top_p)
            .all(|((_, _, _, configured_wire_value), effective_value)| {
                *configured_wire_value == Some(effective_value)
            }),
        "DeepSeek Pro thinking ignores configured top_p (effective 1.0 without tools and 0.95 with tools), and Flash non-thinking also ignores it (effective 1.0); the Pro non-thinking control uses configured 0.2"
    );
}
