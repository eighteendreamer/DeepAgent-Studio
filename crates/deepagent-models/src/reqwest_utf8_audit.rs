//! H-257: exercise the production byte decoder and real HTTP Responses path.

use super::*;
use crate::{DeltaObserver, ModelClient, ModelConfig, ModelStreamEvent, ResponseRequest};
use deepagent_core::message::Message;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

const TERMINAL: &[u8] = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"sequence_number\":3,\"response\":{\"id\":\"resp_audit\",\"object\":\"response\",\"status\":\"completed\",\"model\":\"deepseek-flash\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n";

async fn write_chunk(socket: &mut TcpStream, bytes: &[u8]) {
    socket
        .write_all(format!("{:X}\r\n", bytes.len()).as_bytes())
        .await
        .unwrap();
    socket.write_all(bytes).await.unwrap();
    socket.write_all(b"\r\n").await.unwrap();
}

fn delta(text: &str, sequence: u32) -> Vec<u8> {
    format!(
        "event: response.output_text.delta\ndata: {}\n\n",
        serde_json::json!({
            "type": "response.output_text.delta", "sequence_number": sequence,
            "item_id": "msg_audit", "output_index": 0, "content_index": 0,
            "delta": text,
        })
    )
    .into_bytes()
}

struct Observer {
    text: Arc<Mutex<String>>,
    first_seen: Option<oneshot::Sender<()>>,
}

impl DeltaObserver for Observer {
    fn on_event(&mut self, event: ModelStreamEvent) {
        if let ModelStreamEvent::ContentDelta { text } = event {
            self.text.lock().unwrap().push_str(&text);
            if let Some(sender) = self.first_seen.take() {
                let _ = sender.send(());
            }
        }
    }
}

struct Observation {
    settled_before_eof: bool,
    result: Result<crate::Response>,
    text: String,
}

async fn observe_http(invalid: bool) -> Observation {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (first_seen_tx, first_seen_rx) = oneshot::channel();
    let (sent_tx, sent_rx) = oneshot::channel();
    let (finish_tx, finish_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = request.windows(4).position(|p| p == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                assert!(headers.starts_with("POST /responses HTTP/1.1"));
                let length = headers
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .unwrap()
                    .1
                    .trim()
                    .parse::<usize>()
                    .unwrap();
                if request.len() >= header_end + 4 + length {
                    let body: serde_json::Value =
                        serde_json::from_slice(&request[header_end + 4..][..length]).unwrap();
                    assert_eq!(body["stream"], true);
                    assert_eq!(body["model"], "deepseek-flash");
                    break;
                }
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        write_chunk(&mut socket, &delta("before|", 1)).await;
        // A semantic observer acknowledgment proves the production reader is active.
        first_seen_rx.await.unwrap();
        if invalid {
            write_chunk(&mut socket, &[0xff]).await;
            // Finite synthetic comment tail, not an OOM or resource exhaustion test.
            let mut comment = vec![b'x'; 4094];
            comment.insert(0, b':');
            comment.push(b'\n');
            for _ in 0..16 {
                write_chunk(&mut socket, &comment).await;
            }
            write_chunk(&mut socket, &delta("after|", 2)).await;
        } else {
            let unicode = delta("中文🙂|", 2);
            let start = unicode
                .windows(3)
                .position(|p| p == "中".as_bytes())
                .unwrap();
            write_chunk(&mut socket, &unicode[..start + 1]).await;
            tokio::task::yield_now().await;
            write_chunk(&mut socket, &unicode[start + 1..start + 2]).await;
            tokio::task::yield_now().await;
            write_chunk(&mut socket, &unicode[start + 2..]).await;
        }
        sent_tx.send(()).unwrap();
        // Keep HTTP open without terminal; the caller observes early error handling.
        finish_rx.await.unwrap();
        // If a corrected transport already rejected the byte, writes may fail.
        let trailer = [
            format!("{:X}\r\n", TERMINAL.len()).as_bytes(),
            TERMINAL,
            b"\r\n0\r\n\r\n",
        ]
        .concat();
        let _ = socket.write_all(&trailer).await;
    });
    let transport =
        ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap())
            .with_timeouts(TransportTimeouts {
                connect: Duration::from_secs(2),
                first_byte: Duration::from_secs(2),
                idle: Duration::from_secs(2),
                total: Duration::from_secs(5),
            });
    let mut config = ModelConfig::deepseek("synthetic-audit-placeholder");
    config.base_url = format!("http://{address}");
    let client = ModelClient::new(Arc::new(transport), config);
    let text = Arc::new(Mutex::new(String::new()));
    let mut observer = Observer {
        text: text.clone(),
        first_seen: Some(first_seen_tx),
    };
    let mut operation = tokio::spawn(async move {
        client
            .stream_response_observed(
                ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic audit")]),
                &mut observer,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), sent_rx)
        .await
        .unwrap()
        .unwrap();
    let early = tokio::time::timeout(Duration::from_millis(150), &mut operation).await;
    let settled_before_eof = early.is_ok();
    finish_tx.send(()).unwrap();
    let result = match early {
        Ok(joined) => joined.unwrap(),
        Err(_) => tokio::time::timeout(Duration::from_secs(3), operation)
            .await
            .unwrap()
            .unwrap(),
    };
    server.await.unwrap();
    let text = text.lock().unwrap().clone();
    Observation {
        settled_before_eof,
        result,
        text,
    }
}

#[tokio::test]
async fn audit_valid_split_utf8_preserves_text_and_terminal() {
    for text in ["中", "🙂", "中文🙂"] {
        for split in 1..text.len() {
            let mut pending = text.as_bytes()[..split].to_vec();
            let (mut decoded, consumed) = decode_utf8_prefix(&pending);
            pending.drain(..consumed);
            assert!(pending.len() <= 3);
            pending.extend_from_slice(&text.as_bytes()[split..]);
            let (tail, consumed) = decode_utf8_prefix(&pending);
            decoded.push_str(&tail);
            pending.drain(..consumed);
            assert_eq!(decoded, text);
            assert!(pending.is_empty());
        }
    }
    let observation = observe_http(false).await;
    eprintln!(
        "valid UTF-8: early={} text={:?} result={:?}",
        observation.settled_before_eof, observation.text, observation.result
    );
    assert!(!observation.settled_before_eof);
    assert_eq!(observation.text, "before|中文🙂|");
    assert_eq!(
        observation.result.unwrap().output_text_projection(),
        "before|中文🙂|"
    );
}

#[tokio::test]
#[ignore = "H-257: malformed UTF-8 must fail without retaining subsequent stream bytes"]
async fn audit_invalid_utf8_is_rejected_before_eof_with_encoding_context() {
    // Replay exactly the production pending/decoder/drain sequence, without
    // assuming HTTP chunk boundaries are preserved by TCP or reqwest.
    let mut pending = vec![0xff];
    for _ in 0..16 {
        pending.extend_from_slice(&[b'x'; 4096]);
        let (text, consumed) = decode_utf8_prefix(&pending);
        assert!(text.is_empty());
        pending.drain(..consumed);
    }
    let observation = observe_http(true).await;
    let error = observation.result.unwrap_err().to_string();
    eprintln!(
        "invalid UTF-8: retained={} early={} text={:?} error={error}",
        pending.len(),
        observation.settled_before_eof,
        observation.text
    );
    assert!(
        observation.settled_before_eof && error.to_ascii_lowercase().contains("utf"),
        "invalid byte must produce an encoding error before EOF; retained={} early={} error={error}",
        pending.len(), observation.settled_before_eof,
    );
}
