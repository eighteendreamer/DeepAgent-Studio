//! H-257: exercise the production byte decoder and real HTTP Responses path.

use super::*;
use crate::ModelFailureKind;
use crate::{DeltaObserver, ModelClient, ModelConfig, ModelStreamEvent, ResponseRequest, WireMode};
use deepagent_core::message::Message;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

const TERMINAL: &[u8] = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"sequence_number\":3,\"response\":{\"id\":\"resp_audit\",\"object\":\"response\",\"created_at\":1,\"status\":\"completed\",\"model\":\"deepseek-flash\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n";

async fn write_chunk(socket: &mut TcpStream, bytes: &[u8]) -> bool {
    socket
        .write_all(format!("{:X}\r\n", bytes.len()).as_bytes())
        .await
        .is_ok()
        && socket.write_all(bytes).await.is_ok()
        && socket.write_all(b"\r\n").await.is_ok()
}

fn delta(text: &str, sequence: u32, mode: WireMode) -> Vec<u8> {
    if mode == WireMode::ChatCompletions {
        return format!(
            "data: {}\n\n",
            serde_json::json!({"choices":[{"index":0,"delta":{"content":text},"finish_reason":null}]})
        )
        .into_bytes();
    }
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

#[derive(Clone, Copy)]
enum InputFault {
    Valid,
    InvalidByte,
    IncompleteCodePoint,
    CancelIncomplete,
}

async fn observe_http(fault: InputFault, cancel_aware: bool, mode: WireMode) -> Observation {
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
                let endpoint = match mode {
                    WireMode::Responses => "/responses",
                    WireMode::ChatCompletions => "/chat/completions",
                };
                assert!(headers.starts_with(&format!("POST {endpoint} HTTP/1.1")));
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
        let first = delta("before|", 1, mode);
        assert!(write_chunk(&mut socket, &first).await);
        // A semantic observer acknowledgment proves the production reader is active.
        first_seen_rx.await.unwrap();
        match fault {
            InputFault::InvalidByte => {
                if !write_chunk(&mut socket, &[0xff]).await {
                    let _ = sent_tx.send(());
                    let _ = finish_rx.await;
                    return;
                }
                // Finite synthetic comment tail, not an OOM or resource exhaustion test.
                let mut comment = vec![b'x'; 4094];
                comment.insert(0, b':');
                comment.push(b'\n');
                for _ in 0..16 {
                    if !write_chunk(&mut socket, &comment).await {
                        break;
                    }
                }
                let _ = write_chunk(&mut socket, &delta("after|", 2, mode)).await;
            }
            InputFault::IncompleteCodePoint | InputFault::CancelIncomplete => {
                let _ = write_chunk(&mut socket, &[0xe2, 0x82]).await;
            }
            InputFault::Valid => {
                let unicode = delta("中文🙂|", 2, mode);
                let start = unicode
                    .windows(3)
                    .position(|p| p == "中".as_bytes())
                    .unwrap();
                assert!(write_chunk(&mut socket, &unicode[..start + 1]).await);
                tokio::task::yield_now().await;
                assert!(write_chunk(&mut socket, &unicode[start + 1..start + 2]).await);
                tokio::task::yield_now().await;
                assert!(write_chunk(&mut socket, &unicode[start + 2..]).await);
            }
        }
        let _ = sent_tx.send(());
        // Keep HTTP open without terminal; the caller observes early error handling.
        finish_rx.await.unwrap();
        // If a corrected transport already rejected the byte, writes may fail.
        if matches!(
            fault,
            InputFault::IncompleteCodePoint | InputFault::CancelIncomplete
        ) {
            let _ = socket.write_all(b"0\r\n\r\n").await;
        } else {
            let terminal = match mode {
                WireMode::Responses => TERMINAL,
                WireMode::ChatCompletions => b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".as_slice(),
            };
            let _ = write_chunk(&mut socket, terminal).await;
            let _ = socket.write_all(b"0\r\n\r\n").await;
        }
    });
    let transport =
        ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap())
            .with_timeouts(TransportTimeouts {
                connect: Duration::from_secs(2),
                first_byte: Duration::from_secs(2),
                idle: Duration::from_secs(2),
                total: Duration::from_secs(5),
            });
    let mut config = ModelConfig::deepseek("synthetic-audit-placeholder").with_wire_mode(mode);
    config.base_url = format!("http://{address}");
    let client = ModelClient::new(Arc::new(transport), config);
    let text = Arc::new(Mutex::new(String::new()));
    let mut observer = Observer {
        text: text.clone(),
        first_seen: Some(first_seen_tx),
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let operation_cancel = cancel.clone();
    let mut operation = tokio::spawn(async move {
        let request =
            ResponseRequest::new("deepseek-flash", vec![Message::user("synthetic audit")]);
        if cancel_aware {
            client
                .stream_response_observed_cancelled(request, &mut observer, operation_cancel)
                .await
        } else {
            client
                .stream_response_observed(request, &mut observer)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(3), sent_rx)
        .await
        .unwrap()
        .unwrap();
    if matches!(fault, InputFault::CancelIncomplete) {
        assert!(cancel_aware);
        cancel.store(true, Ordering::Release);
    }
    let early = tokio::time::timeout(Duration::from_millis(500), &mut operation).await;
    let settled_before_eof = early.is_ok();
    finish_tx.send(()).unwrap();
    let result = match early {
        Ok(joined) => joined.unwrap(),
        Err(_) => match tokio::time::timeout(Duration::from_secs(3), &mut operation).await {
            Ok(joined) => joined.unwrap(),
            Err(_) => {
                operation.abort();
                let _ = operation.await;
                server.abort();
                let _ = server.await;
                panic!("HTTP operation failed to settle after fixture release");
            }
        },
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
    for text in ["é", "中", "🙂", "é中文🙂"] {
        for split in 1..text.len() {
            let mut pending = text.as_bytes()[..split].to_vec();
            let (mut decoded, consumed) =
                decode_utf8_prefix(&pending).expect("a valid UTF-8 prefix is not malformed");
            pending.drain(..consumed);
            assert!(pending.len() <= 3);
            pending.extend_from_slice(&text.as_bytes()[split..]);
            let (tail, consumed) =
                decode_utf8_prefix(&pending).expect("completed UTF-8 code point must decode");
            decoded.push_str(&tail);
            pending.drain(..consumed);
            assert_eq!(decoded, text);
            assert!(pending.is_empty());
        }
    }
    let observation = observe_http(InputFault::Valid, false, WireMode::Responses).await;
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

#[test]
fn utf8_decoder_distinguishes_invalid_sequences_from_incomplete_suffixes() {
    for malformed in [
        vec![0xff],
        vec![0x80],
        vec![0xc0, 0xaf],
        vec![0xed, 0xa0, 0x80],
        vec![0xf4, 0x90, 0x80, 0x80],
        vec![0xe2, 0x28, 0xa1],
        vec![0xf0, 0x9f, 0x99, 0x28],
    ] {
        for offset in 0..=malformed.len() {
            let mut bytes = "before|".as_bytes()[..offset.min("before|".len())].to_vec();
            bytes.extend_from_slice(&malformed);
            let error = decode_utf8_prefix(&bytes).unwrap_err();
            assert_eq!(error.valid_up_to, offset.min("before|".len()));
            assert!((1..=4).contains(&error.error_len));
            assert_eq!(
                &error.valid_prefix,
                &"before|"[..offset.min("before|".len())]
            );
        }
    }

    let mut bytes = b"before|a".to_vec();
    bytes.push(0x80);
    let error = decode_utf8_prefix(&bytes).unwrap_err();
    assert_eq!(error.valid_up_to, "before|a".len());
    assert_eq!(error.valid_prefix, "before|a");
    assert_eq!(error.error_len, 1);

    for incomplete in [
        vec![0xc2],
        vec![0xe2],
        vec![0xe2, 0x82],
        vec![0xf0],
        vec![0xf0, 0x9f],
        vec![0xf0, 0x9f, 0x99],
    ] {
        let mut bytes = b"before|".to_vec();
        bytes.extend_from_slice(&incomplete);
        let (prefix, consumed) = decode_utf8_prefix(&bytes).unwrap();
        assert_eq!(prefix, "before|");
        assert_eq!(consumed, "before|".len());
        assert_eq!(bytes[consumed..], incomplete);
        assert!(bytes.len() - consumed <= 3);
    }
}

#[tokio::test]
async fn audit_invalid_utf8_is_rejected_before_eof_with_encoding_context() {
    let observation = observe_http(InputFault::InvalidByte, false, WireMode::Responses).await;
    let core_error = observation.result.unwrap_err();
    let error = core_error.to_string();
    eprintln!(
        "invalid UTF-8: early={} text={:?} error={error}",
        observation.settled_before_eof, observation.text
    );
    assert!(
        observation.settled_before_eof
            && error.to_ascii_lowercase().contains("invalid utf-8")
            && !error.contains("terminal response"),
        "invalid byte must produce an encoding error before EOF; early={} error={error}",
        observation.settled_before_eof,
    );
    assert!(matches!(core_error, CoreError::Serialization(_)));
    assert_eq!(
        crate::classify_model_error(&core_error),
        ModelFailureKind::InvalidRequest,
        "malformed provider bytes must not enter retry/fallback policy"
    );
    assert!(!ModelFailureKind::InvalidRequest.retryable());

    let cancelled = observe_http(InputFault::InvalidByte, true, WireMode::Responses).await;
    let cancelled_error = cancelled.result.unwrap_err();
    let error = cancelled_error.to_string();
    assert!(cancelled.settled_before_eof && error.to_ascii_lowercase().contains("invalid utf-8"));
    assert!(matches!(cancelled_error, CoreError::Serialization(_)));
}

#[tokio::test]
async fn incomplete_utf8_at_eof_is_reported_as_truncation_not_missing_terminal() {
    let observation =
        observe_http(InputFault::IncompleteCodePoint, false, WireMode::Responses).await;
    let core_error = observation.result.unwrap_err();
    let error = core_error.to_string();
    eprintln!(
        "incomplete UTF-8 at EOF: early={} text={:?} error={error}",
        observation.settled_before_eof, observation.text
    );
    assert!(!observation.settled_before_eof);
    assert!(
        error.to_ascii_lowercase().contains("incomplete utf-8"),
        "{error}"
    );
    assert!(!error.contains("terminal response"), "{error}");
    assert!(matches!(core_error, CoreError::Serialization(_)));
    assert_eq!(observation.text, "before|");
}

#[tokio::test]
async fn incomplete_utf8_wait_remains_actually_cancellable_in_both_protocols() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let observation = observe_http(InputFault::CancelIncomplete, true, mode).await;
        let error = observation.result.unwrap_err();
        assert!(observation.settled_before_eof, "{mode:?}: {error}");
        assert!(error.to_string().contains("request cancelled"), "{error}");
        assert_eq!(
            crate::classify_model_error(&error),
            ModelFailureKind::Cancelled
        );
        assert_eq!(observation.text, "before|");
    }
}
