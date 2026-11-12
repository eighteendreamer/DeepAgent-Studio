use std::sync::{atomic::AtomicBool, Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{
    error::{CoreError, Result},
    message::Message,
};
use deepagent_models::{
    transport::EventSink, ChatCompletionRequest, HttpTransport, ModelClient, ModelConfig,
    ReasoningEffort, Response, ResponseRequest, ToolSchema, TransportRequest,
};
use serde_json::{json, Value};

fn tools() -> Vec<ToolSchema> {
    ["read_file", "other_file"]
        .into_iter()
        .map(|name| ToolSchema::function(name, "fixture", json!({"type":"object","properties":{}})))
        .collect()
}

fn chat_choice(name: &str) -> Value {
    json!({"type":"function","function":{"name":name}})
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        assert!(request.url.ends_with("/chat/completions"));
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        for event in [
            r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

async fn send(
    client: &ModelClient,
    choice: Option<Value>,
    native: bool,
    entry: usize,
) -> Result<Response> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut observer = deepagent_models::NoopObserver;
    if native {
        let mut request =
            ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
                .with_tools(tools())
                .with_reasoning_effort(ReasoningEffort::None);
        request.tool_choice = choice;
        match entry {
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
        }
    } else {
        let mut request = ResponseRequest::new("deepseek-flash", vec![Message::user("fixture")])
            .with_tools(tools())
            .with_reasoning_effort(ReasoningEffort::None);
        request.tool_choice = choice;
        match entry {
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
        }
    }
}

fn config(native: bool) -> ModelConfig {
    // Direct Chat must also work independently of the client's default wire.
    if native {
        ModelConfig::deepseek("synthetic")
    } else {
        ModelConfig::deepseek_chat("synthetic")
    }
}

#[tokio::test]
async fn chat_choice_request_and_defaults_use_native_shape_on_every_entry() {
    for native in [false, true] {
        for entry in 0..4 {
            for defaults in [false, true] {
                for (canonical, expected) in [
                    (None, None),
                    (Some(json!("auto")), Some(json!("auto"))),
                    (Some(json!("none")), Some(json!("none"))),
                    (Some(json!("required")), Some(json!("required"))),
                    (
                        Some(json!({"type":"function","name":"read_file"})),
                        Some(chat_choice("read_file")),
                    ),
                ] {
                    let transport = Arc::new(Recorder::default());
                    let mut cfg = config(native);
                    let choice = if defaults {
                        cfg.defaults.tool_choice = canonical;
                        None
                    } else if native {
                        expected.clone()
                    } else {
                        canonical
                    };
                    let client = ModelClient::new(transport.clone(), cfg);
                    assert_eq!(
                        send(&client, choice, native, entry)
                            .await
                            .unwrap()
                            .output_text_projection(),
                        "ok"
                    );
                    let wire = transport.0.lock().unwrap();
                    assert_eq!(wire.len(), 1);
                    assert_eq!(wire[0].get("tool_choice"), expected.as_ref());
                    assert_eq!(wire[0]["thinking"]["type"], "disabled");
                }
            }
        }
    }
}

#[tokio::test]
async fn chat_choice_explicit_value_overrides_default_without_mutation_or_double_conversion() {
    for native in [false, true] {
        for default in [
            json!({"type":"function","name":"other_file"}),
            json!({"type":"custom","name":"private-value"}),
        ] {
            let transport = Arc::new(Recorder::default());
            let mut cfg = config(native);
            cfg.defaults.tool_choice = Some(default.clone());
            let client = ModelClient::new(transport.clone(), cfg);
            for entry in 0..4 {
                let choice = if native {
                    chat_choice("read_file")
                } else {
                    json!({"type":"function","name":"read_file"})
                };
                send(&client, Some(choice.clone()), native, entry)
                    .await
                    .unwrap();
            }
            assert!(transport
                .0
                .lock()
                .unwrap()
                .iter()
                .all(|wire| wire["tool_choice"] == chat_choice("read_file")));
            // A subsequent request must still read the original default,
            // proving the earlier overrides did not mutate/cache its wire form.
            let next = send(&client, None, native, 0).await;
            if default["type"] == "function" {
                next.unwrap();
                assert_eq!(
                    transport.0.lock().unwrap()[4]["tool_choice"],
                    chat_choice("other_file")
                );
            } else {
                assert!(matches!(next.unwrap_err(), CoreError::Invalid(_)));
                assert_eq!(transport.0.lock().unwrap().len(), 4);
            }
        }
    }
}

fn invalid_common() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!([]),
        json!(7),
        json!("unsupported"),
        json!({}),
        json!({"type":"custom","name":"private-value"}),
        json!({"type":"web_search"}),
        json!({"type":"function"}),
    ]
}

#[tokio::test]
async fn invalid_choices_are_rejected_before_transport_for_defaults_and_each_protocol_input() {
    for native in [false, true] {
        for defaults in [false, true] {
            let mut choices = invalid_common();
            if defaults || !native {
                choices.extend([
                    chat_choice("read_file"),
                    json!({"type":"function","name":"private/value"}),
                    json!({"type":"function","name":"read_file","extra":true}),
                ]);
            } else {
                choices.extend([
                    json!({"type":"function","name":"read_file"}),
                    chat_choice("private/value"),
                    json!({"type":"function","function":{"name":"read_file","extra":true}}),
                ]);
            }
            for choice in choices {
                for entry in 0..4 {
                    let transport = Arc::new(Recorder::default());
                    let mut cfg = config(native);
                    let explicit = if defaults {
                        cfg.defaults.tool_choice = Some(choice.clone());
                        None
                    } else {
                        Some(choice.clone())
                    };
                    let client = ModelClient::new(transport.clone(), cfg);
                    let error = send(&client, explicit, native, entry).await.unwrap_err();
                    assert!(matches!(error, CoreError::Invalid(_)));
                    assert!(!error.to_string().contains("private"));
                    assert!(transport.0.lock().unwrap().is_empty());
                }
            }
        }
    }
}

#[test]
fn direct_chat_serde_accepts_only_native_supported_shapes() {
    let mut bad = invalid_common();
    bad.extend([
        json!({"type":"function","name":"read_file"}),
        chat_choice(""),
        chat_choice(&"x".repeat(129)),
        chat_choice("private/value"),
        json!({"type":"function","function":{"name":"read_file"},"extra":true}),
    ]);
    for choice in bad {
        let request = ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
            .with_tools(tools())
            .with_reasoning_effort(ReasoningEffort::None)
            .with_tool_choice(choice);
        assert!(request.validate().is_err());
        assert!(serde_json::to_value(request).is_err());
    }
    for choice in [
        chat_choice("read_file"),
        chat_choice(&"x".repeat(128)),
        json!("auto"),
        json!("none"),
        json!("required"),
    ] {
        let advertised = choice["function"]["name"]
            .as_str()
            .map(|name| {
                vec![ToolSchema::function(
                    name,
                    "fixture",
                    json!({"type":"object","properties":{}}),
                )]
            })
            .unwrap_or_else(tools);
        let request = ChatCompletionRequest::new("deepseek-flash", vec![Message::user("fixture")])
            .with_tools(advertised)
            .with_reasoning_effort(ReasoningEffort::None)
            .with_tool_choice(choice.clone());
        request.validate().unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap()["tool_choice"],
            choice
        );
    }
}

#[cfg(feature = "http")]
#[tokio::test]
async fn real_chat_http_roundtrip_uses_mapped_request_and_default_choices() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        time::{timeout, Duration},
    };

    for native in [false, true] {
        for defaults in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut cfg = config(native);
            cfg.base_url = format!("http://{}", listener.local_addr().unwrap());
            let canonical = json!({"type":"function","name":"read_file"});
            let explicit = if defaults {
                cfg.defaults.tool_choice = Some(canonical);
                None
            } else if native {
                Some(chat_choice("read_file"))
            } else {
                Some(canonical)
            };
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
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
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
                assert_eq!(body["tool_choice"], chat_choice("read_file"));
                assert_eq!(body["thinking"]["type"], "disabled");
                let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}", sse.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let transport = deepagent_models::ReqwestTransport::with_client(
                reqwest::Client::builder().no_proxy().build().unwrap(),
            );
            let client = ModelClient::new(Arc::new(transport), cfg);
            assert_eq!(
                timeout(Duration::from_secs(5), send(&client, explicit, native, 0))
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
}
