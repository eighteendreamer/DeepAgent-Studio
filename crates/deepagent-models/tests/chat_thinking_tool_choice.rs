use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{
    error::{CoreError, Result},
    message::Message,
};
use deepagent_models::{
    transport::EventSink, ChatCompletionRequest, HttpTransport, ModelClient, ModelConfig,
    ModelFailureKind, NoopObserver, ReasoningEffort, Response, ResponseRequest, ThinkingToggle,
    ToolSchema, TransportRequest,
};
use serde_json::{json, Value};

fn tools() -> Vec<ToolSchema> {
    vec![ToolSchema::function(
        "lookup",
        "fixture",
        json!({"type":"object","properties":{}}),
    )]
}

fn named(native: bool) -> Value {
    if native {
        json!({"type":"function","function":{"name":"lookup"}})
    } else {
        json!({"type":"function","name":"lookup"})
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        if request.url.ends_with("/chat/completions") {
            sink.on_event(r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#)?;
            sink.on_event("[DONE]")?;
        } else {
            sink.on_event(r#"{"type":"response.completed","sequence_number":0,"response":{"id":"fixture","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
        }
        Ok(())
    }
}

enum Request {
    Chat(ChatCompletionRequest),
    Responses(ResponseRequest),
}

fn request(native: bool, effort: Option<ReasoningEffort>, choice: Option<Value>) -> Request {
    if native {
        let mut request =
            ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
                .with_tools(tools());
        if let Some(effort) = effort {
            request = request.with_reasoning_effort(effort);
        }
        request.tool_choice = choice;
        Request::Chat(request)
    } else {
        let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")])
            .with_tools(tools());
        request.reasoning_effort = effort;
        request.tool_choice = choice;
        Request::Responses(request)
    }
}

async fn send(client: &ModelClient, request: Request, entry: usize) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut observer = NoopObserver;
    match request {
        Request::Chat(request) => match entry {
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
        },
        Request::Responses(request) => match entry {
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
        },
    }
}

fn invalid(error: &CoreError) -> bool {
    let kind = deepagent_models::classify_model_error(error);
    matches!(error, CoreError::Invalid(_))
        && kind == ModelFailureKind::InvalidRequest
        && !kind.retryable()
        && !error.to_string().contains("lookup")
}

#[tokio::test]
async fn final_chat_mode_and_choice_matrix_uses_defaults_overrides_and_all_entries() {
    let mut violations = Vec::new();
    for native in [false, true] {
        for entry in 0..4 {
            for (label, explicit, default, enabled) in [
                ("provider-default", None, None, true),
                ("request-low", Some(ReasoningEffort::Low), None, true),
                ("request-high", Some(ReasoningEffort::High), None, true),
                ("request-max", Some(ReasoningEffort::Max), None, true),
                ("request-none", Some(ReasoningEffort::None), None, false),
                ("default-high", None, Some(ReasoningEffort::High), true),
                ("default-none", None, Some(ReasoningEffort::None), false),
                (
                    "override-off",
                    Some(ReasoningEffort::None),
                    Some(ReasoningEffort::High),
                    false,
                ),
                (
                    "override-on",
                    Some(ReasoningEffort::High),
                    Some(ReasoningEffort::None),
                    true,
                ),
            ] {
                for from_defaults in [false, true] {
                    for (choice, forced) in [
                        (None, false),
                        (Some(json!("auto")), false),
                        (Some(json!("none")), false),
                        (Some(json!("required")), true),
                        (Some(named(native && !from_defaults)), true),
                    ] {
                        let transport = Arc::new(Recorder::default());
                        let mut cfg = ModelConfig::deepseek_chat("synthetic");
                        cfg.defaults.reasoning_effort = default;
                        let requested = if from_defaults {
                            cfg.defaults.tool_choice = choice;
                            None
                        } else {
                            choice
                        };
                        let client = ModelClient::new(transport.clone(), cfg);
                        let result =
                            send(&client, request(native, explicit, requested), entry).await;
                        let wire = transport.0.lock().unwrap();
                        let correct = if enabled && forced {
                            result.as_ref().err().is_some_and(invalid) && wire.is_empty()
                        } else {
                            result.is_ok() && wire.len() == 1
                        };
                        if !correct {
                            violations.push((
                                native,
                                entry,
                                label,
                                from_defaults,
                                enabled,
                                forced,
                                result.is_ok(),
                                wire.len(),
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Chat mode/choice violations: {violations:?}"
    );
}

#[test]
fn chat_direct_serde_and_validation_share_effective_mode_gate() {
    let mut violations = Vec::new();
    for (label, toggle, effort, enabled) in [
        ("default", None, None, true),
        ("toggle-on", Some("enabled"), None, true),
        ("toggle-off", Some("disabled"), None, false),
        ("raw-effort", None, Some(ReasoningEffort::High), true),
        (
            "canonical-off",
            Some("disabled"),
            Some(ReasoningEffort::None),
            false,
        ),
    ] {
        for (choice, forced) in [
            (None, false),
            (Some(json!("auto")), false),
            (Some(json!("none")), false),
            (Some(json!("required")), true),
            (Some(named(true)), true),
        ] {
            let mut request =
                ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
                    .with_tools(tools());
            request.thinking = toggle.map(|kind| ThinkingToggle { kind: kind.into() });
            request.reasoning_effort = effort;
            request.tool_choice = choice;
            let expected = !(enabled && forced);
            if request.validate().is_ok() != expected
                || serde_json::to_value(request).is_ok() != expected
            {
                violations.push((label, forced));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "direct Chat gate violations: {violations:?}"
    );
}

#[tokio::test]
async fn explicit_chat_toggle_wins_over_unused_defaults_and_responses_does_not_inherit_chat_rules()
{
    for enabled in [false, true] {
        for entry in 0..4 {
            let transport = Arc::new(Recorder::default());
            let mut cfg = ModelConfig::deepseek("synthetic");
            cfg.defaults.reasoning_effort = Some(if enabled {
                ReasoningEffort::None
            } else {
                ReasoningEffort::High
            });
            let client = ModelClient::new(transport.clone(), cfg);
            let request =
                ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
                    .with_tools(tools())
                    .with_thinking(enabled)
                    .with_tool_choice(named(true));
            let result = send(&client, Request::Chat(request), entry).await;
            if enabled {
                assert!(invalid(&result.unwrap_err()));
                assert!(transport.0.lock().unwrap().is_empty());
            } else {
                result.unwrap();
                assert_eq!(
                    transport.0.lock().unwrap()[0]["thinking"]["type"],
                    "disabled"
                );
            }
        }
    }
    for entry in 0..4 {
        for choice in [json!("required"), named(false)] {
            let transport = Arc::new(Recorder::default());
            let client = ModelClient::new(transport.clone(), ModelConfig::deepseek("synthetic"));
            send(
                &client,
                request(false, Some(ReasoningEffort::High), Some(choice.clone())),
                entry,
            )
            .await
            .unwrap();
            assert_eq!(transport.0.lock().unwrap()[0]["tool_choice"], choice);
            assert_eq!(
                transport.0.lock().unwrap()[0]["reasoning"]["effort"],
                "high"
            );
        }
    }
}

#[cfg(feature = "http")]
#[tokio::test]
async fn actual_http_transport_has_zero_connections_for_rejected_thinking_and_accepts_disabled_control(
) {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{timeout, Duration},
    };

    for native in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = ModelConfig::deepseek_chat("synthetic");
        cfg.base_url = format!("http://{}", listener.local_addr().unwrap());
        cfg.defaults.reasoning_effort = Some(ReasoningEffort::High);
        let transport = deepagent_models::ReqwestTransport::with_client(
            reqwest::Client::builder().no_proxy().build().unwrap(),
        );
        let client = ModelClient::new(Arc::new(transport), cfg);
        for choice in [json!("required"), named(native)] {
            let result = send(&client, request(native, None, Some(choice)), 0).await;
            assert!(invalid(&result.unwrap_err()));
        }
        assert!(timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut chunk = [0; 1024];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                assert!(bytes.len() < 64 * 1024);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                    assert!(headers.starts_with("POST /chat/completions HTTP/1.1"));
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
            while bytes.len() < header_end + length {
                let mut chunk = [0; 1024];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                assert!(bytes.len() < 64 * 1024);
            }
            let body: Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            assert_eq!(body["thinking"]["type"], "disabled");
            assert_eq!(body["tool_choice"], named(true));
            let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}", sse.len()).as_bytes()).await.unwrap();
        });
        let request = request(native, Some(ReasoningEffort::None), Some(named(native)));
        assert_eq!(
            timeout(Duration::from_secs(5), send(&client, request, 0))
                .await
                .unwrap()
                .unwrap()
                .output_text_projection(),
            "ok"
        );
        timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}
