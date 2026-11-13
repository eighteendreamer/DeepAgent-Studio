//! Audit the model-specific reasoning metadata at the Responses request seam.

#[path = "support/sequence.rs"]
pub mod sequence_fixture;
use std::sync::atomic::AtomicBool;
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
    assert_eq!(
        wire_top_p,
        Some(0.95),
        "in DeepSeek Flash thinking mode, a configured top_p below 0.95 is normalized by the provider; local config and wire should not imply that 0.2 is effective"
    );
}

#[tokio::test]
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
    assert_eq!(observed[0].0, Some("deepseek-v4-pro"));
    assert_eq!(observed[1].0, Some("deepseek-v4-pro"));
    assert_eq!(observed[2].2, Some("none"));
    assert_eq!(observed[3].0, Some("deepseek-flash"));
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

async fn sampling_case_violation(
    model: &str,
    effort: Option<ReasoningEffort>,
    tool_mode: &str,
    configured: f32,
    from_defaults: bool,
    cancel_aware: bool,
) -> (bool, Option<String>) {
    let thinking = effort != Some(ReasoningEffort::None);
    // Independent values from the DeepSeek Responses docs, not production code.
    // tool_choice=none disables tools even when schemas are supplied.
    let expected = match (model, thinking, tool_mode) {
        ("deepseek-flash", true, _) => configured.max(0.95),
        ("deepseek-flash", false, _) => 1.0,
        ("deepseek-v4-pro", true, "auto") => 0.95,
        ("deepseek-v4-pro", true, _) => 1.0,
        _ => configured,
    };
    let transport = Arc::new(AuditTransport::default());
    let mut defaults = ResponseDefaults::default();
    let mut request = ResponseRequest::new(model, vec![Message::user("offline sampling fixture")]);
    if from_defaults {
        defaults.top_p = Some(configured);
        defaults.reasoning_effort = effort;
    } else {
        request.top_p = Some(configured);
        request.reasoning_effort = effort;
    }
    if tool_mode != "absent" {
        request.tools.push(ToolSchema::function(
            "fixture_tool",
            "Offline fixture; never executed",
            serde_json::json!({"type":"object","properties":{}}),
        ));
        let choice = serde_json::json!(tool_mode);
        if from_defaults {
            defaults.tool_choice = Some(choice);
        } else {
            request.tool_choice = Some(choice);
        }
    }
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-audit-placeholder").with_defaults(defaults),
    );
    let outcome = if cancel_aware {
        client
            .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
            .await
    } else {
        client.stream_response(request).await
    };
    let requests = transport.requests.lock().unwrap();
    let wire = requests
        .first()
        .and_then(|request| request["top_p"].as_f64())
        .map(|value| value as f32);
    let correct_wire = outcome.is_ok() && requests.len() == 1 && wire == Some(expected);
    let unchanged = configured == expected;
    let violation = !correct_wire;
    (unchanged, violation.then(|| format!(
        "{model} effort={effort:?} tools={tool_mode} configured={configured} expected={expected} defaults={from_defaults} cancel={cancel_aware} wire={wire:?} calls={}",
        requests.len(),
    )))
}

async fn sampling_mode_violations(
    model: &str,
    effort: Option<ReasoningEffort>,
    tool_mode: &str,
) -> (usize, Vec<String>) {
    let mut controls = 0;
    let mut failures = Vec::new();
    for configured in [0.2_f32, 0.94, 0.95, 1.0] {
        for (from_defaults, cancel_aware) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let (unchanged, violation) = sampling_case_violation(
                model,
                effort,
                tool_mode,
                configured,
                from_defaults,
                cancel_aware,
            )
            .await;
            controls += usize::from(unchanged);
            failures.extend(violation);
        }
    }
    (controls, failures)
}

/// User-selected policy: valid settings normalize visibly rather than reject.
/// Every combination must dispatch exactly once with the effective value.
/// Preview, diagnostics and recovery are verified separately.
#[tokio::test]
async fn h166_sampling_matrix_never_dispatches_a_misleading_top_p() {
    let mut failures = Vec::new();
    let mut controls = 0;
    let mut modes = 0;
    for model in ["deepseek-flash", "deepseek-v4-pro"] {
        for effort in [
            None,
            Some(ReasoningEffort::None),
            Some(ReasoningEffort::Low),
            Some(ReasoningEffort::High),
            Some(ReasoningEffort::Max),
        ] {
            for tool_mode in ["absent", "auto", "none"] {
                let (mode_controls, mode_failures) =
                    sampling_mode_violations(model, effort, tool_mode).await;
                modes += 1;
                controls += mode_controls;
                failures.extend(mode_failures);
            }
        }
    }
    assert_eq!(modes, 30);
    assert_eq!(controls, 204);
    eprintln!(
        "H166 common-floor matrix: {} cases, {controls} unchanged controls, {} violations",
        modes * 16,
        failures.len(),
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn h166_final_tool_face_overrides_defaults_without_leaking_local_metadata() {
    let transport = Arc::new(AuditTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-placeholder").with_defaults(ResponseDefaults {
            top_p: Some(0.2),
            reasoning_effort: Some(ReasoningEffort::High),
            tool_choice: Some(serde_json::json!("none")),
            ..Default::default()
        }),
    );
    for (kind, name, choice, expected) in [
        ("function", "fixture", "auto", 0.95_f32),
        ("custom", "apply_patch", "auto", 0.95),
        ("web_search", "ignored", "auto", 0.95),
        ("file_search", "ignored", "auto", 1.0),
        // Existing apply_patch defaulting rewrites this face to custom. H156
        // owns whether that conversion is desirable; H166 must report the
        // face actually sent instead of contradicting its own wire metadata.
        ("file_search", "apply_patch", "auto", 0.95),
        ("function", "fixture", "none", 1.0),
    ] {
        let mut tool =
            ToolSchema::function(name, "not executed", serde_json::json!({"type":"object"}));
        tool.kind = kind.into();
        let request = ResponseRequest::new("deepseek-v4-pro", vec![Message::user("offline")])
            .with_top_p(0.96)
            .with_tools(vec![tool])
            .with_tool_choice(serde_json::json!(choice));
        let evidence = client.response_sampling(&request).unwrap();
        assert_eq!(evidence.configured_top_p, Some(0.96));
        assert_eq!(evidence.effective_top_p, Some(expected));
        let prepared = client.prepare_response_request(request.clone());
        assert_eq!(prepared.top_p, Some(expected));
        assert_eq!(client.prepare_response_request(prepared.clone()), prepared);
        client.stream_response(request).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        let wire = requests.last().unwrap();
        assert_eq!(wire["top_p"].as_f64().unwrap() as f32, expected);
        for local in ["sampling", "configured_top_p", "effective_top_p", "reason"] {
            assert!(wire.get(local).is_none());
        }
    }
    let control = ResponseRequest::new("deepseek-v4-pro", vec![Message::user("offline")])
        .with_reasoning_effort(ReasoningEffort::None);
    let metadata = client.response_sampling(&control).unwrap();
    assert_eq!(metadata.configured_top_p, Some(0.2));
    assert_eq!(metadata.wire_top_p, Some(0.2));
    client.stream_response(control).await.unwrap();
    assert_eq!(transport.requests.lock().unwrap().len(), 7);
}

#[tokio::test]
async fn h166_omission_and_unknown_model_remain_intact_and_cancel_has_zero_dispatch() {
    let transport = Arc::new(AuditTransport::default());
    let client = ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek("offline-placeholder"),
    );
    let omitted = ResponseRequest::new("deepseek-v4-pro", vec![Message::user("offline")]);
    assert_eq!(
        client.response_sampling(&omitted).unwrap().effective_top_p,
        Some(1.0)
    );
    client.stream_response(omitted.clone()).await.unwrap();
    assert!(transport.requests.lock().unwrap()[0].get("top_p").is_none());
    let unknown =
        ResponseRequest::new("custom-flash", vec![Message::user("offline")]).with_top_p(0.2);
    assert_eq!(
        client.response_sampling(&unknown).unwrap().effective_top_p,
        None
    );
    client.stream_response(unknown).await.unwrap();
    assert_eq!(transport.requests.lock().unwrap()[1]["top_p"], 0.2);
    assert!(client
        .stream_response_cancelled(omitted, Arc::new(AtomicBool::new(true)))
        .await
        .is_err());
    assert_eq!(transport.requests.lock().unwrap().len(), 2);
    let chat_client =
        ModelClient::new(transport, ModelConfig::deepseek_chat("offline-placeholder"));
    let request =
        ResponseRequest::new("deepseek-flash", vec![Message::user("offline")]).with_top_p(0.2);
    assert!(chat_client.response_sampling(&request).is_none());
    assert_eq!(
        chat_client.prepare_response_request(request).top_p,
        Some(0.2)
    );
}
