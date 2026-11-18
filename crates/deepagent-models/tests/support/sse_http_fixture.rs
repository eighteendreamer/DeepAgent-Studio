//! Finite, synthetic loopback HTTP fixture shared by model and host acceptance.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use deepagent_models::{ModelConfig, ReqwestTransport, TransportTimeouts, WireMode};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[path = "sse_protocol_fixture.rs"]
pub mod protocol;

pub struct Fixture {
    pub base_url: String,
    stop: Option<oneshot::Sender<()>>,
    server: JoinHandle<()>,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Fixture {
    pub async fn start(chunks: Vec<Vec<u8>>) -> Self {
        Self::start_timed(
            chunks
                .into_iter()
                .map(|chunk| (Duration::ZERO, chunk))
                .collect(),
            Duration::ZERO,
            200,
        )
        .await
    }

    pub async fn start_timed(
        chunks: Vec<(Duration, Vec<u8>)>,
        header_delay: Duration,
        status: u16,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (stop, mut stopped) = oneshot::channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                let request =
                    tokio::time::timeout(Duration::from_secs(5), read_request(&mut socket))
                        .await
                        .expect("fixture request deadline");
                received.lock().unwrap().push(request);
                if header_delay > Duration::ZERO {
                    tokio::time::sleep(header_delay).await;
                }
                let headers = format!("HTTP/1.1 {status} Synthetic\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
                if socket.write_all(headers.as_bytes()).await.is_err() {
                    continue;
                }
                for (delay, chunk) in &chunks {
                    if chunk.is_empty() {
                        continue;
                    }
                    if *delay > Duration::ZERO {
                        tokio::time::sleep(*delay).await;
                    }
                    let header = format!("{:X}\r\n", chunk.len());
                    if socket.write_all(header.as_bytes()).await.is_err()
                        || socket.write_all(chunk).await.is_err()
                        || socket.write_all(b"\r\n").await.is_err()
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let _ = socket.write_all(b"0\r\n\r\n").await;
                let _ = socket.shutdown().await;
            }
        });
        Self {
            base_url,
            stop: Some(stop),
            server,
            requests,
        }
    }

    pub fn config(&self, mode: WireMode) -> ModelConfig {
        let mut config = ModelConfig::deepseek("h30-synthetic").with_wire_mode(mode);
        config.base_url = self.base_url.clone();
        config.defaults.max_output_tokens = Some(32);
        config
    }

    pub fn transport() -> ReqwestTransport {
        ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap())
            .with_timeouts(TransportTimeouts {
                connect: Duration::from_secs(3),
                first_byte: Duration::from_secs(3),
                idle: Duration::from_secs(3),
                total: Duration::from_secs(10),
            })
    }

    pub async fn finish(mut self) -> Vec<Value> {
        let _ = self.stop.take().unwrap().send(());
        match tokio::time::timeout(Duration::from_secs(6), &mut self.server).await {
            Ok(joined) => joined.expect("fixture server failed"),
            Err(_) => {
                self.server.abort();
                let _ = (&mut self.server).await;
                panic!("fixture server did not stop");
            }
        }
        let requests = self.requests.lock().unwrap().clone();
        requests
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn read_request(socket: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut buffer = [0; 2048];
    loop {
        let size = socket.read(&mut buffer).await.unwrap();
        assert!(size > 0);
        bytes.extend_from_slice(&buffer[..size]);
        assert!(bytes.len() < 1024 * 1024);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
            assert!(
                headers.starts_with("POST /responses HTTP/1.1")
                    || headers.starts_with("POST /chat/completions HTTP/1.1")
                    || headers.starts_with("POST /v1/messages HTTP/1.1")
                    || headers.starts_with("GET /models HTTP/1.1")
            );
            if headers.starts_with("GET /models HTTP/1.1") {
                return Value::Null;
            }
            let size: usize = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .unwrap()
                .1
                .trim()
                .parse()
                .unwrap();
            if bytes.len() >= end + 4 + size {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + size]).unwrap();
            }
        }
    }
}

#[allow(dead_code)] // Compatibility helper for the AppCore Responses-only fixtures.
pub fn response_terminal() -> String {
    serde_json::json!({"type":"response.completed","sequence_number":0,"response":{
        "id":"h30-response","object":"response","created_at":1,"model":"deepseek-v4-flash",
        "status":"completed","output":[{"type":"message","id":"h30-output","role":"assistant",
            "content":[{"type":"output_text","text":"H30 complete 中文🙂","annotations":[]}]}]
    }})
    .to_string()
}
