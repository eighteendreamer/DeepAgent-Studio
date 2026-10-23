//! Real HTTP transport backed by `reqwest` (only compiled with `--features http`).
//!
//! Streams the response body in chunks, decodes UTF-8 incrementally, and runs
//! each line through the [`SseParser`] so partial network frames never drop
//! data.

use async_trait::async_trait;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};

use crate::sse::SseParser;
use crate::transport::{EventSink, HttpTransport, TransportRequest};

/// `reqwest`-backed transport with connection reuse.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    timeouts: TransportTimeouts,
}

#[derive(Debug, Clone, Copy)]
pub struct TransportTimeouts {
    pub connect: Duration,
    pub first_byte: Duration,
    pub idle: Duration,
    pub total: Duration,
}

const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_GET_BODY_BYTES: usize = 16 * 1024 * 1024;
const GET_TOTAL_TIMEOUT_CAP: Duration = Duration::from_secs(30);

impl Default for TransportTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            first_byte: Duration::from_secs(60),
            idle: Duration::from_secs(45),
            total: Duration::from_secs(300),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestTransport {
    /// Build a transport with a fresh client.
    pub fn new() -> Self {
        let timeouts = TransportTimeouts::default();
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(timeouts.connect)
                .build()
                .expect("reqwest client with valid timeout configuration"),
            timeouts,
        }
    }

    /// Build from an existing client (to share pools / config).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
            timeouts: TransportTimeouts::default(),
        }
    }

    pub fn with_timeouts(mut self, timeouts: TransportTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn get_json(&self, url: &str, api_key: &str) -> Result<String> {
        tokio::time::timeout(
            self.timeouts.total.min(GET_TOTAL_TIMEOUT_CAP),
            self.get_json_inner(url, api_key, None),
        )
        .await
        .map_err(|_| CoreError::other("GET total deadline exceeded"))?
    }

    async fn get_json_cancelled(
        &self,
        url: &str,
        api_key: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<String> {
        tokio::time::timeout(
            self.timeouts.total.min(GET_TOTAL_TIMEOUT_CAP),
            self.get_json_inner(url, api_key, Some(cancel)),
        )
        .await
        .map_err(|_| CoreError::other("GET total deadline exceeded"))?
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        tokio::time::timeout(self.timeouts.total, self.stream_inner(request, sink, None))
            .await
            .map_err(|_| CoreError::other("model stream total deadline exceeded"))?
    }

    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        tokio::time::timeout(
            self.timeouts.total,
            self.stream_inner(request, sink, Some(cancel)),
        )
        .await
        .map_err(|_| CoreError::other("model stream total deadline exceeded"))?
    }
}

impl ReqwestTransport {
    async fn get_json_inner(
        &self,
        url: &str,
        api_key: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<String> {
        if is_cancelled(cancel.as_ref()) {
            return Err(CoreError::other("request cancelled"));
        }

        let send = self
            .client
            .get(url)
            .bearer_auth(api_key)
            .header("accept", "application/json")
            .send();
        let response = self
            .await_cancelable_timeout(
                send,
                cancel.as_ref(),
                self.timeouts.first_byte,
                "GET first-byte timeout",
            )
            .await
            .map_err(|error| CoreError::other(format!("GET {url} failed: {error}")))?
            .map_err(|error| CoreError::other(format!("GET {url} failed: {error}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let (retry_after_ms, request_id) = provider_response_metadata(&response);
            let detail = self
                .read_provider_error_body(response, cancel.as_ref())
                .await?;
            return Err(provider_error_with_metadata(
                status.as_u16(),
                &detail,
                retry_after_ms,
                request_id,
            ));
        }

        self.read_get_body(response, cancel.as_ref()).await
    }

    async fn stream_inner(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        if is_cancelled(cancel.as_ref()) {
            return Err(CoreError::other("request cancelled"));
        }

        let send = self
            .client
            .post(&request.url)
            .bearer_auth(&request.api_key)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(request.body)
            .send();

        let response = self
            .await_cancelable_timeout(
                send,
                cancel.as_ref(),
                self.timeouts.first_byte,
                "model first-byte timeout",
            )
            .await
            .map_err(|e| CoreError::other(format!("request failed: {e}")))?
            .map_err(|e| CoreError::other(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let (retry_after_ms, request_id) = provider_response_metadata(&response);
            let detail = self
                .read_provider_error_body(response, cancel.as_ref())
                .await?;
            return Err(provider_error_with_metadata(
                status.as_u16(),
                &detail,
                retry_after_ms,
                request_id,
            ));
        }

        let mut parser = SseParser::new();
        let mut body = response.bytes_stream();
        let mut pending = Vec::new();

        loop {
            let item = self
                .await_cancelable_timeout(
                    body.next(),
                    cancel.as_ref(),
                    self.timeouts.idle,
                    "model SSE idle timeout",
                )
                .await?;
            let Some(item) = item else {
                break;
            };
            let bytes = item.map_err(|e| CoreError::other(format!("stream error: {e}")))?;
            pending.extend_from_slice(&bytes);

            // Decode as much valid UTF-8 as possible, retaining a trailing
            // partial code point for the next chunk.
            let (text, consumed) = decode_utf8_prefix(&pending);
            if !text.is_empty() {
                for payload in parser.feed(&text) {
                    if sink.on_event(&payload)? {
                        return Ok(());
                    }
                }
            }
            pending.drain(..consumed);
        }

        if let Some(payload) = parser.flush() {
            sink.on_event(&payload)?;
        }
        Ok(())
    }

    async fn await_cancelable<F, T>(
        &self,
        future: F,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<T, CoreError>
    where
        F: std::future::Future<Output = T>,
    {
        match cancel {
            Some(cancel) => {
                tokio::select! {
                    value = future => Ok(value),
                    _ = wait_for_cancel(cancel.clone()) => Err(CoreError::other("request cancelled")),
                }
            }
            None => Ok(future.await),
        }
    }

    async fn await_cancelable_timeout<F, T>(
        &self,
        future: F,
        cancel: Option<&Arc<AtomicBool>>,
        timeout: Duration,
        label: &'static str,
    ) -> Result<T, CoreError>
    where
        F: std::future::Future<Output = T>,
    {
        tokio::time::timeout(timeout, self.await_cancelable(future, cancel))
            .await
            .map_err(|_| CoreError::other(label))?
    }

    async fn read_provider_error_body(
        &self,
        response: reqwest::Response,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<String, CoreError> {
        let mut body_stream = response.bytes_stream();
        let mut body = Vec::new();

        while let Some(chunk) = self
            .await_cancelable_timeout(
                body_stream.next(),
                cancel,
                self.timeouts.idle,
                "provider error body idle timeout",
            )
            .await?
        {
            let chunk = chunk.map_err(|error| {
                CoreError::other(format!("reading provider error body failed: {error}"))
            })?;
            let remaining = MAX_PROVIDER_ERROR_BODY_BYTES.saturating_sub(body.len());
            if remaining == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }

        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    async fn read_get_body(
        &self,
        response: reqwest::Response,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<String, CoreError> {
        let mut body_stream = response.bytes_stream();
        let mut body = Vec::new();

        while let Some(chunk) = self
            .await_cancelable_timeout(
                body_stream.next(),
                cancel,
                self.timeouts.idle,
                "GET body idle timeout",
            )
            .await?
        {
            let chunk = chunk
                .map_err(|error| CoreError::other(format!("reading GET body failed: {error}")))?;
            if body.len().saturating_add(chunk.len()) > MAX_GET_BODY_BYTES {
                return Err(CoreError::other(format!(
                    "GET response body exceeds {MAX_GET_BODY_BYTES} bytes"
                )));
            }
            body.extend_from_slice(&chunk);
        }

        String::from_utf8(body)
            .map_err(|error| CoreError::other(format!("GET response body is not UTF-8: {error}")))
    }
}

#[cfg(test)]
fn provider_error(status: u16, body: &str) -> CoreError {
    provider_error_with_metadata(status, body, None, None)
}

fn provider_error_with_metadata(
    status: u16,
    body: &str,
    retry_after_ms: Option<u64>,
    request_id: Option<String>,
) -> CoreError {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let error = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .or(parsed.as_ref());
    crate::provider_error::from_json_error(
        Some(status),
        error,
        None,
        body,
        retry_after_ms,
        request_id,
    )
}

fn provider_response_metadata(response: &reqwest::Response) -> (Option<u64>, Option<String>) {
    let retry_after_ms = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after_ms);
    let request_id = ["request-id", "x-request-id", "x-deepseek-request-id"]
        .iter()
        .find_map(|name| response.headers().get(*name))
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().chars().take(256).collect::<String>())
        .filter(|value| !value.is_empty());
    (retry_after_ms, request_id)
}

fn parse_retry_after_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return seconds.checked_mul(1_000).filter(|delay| *delay > 0);
    }

    let deadline = parse_http_date(value)?;
    let remaining = (deadline - time::OffsetDateTime::now_utc()).whole_milliseconds();
    u64::try_from(remaining).ok().filter(|delay| *delay > 0)
}

fn parse_http_date(value: &str) -> Option<time::OffsetDateTime> {
    let imf_fixdate = time::macros::format_description!(
        "[weekday repr:short], [day] [month repr:short] [year padding:none] [hour]:[minute]:[second] GMT"
    );
    let obsolete_rfc850 = time::macros::format_description!(
        "[weekday repr:long], [day]-[month repr:short]-[year repr:last_two] [hour]:[minute]:[second] GMT"
    );
    let obsolete_asctime = time::macros::format_description!(
        "[weekday repr:short] [month repr:short] [day padding:space] [hour]:[minute]:[second] [year padding:none]"
    );

    for format in [imf_fixdate, obsolete_rfc850, obsolete_asctime] {
        let mut parsed = time::parsing::Parsed::new();
        let Ok(remaining) = parsed.parse_items(value.as_bytes(), format) else {
            continue;
        };
        if !remaining.is_empty() {
            continue;
        }
        if let Some(year) = parsed
            .year()
            .or_else(|| parsed.year_last_two().map(i32::from))
        {
            let century = match year {
                0..=68 => 2_000,
                69..=99 => 1_900,
                _ => 0,
            };
            parsed.set_year(year + century);
        }
        if let Ok(deadline) = time::PrimitiveDateTime::try_from(parsed) {
            return Some(deadline.assume_utc());
        }
    }
    None
}

/// Decode the longest valid UTF-8 prefix of `bytes`, returning the decoded text
/// and the number of bytes consumed (so a trailing partial code point is kept).
fn decode_utf8_prefix(bytes: &[u8]) -> (String, usize) {
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), bytes.len()),
        Err(e) => {
            let valid = e.valid_up_to();
            let s = std::str::from_utf8(&bytes[..valid])
                .expect("valid_up_to guarantees valid UTF-8")
                .to_string();
            (s, valid)
        }
    }
}

fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::Relaxed))
        .unwrap_or(false)
}

async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn audit_get_json_is_bounded_while_waiting_for_headers_and_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let mut unbounded = Vec::new();
        for mode in ["headers", "success_body", "error_body"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (request_received, received) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let mode = mode.to_string();
            let server_mode = mode.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    assert!(request.len() < 16 * 1024);
                    if request.windows(4).any(|part| part == b"\r\n\r\n") {
                        break;
                    }
                }
                request_received.send(()).unwrap();

                let body = if server_mode == "error_body" {
                    r#"{"error":{"message":"audit failure"}}"#
                } else {
                    r#"{"ok":true}"#
                };
                if server_mode == "headers" {
                    released.await.unwrap();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                } else {
                    let status = if server_mode == "error_body" {
                        "503 Service Unavailable"
                    } else {
                        "200 OK"
                    };
                    let headers = format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    socket.write_all(headers.as_bytes()).await.unwrap();
                    socket.write_all(&body.as_bytes()[..1]).await.unwrap();
                    released.await.unwrap();
                    let _ = socket.write_all(&body.as_bytes()[1..]).await;
                }
            });

            let client = reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_secs(1))
                .build()
                .unwrap();
            let transport =
                ReqwestTransport::with_client(client).with_timeouts(TransportTimeouts {
                    connect: Duration::from_secs(1),
                    first_byte: Duration::from_millis(50),
                    idle: Duration::from_millis(50),
                    total: Duration::from_millis(50),
                });
            let mut operation = tokio::spawn(async move {
                transport
                    .get_json(&format!("http://{address}/models"), "audit-placeholder")
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), received)
                .await
                .unwrap()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
            let returned_within_budget = operation.is_finished();
            release.send(()).unwrap();
            let result = tokio::time::timeout(Duration::from_secs(2), &mut operation)
                .await
                .unwrap()
                .unwrap();
            server.await.unwrap();

            eprintln!(
                "mode={mode}, returned_within_50ms_budget={returned_within_budget}, result={result:?}"
            );
            if !returned_within_budget {
                unbounded.push(mode);
            }
        }

        assert!(
            unbounded.is_empty(),
            "GET remained pending beyond the configured 50ms budget: {unbounded:?}"
        );
    }

    #[tokio::test]
    async fn get_json_cancelled_interrupts_stalled_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (body_started, body_ready) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{";
            socket.write_all(response).await.unwrap();
            body_started.send(()).unwrap();
            released.await.unwrap();
            let _ = socket.write_all(b"}").await;
        });

        let transport =
            ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap())
                .with_timeouts(TransportTimeouts {
                    connect: Duration::from_secs(1),
                    first_byte: Duration::from_secs(1),
                    idle: Duration::from_secs(2),
                    total: Duration::from_secs(4),
                });
        let cancel = Arc::new(AtomicBool::new(false));
        let request_cancel = cancel.clone();
        let mut operation = tokio::spawn(async move {
            transport
                .get_json_cancelled(
                    &format!("http://{address}/models"),
                    "audit-placeholder",
                    request_cancel,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), body_ready)
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.store(true, Ordering::Release);

        let result = tokio::time::timeout(Duration::from_millis(500), &mut operation)
            .await
            .expect("cancel-aware GET did not finish within 500ms")
            .unwrap();
        let error = result.expect_err("cancelled GET unexpectedly succeeded");
        assert!(
            error.to_string().contains("request cancelled"),
            "unexpected cancellation error: {error:?}"
        );
        release.send(()).unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cross_origin_redirect_does_not_forward_bearer_authorization() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let redirect_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_address = redirect_listener.local_addr().unwrap();
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_address = target_listener.local_addr().unwrap();
        let (first_hop_sent, first_hop_received) = oneshot::channel();
        let (second_hop_sent, second_hop_received) = oneshot::channel();

        let redirect_server = tokio::spawn(async move {
            let (mut socket, _) = redirect_listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            first_hop_sent.send(request).unwrap();
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/capture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let target_server = tokio::spawn(async move {
            let (mut socket, _) = target_listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            second_hop_sent.send(request).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                )
                .await
                .unwrap();
        });

        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let transport = ReqwestTransport::with_client(client);
        let body = transport
            .get_json(
                &format!("http://{redirect_address}/models"),
                "audit-secret-placeholder",
            )
            .await
            .unwrap();
        let first_hop = String::from_utf8(first_hop_received.await.unwrap())
            .unwrap()
            .to_ascii_lowercase();
        let second_hop = String::from_utf8(second_hop_received.await.unwrap())
            .unwrap()
            .to_ascii_lowercase();
        redirect_server.await.unwrap();
        target_server.await.unwrap();

        assert_eq!(body, r#"{"ok":true}"#);
        assert!(first_hop.contains("authorization: bearer audit-secret-placeholder"));
        assert!(!second_hop.contains("authorization:"));
    }

    #[tokio::test]
    async fn audit_get_error_body_does_not_echo_bearer_credential() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let headers = String::from_utf8(request).unwrap().to_ascii_lowercase();
            let authorization = headers
                .lines()
                .find_map(|line| line.strip_prefix("authorization: "))
                .unwrap();
            let body =
                format!(r#"{{"error":{{"message":"credential rejected: {authorization}"}}}}"#);
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let transport = ReqwestTransport::with_client(client);
        let error = transport
            .get_json(
                &format!("http://{address}/models"),
                "sk-audit-secret-placeholder",
            )
            .await
            .unwrap_err()
            .to_string();
        server.await.unwrap();

        assert!(
            !error.contains("sk-audit-secret-placeholder"),
            "GET error propagated the reflected bearer credential: {error}"
        );
    }

    #[tokio::test]
    async fn get_json_retains_provider_retry_metadata() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            loop {
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#;
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nRetry-After: 3\r\nX-Request-Id: audit-request-id\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let transport =
            ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap());
        let error = transport
            .get_json(&format!("http://{address}/models"), "audit-placeholder")
            .await
            .unwrap_err();
        server.await.unwrap();

        assert!(matches!(
            error,
            CoreError::Provider {
                status: Some(429),
                retry_after_ms: Some(3_000),
                ref request_id,
                ..
            } if request_id.as_deref() == Some("audit-request-id")
        ));
    }

    #[tokio::test]
    async fn audit_cancel_interrupts_stalled_success_and_error_bodies() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::oneshot;

        let mut stalled = Vec::new();
        for status in [200, 429, 503] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (headers_sent, headers_received) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut chunk = [0; 1024];
                loop {
                    let count = socket.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    assert!(request.len() < 16 * 1024);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = std::str::from_utf8(&request[..end]).unwrap();
                        let length: usize = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .unwrap()
                            .1
                            .trim()
                            .parse()
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let body = if status == 200 {
                    "data: [DONE]\n\n"
                } else {
                    r#"{"error":{"code":"audit_error","message":"temporary failure"}}"#
                };
                let content_type = if status == 200 {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let header = format!(
                    "HTTP/1.1 {status} Audit\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&body.as_bytes()[..1]).await.unwrap();
                headers_sent.send(()).unwrap();
                released.await.unwrap();
                // A correctly cancelled client may already have closed its socket.
                let _ = socket.write_all(&body.as_bytes()[1..]).await;
            });
            let transport = ReqwestTransport::with_client(
                reqwest::Client::builder().no_proxy().build().unwrap(),
            )
            .with_timeouts(TransportTimeouts {
                connect: Duration::from_secs(1),
                first_byte: Duration::from_secs(1),
                idle: Duration::from_secs(2),
                total: Duration::from_secs(4),
            });
            let cancel = Arc::new(AtomicBool::new(false));
            let request_cancel = cancel.clone();
            let mut operation = tokio::spawn(async move {
                let request = TransportRequest {
                    url: format!("http://{address}/v1/chat/completions"),
                    api_key: "audit-placeholder".to_string(),
                    body: "{}".to_string(),
                };
                transport
                    .stream_cancelled(request, &mut |_: &str| Ok(false), request_cancel)
                    .await
            });
            tokio::time::timeout(Duration::from_secs(2), headers_received)
                .await
                .unwrap()
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(
                !operation.is_finished(),
                "fixture body must still be pending"
            );
            let requested_at = std::time::Instant::now();
            cancel.store(true, Ordering::Release);
            let completion = tokio::time::timeout(Duration::from_millis(500), &mut operation).await;
            let elapsed = requested_at.elapsed();
            let prompt = completion.is_ok();
            let result = match completion {
                Ok(completed) => {
                    release.send(()).unwrap();
                    completed.unwrap()
                }
                Err(_) => {
                    release.send(()).unwrap();
                    operation.await.unwrap()
                }
            };
            server.await.unwrap();
            eprintln!(
                "status={status}, cancelled_within_500ms={prompt}, observed_ms={}, result={result:?}",
                elapsed.as_millis()
            );
            if prompt {
                assert!(result.unwrap_err().to_string().contains("cancelled"));
            } else {
                stalled.push(status);
            }
        }
        assert!(
            stalled.is_empty(),
            "cancellation did not interrupt stalled response bodies: {stalled:?}"
        );
    }

    #[test]
    fn decodes_full_utf8() {
        let (s, n) = decode_utf8_prefix("hello".as_bytes());
        assert_eq!(s, "hello");
        assert_eq!(n, 5);
    }

    #[test]
    fn retains_partial_codepoint() {
        // "é" is 0xC3 0xA9; feed only the first byte.
        let bytes = [b'a', 0xC3];
        let (s, n) = decode_utf8_prefix(&bytes);
        assert_eq!(s, "a");
        assert_eq!(n, 1);
    }

    #[test]
    fn parses_openai_compatible_provider_error() {
        let error = provider_error(
            413,
            r#"{"error":{"message":"maximum context window exceeded","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
        );
        assert!(matches!(
            error,
            CoreError::Provider {
                status: Some(413),
                ref code,
                ref message,
                ..
            } if code.as_deref() == Some("context_length_exceeded")
                && message == "maximum context window exceeded"
        ));
    }

    #[test]
    fn provider_error_redacts_api_key_shaped_tokens() {
        let error = provider_error(401, "bad key sk-secret-value");
        assert!(!error.to_string().contains("secret-value"));
    }

    #[test]
    fn parses_bounded_retry_after_seconds_and_http_date() {
        assert_eq!(parse_retry_after_ms("3"), Some(3_000));
        assert_eq!(parse_retry_after_ms("0"), None);
        assert_eq!(parse_retry_after_ms("invalid"), None);

        let formats = [
            time::macros::format_description!(
                "[weekday repr:short], [day] [month repr:short] [year padding:none] [hour]:[minute]:[second] GMT"
            ),
            time::macros::format_description!(
                "[weekday repr:long], [day]-[month repr:short]-[year repr:last_two] [hour]:[minute]:[second] GMT"
            ),
            time::macros::format_description!(
                "[weekday repr:short] [month repr:short] [day padding:space] [hour]:[minute]:[second] [year padding:none]"
            ),
        ];
        let future = time::OffsetDateTime::now_utc() + time::Duration::seconds(60);
        for format in formats {
            let header = future.format(format).unwrap();
            let parsed = parse_retry_after_ms(&header).expect("future HTTP date should parse");
            assert!(parsed > 0 && parsed <= 60_000, "header={header}");
        }
    }

    #[test]
    fn audit_long_unicode_provider_error_remains_structured() {
        let message = "错".repeat(800);
        let json = serde_json::json!({
            "error": { "message": message, "code": "rate_limit_exceeded" }
        })
        .to_string();
        let mut panicked = Vec::new();
        for (format, body) in [("json", json), ("plain", message)] {
            let result = std::panic::catch_unwind(|| provider_error(429, &body));
            eprintln!("format={format}, parser_panicked={}", result.is_err());
            match result {
                Ok(error) => {
                    assert!(matches!(
                        error,
                        CoreError::Provider {
                            status: Some(429),
                            ..
                        }
                    ));
                }
                Err(_) => panicked.push(format),
            }
        }
        assert!(
            panicked.is_empty(),
            "provider error parsing must return a structured failure, panicked formats={panicked:?}"
        );
    }
}
