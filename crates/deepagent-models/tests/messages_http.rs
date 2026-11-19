#![cfg(feature = "http")]
//! Inspect real HTTP bytes, including discovery, cancellation and redirects.
use std::{
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

use deepagent_models::{
    ModelClient, ModelConfig, ModelDiscovery, ReqwestTransport, ResponseRequest, WireMode,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[path = "support/sse_protocol_fixture.rs"]
mod wire;

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0);
        bytes.extend_from_slice(&chunk[..read]);
        assert!(bytes.len() < 1024 * 1024);
        if let Some(end) = bytes.windows(4).position(|chunk| chunk == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
            let size: usize = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse().unwrap())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + size {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

fn response_body() -> String {
    wire::start(WireMode::Messages)
        .into_iter()
        .chain(wire::finish(WireMode::Messages, "ok", 0))
        .map(|event| format!("data: {event}\n\n"))
        .collect()
}

#[tokio::test]
async fn messages_inference_and_discovery_send_native_headers_and_keep_bearer_compatibility() {
    for cancel_aware in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut received = Vec::new();
            for body in [
                r#"{"data":[{"id":"deepseek-flash"}]}"#.to_owned(),
                response_body(),
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                received.push(read_request(&mut socket).await);
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
            received
        });
        let transport = Arc::new(ReqwestTransport::new());
        let discovered = ModelDiscovery::with_base_url(transport.clone(), &base)
            .with_wire_mode(WireMode::Messages)
            .list_models("synthetic-key")
            .await
            .unwrap();
        assert_eq!(discovered[0].id, "deepseek-flash");
        let mut config = ModelConfig::deepseek("synthetic-key").with_wire_mode(WireMode::Messages);
        config.base_url = base;
        let client = ModelClient::new(transport, config);
        let mut request =
            ResponseRequest::with_instructions_and_user_input("deepseek-flash", "rules", "hello")
                .with_max_output_tokens(128);
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let encoded =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png.into_inner());
        request.input = vec![deepagent_models::ResponseItem::InputMessage {
            role: "user".into(),
            content: "hello".into(),
            attachments: vec![deepagent_core::message::MessageAttachment {
                id: "native-http-image".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
                sha256: None,
            }],
        }];
        request.attachment_data_urls.insert(
            "native-http-image".into(),
            format!("data:image/png;base64,{encoded}"),
        );
        if cancel_aware {
            client
                .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
                .await
        } else {
            client.stream_response(request).await
        }
        .unwrap();
        let received = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert!(received[0].starts_with("GET /v1/models HTTP/1.1"));
        assert!(received[1].starts_with("POST /v1/messages HTTP/1.1"));
        let body: serde_json::Value =
            serde_json::from_str(received[1].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["messages"][0]["content"][1]["source"]["data"], encoded);
        for raw in received {
            let headers = raw.split("\r\n\r\n").next().unwrap().to_ascii_lowercase();
            assert!(
                headers.contains("x-api-key: synthetic-key\r\n"),
                "native API key header missing"
            );
            assert!(
                headers.contains("anthropic-version: 2023-06-01\r\n"),
                "version header missing"
            );
            assert!(
                headers.contains("authorization: bearer synthetic-key\r\n"),
                "gateway compatibility header missing"
            );
        }
    }
}

#[tokio::test]
async fn messages_credentials_and_body_never_follow_a_cross_origin_redirect() {
    for inference in [false, true] {
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", origin.local_addr().unwrap());
        let location = format!("http://{}/capture", target.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = origin.accept().await.unwrap();
            let raw = read_request(&mut socket).await;
            socket.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            raw
        });
        let transport = Arc::new(ReqwestTransport::new());
        let operation = async {
            if inference {
                let mut config =
                    ModelConfig::deepseek("synthetic-key").with_wire_mode(WireMode::Messages);
                config.base_url = base;
                ModelClient::new(transport, config)
                    .stream_response(
                        ResponseRequest::with_instructions_and_user_input(
                            "deepseek-flash",
                            "private synthetic prompt",
                            "hello",
                        )
                        .with_max_output_tokens(128),
                    )
                    .await
                    .map(|_| ())
            } else {
                ModelDiscovery::with_base_url(transport, base)
                    .with_wire_mode(WireMode::Messages)
                    .list_models("synthetic-key")
                    .await
                    .map(|_| ())
            }
        };
        assert!(tokio::time::timeout(Duration::from_secs(3), operation)
            .await
            .unwrap()
            .is_err());
        assert!(server
            .await
            .unwrap()
            .to_ascii_lowercase()
            .contains("x-api-key: synthetic-key"));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), target.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn a_custom_client_must_prove_its_redirect_policy_before_native_credentials_are_sent() {
    use deepagent_models::{HttpTransport, TransportAuth, TransportRequest};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let transport =
        ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap());
    let request = TransportRequest {
        url: url.clone(),
        api_key: "PRIVATE_SYNTHETIC_KEY".into(),
        auth: TransportAuth::Messages,
        body: "PRIVATE_SYNTHETIC_BODY".into(),
    };
    let debug = format!("{request:?}");
    assert!(!debug.contains("PRIVATE_SYNTHETIC"));
    let error = transport
        .stream(request, &mut |_: &str| Ok(false))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("with_client_builder"));
    assert!(transport
        .get_json_authenticated(&url, "PRIVATE_SYNTHETIC_KEY", TransportAuth::Messages)
        .await
        .is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn same_origin_redirects_preserve_native_headers_and_errors_redact_the_key() {
    for inference in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /v1/target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            let (mut socket, _) = listener.accept().await.unwrap();
            let raw = read_request(&mut socket).await;
            let body = r#"{"error":{"type":"authentication_error","message":"rejected x-api-key PRIVATE_SYNTHETIC_KEY"}}"#;
            socket.write_all(format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            raw
        });
        let transport = Arc::new(
            ReqwestTransport::with_client_builder(reqwest::Client::builder().no_proxy()).unwrap(),
        );
        let result = if inference {
            let mut config =
                ModelConfig::deepseek("PRIVATE_SYNTHETIC_KEY").with_wire_mode(WireMode::Messages);
            config.base_url = base;
            ModelClient::new(transport, config)
                .stream_response(
                    ResponseRequest::with_instructions_and_user_input(
                        "deepseek-flash",
                        "rules",
                        "hello",
                    )
                    .with_max_output_tokens(128),
                )
                .await
                .map(|_| ())
        } else {
            ModelDiscovery::with_base_url(transport, base)
                .with_wire_mode(WireMode::Messages)
                .list_models("PRIVATE_SYNTHETIC_KEY")
                .await
                .map(|_| ())
        };
        let error = result.unwrap_err().to_string();
        assert!(
            !error.contains("PRIVATE_SYNTHETIC_KEY"),
            "credential echoed in provider error"
        );
        assert!(error.contains("authentication_error"));
        let raw = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap()
            .to_ascii_lowercase();
        assert!(raw.contains("x-api-key: private_synthetic_key"));
        assert!(raw.contains("anthropic-version: 2023-06-01"));
    }
}
