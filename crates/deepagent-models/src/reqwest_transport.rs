//! Real HTTP transport backed by `reqwest` (only compiled with `--features http`).
//!
//! Streams the response body in chunks, decodes UTF-8 incrementally, and runs
//! decoded text through [`SseParser`], forwarding only complete SSE events.

use async_trait::async_trait;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use deepagent_core::error::{CoreError, Result};

use crate::sse::SseParser;
use crate::transport::{
    EventSink, FileUploadRequest, HttpTransport, TransportRequest, UploadedFile,
};

/// `reqwest`-backed transport with connection reuse.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    timeouts: TransportTimeouts,
}

#[derive(Debug, Clone, Copy)]
pub struct TransportTimeouts {
    pub connect: Duration,
    /// Time allowed for response headers, within the non-streaming total budget.
    pub first_byte: Duration,
    /// Maximum wait for the next response body chunk, reset by each chunk.
    pub idle: Duration,
    /// GET lifetime (also capped at 30s), or POST response headers/error body.
    /// Successful SSE bodies use `idle` and cancellation, not this stopwatch.
    pub total: Duration,
}

const MAX_PROVIDER_ERROR_BODY_BYTES: usize = 64 * 1024;
const MAX_GET_BODY_BYTES: usize = 16 * 1024 * 1024;
const GET_TOTAL_TIMEOUT_CAP: Duration = Duration::from_secs(30);

#[derive(serde::Deserialize)]
struct FileUploadResponse {
    id: String,
    expires_at: Option<i64>,
}

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

    /// Build from an existing client (to share pools / config). An explicit
    /// timeout configured on that client remains the caller's additional policy.
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
    fn supports_file_upload(&self) -> bool {
        true
    }

    async fn upload_file(&self, request: FileUploadRequest) -> Result<UploadedFile> {
        tokio::time::timeout(self.timeouts.total, self.upload_file_inner(request, None))
            .await
            .map_err(|_| CoreError::other("DeepSeek Files upload deadline exceeded"))?
    }

    async fn upload_file_cancelled(
        &self,
        request: FileUploadRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<UploadedFile> {
        tokio::time::timeout(
            self.timeouts.total,
            self.upload_file_inner(request, Some(cancel)),
        )
        .await
        .map_err(|_| CoreError::other("DeepSeek Files upload deadline exceeded"))?
    }

    async fn delete_file(&self, url: &str, api_key: &str, file_id: &str) -> Result<()> {
        self.delete_file_inner(url, api_key, file_id, None).await
    }

    async fn delete_file_cancelled(
        &self,
        url: &str,
        api_key: &str,
        file_id: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        tokio::time::timeout(
            self.timeouts.total,
            self.delete_file_inner(url, api_key, file_id, Some(cancel)),
        )
        .await
        .map_err(|_| CoreError::other("DeepSeek file cleanup deadline exceeded"))?
    }

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
        self.stream_inner(request, sink, None).await
    }

    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        self.stream_inner(request, sink, Some(cancel)).await
    }
}

impl ReqwestTransport {
    async fn delete_file_inner(
        &self,
        url: &str,
        api_key: &str,
        file_id: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        if !crate::file_references::valid_file_id(file_id) {
            return Err(CoreError::invalid("DeepSeek Files reference ID is invalid"));
        }
        if is_cancelled(cancel.as_ref()) {
            return Err(CoreError::other("request cancelled"));
        }
        let send = self
            .client
            .delete(format!("{}/{}", url.trim_end_matches('/'), file_id))
            .bearer_auth(api_key)
            .header("accept", "application/json")
            .send();
        let response = self
            .await_cancelable_timeout(
                send,
                cancel.as_ref(),
                self.timeouts.first_byte.min(self.timeouts.total),
                "DeepSeek file cleanup timeout",
            )
            .await?
            .map_err(|_| CoreError::other("DeepSeek file cleanup failed"))?;
        if !response.status().is_success() {
            return Err(provider_error_with_metadata(
                response.status().as_u16(),
                "DeepSeek file cleanup failed",
                None,
                provider_response_metadata(&response).1,
            ));
        }
        Ok(())
    }

    async fn upload_file_inner(
        &self,
        request: FileUploadRequest,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<UploadedFile> {
        if is_cancelled(cancel.as_ref()) {
            return Err(CoreError::other("request cancelled"));
        }
        let part = reqwest::multipart::Part::bytes(request.bytes)
            .file_name(request.filename)
            .mime_str(&request.mime)
            .map_err(|_| CoreError::invalid("image_file_upload: invalid multipart metadata"))?;
        let form = reqwest::multipart::Form::new()
            .text("purpose", "user_data")
            .text("expires_after[anchor]", "created_at")
            .text(
                "expires_after[seconds]",
                crate::file_references::FILES_TTL_SECONDS.to_string(),
            )
            .part("file", part);
        let send = self
            .client
            .post(&request.url)
            .bearer_auth(&request.api_key)
            .multipart(form)
            .send();
        let response = self
            .await_cancelable_timeout(
                send,
                cancel.as_ref(),
                self.timeouts.first_byte.min(self.timeouts.total),
                "DeepSeek Files upload timeout",
            )
            .await?
            .map_err(|_| CoreError::other("DeepSeek Files upload transport failed"))?;
        if !response.status().is_success() {
            let status = response.status();
            let request_id = provider_response_metadata(&response).1;
            // Do not forward provider multipart errors; retain only status and ID.
            let _ = self
                .read_provider_error_body(response, cancel.as_ref())
                .await?;
            return Err(provider_error_with_metadata(
                status.as_u16(),
                "DeepSeek Files image upload failed",
                None,
                request_id,
            ));
        }
        let body = self.read_get_body(response, cancel.as_ref()).await?;
        let metadata: FileUploadResponse = serde_json::from_str(&body)
            .map_err(|_| CoreError::other("DeepSeek Files response has invalid metadata"))?;
        if !crate::file_references::valid_file_id(&metadata.id) {
            return Err(CoreError::other(
                "DeepSeek Files response omitted a valid file id",
            ));
        }
        Ok(UploadedFile {
            id: metadata.id,
            expires_at: metadata.expires_at,
        })
    }

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

    /// The finite HTTP preparation phase keeps its total deadline even if a
    /// non-success response drips an error body often enough to avoid idle.
    async fn open_stream(
        &self,
        request: TransportRequest,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<reqwest::Response> {
        if is_cancelled(cancel) {
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
                cancel,
                self.timeouts.first_byte,
                "model first-byte timeout",
            )
            .await
            .map_err(|e| CoreError::other(format!("request failed: {e}")))?
            .map_err(|e| CoreError::other(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let (retry_after_ms, request_id) = provider_response_metadata(&response);
            let detail = self.read_provider_error_body(response, cancel).await?;
            return Err(provider_error_with_metadata(
                status.as_u16(),
                &detail,
                retry_after_ms,
                request_id,
            ));
        }

        Ok(response)
    }

    async fn stream_inner(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<()> {
        let response = tokio::time::timeout(
            self.timeouts.total,
            self.open_stream(request, cancel.as_ref()),
        )
        .await
        .map_err(|_| CoreError::other("model response total deadline exceeded"))??;

        let mut parser = SseParser::new();
        let mut body = response.bytes_stream();
        let mut pending = Vec::new();
        let mut consumed_stream_bytes = 0usize;

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
            let (text, consumed, invalid) = match decode_utf8_prefix(&pending) {
                Ok((text, consumed)) => (text, consumed, None),
                Err(error) => (error.valid_prefix, error.valid_up_to, Some(error.error_len)),
            };
            if !text.is_empty() {
                for payload in parser.feed(&text) {
                    if sink.on_event(&payload)? {
                        return Ok(());
                    }
                }
            }
            pending.drain(..consumed);
            let invalid_offset = consumed_stream_bytes.saturating_add(consumed);
            consumed_stream_bytes = invalid_offset;
            if let Some(error_len) = invalid {
                return Err(CoreError::Serialization(format!(
                    "provider SSE contains invalid UTF-8 at byte {invalid_offset} (sequence length {error_len})"
                )));
            }
        }

        if !pending.is_empty() {
            return Err(CoreError::Serialization(format!(
                "provider SSE ended with an incomplete UTF-8 sequence ({} trailing bytes)",
                pending.len()
            )));
        }

        // EOF is not an SSE event boundary. The model accumulator diagnoses a
        // missing terminal instead of accepting an unterminated terminal line.
        let _ = parser.flush();
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

#[derive(Debug)]
struct InvalidUtf8Prefix {
    valid_prefix: String,
    valid_up_to: usize,
    error_len: usize,
}

/// Keep only a trailing incomplete code point for the next HTTP chunk. A
/// sequence with a known error length cannot be repaired by future bytes.
fn decode_utf8_prefix(bytes: &[u8]) -> std::result::Result<(String, usize), InvalidUtf8Prefix> {
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok((s.to_string(), bytes.len())),
        Err(e) => {
            let valid = e.valid_up_to();
            let s = std::str::from_utf8(&bytes[..valid])
                .expect("valid_up_to guarantees valid UTF-8")
                .to_string();
            match e.error_len() {
                Some(error_len) => Err(InvalidUtf8Prefix {
                    valid_prefix: s,
                    valid_up_to: valid,
                    error_len,
                }),
                None => Ok((s, valid)),
            }
        }
    }
}

#[cfg(test)]
#[path = "reqwest_utf8_audit.rs"]
mod utf8_audit;

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

    async fn read_complete_file_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
        use tokio::io::AsyncReadExt;
        let mut received = Vec::new();
        loop {
            let mut buffer = [0; 1024];
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            received.extend_from_slice(&buffer[..read]);
            assert!(received.len() < 32 * 1024);
            if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&received[..end]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if received.len() >= end + 4 + length {
                    return received;
                }
            }
        }
    }

    #[tokio::test]
    async fn files_errors_never_echo_credentials_and_delete_uses_the_files_route() {
        use tokio::io::AsyncWriteExt;
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_complete_file_request(&mut socket).await;
            assert!(request.starts_with(b"POST /files "));
            let body = r#"{"error":{"message":"Bearer synthetic-secret sk-synthetic-secret","code":"unsafe-provider-body"}}"#;
            socket.write_all(format!(
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()
            ).as_bytes()).await.unwrap();
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_complete_file_request(&mut socket).await;
            assert!(request.starts_with(b"DELETE /files/file-api-cleanup "));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let transport =
            ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap());
        let files_url = format!("http://{address}/files");
        let error = transport
            .upload_file(FileUploadRequest {
                url: files_url.clone(),
                api_key: "synthetic-secret".into(),
                filename: "image.png".into(),
                mime: "image/png".into(),
                bytes: b"synthetic".to_vec(),
            })
            .await
            .unwrap_err();
        let display = error.to_string();
        assert!(matches!(
            error,
            CoreError::Provider {
                status: Some(429),
                ..
            }
        ));
        assert!(!display.contains("synthetic-secret"));
        assert!(!display.contains("unsafe-provider-body"));
        transport
            .delete_file(&files_url, "synthetic-secret", "file-api-cleanup")
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn files_upload_header_wait_uses_existing_cancellation_and_deadline() {
        use tokio::net::TcpListener;
        for cancel_request in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (entered, ready) = tokio::sync::oneshot::channel();
            let (release, held) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_complete_file_request(&mut socket).await;
                entered.send(()).unwrap();
                held.await.unwrap();
            });
            let transport = ReqwestTransport::with_client(
                reqwest::Client::builder().no_proxy().build().unwrap(),
            )
            .with_timeouts(TransportTimeouts {
                connect: Duration::from_secs(1),
                first_byte: Duration::from_millis(300),
                idle: Duration::from_millis(300),
                total: Duration::from_millis(500),
            });
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = cancel.clone();
            let operation = tokio::spawn(async move {
                transport
                    .upload_file_cancelled(
                        FileUploadRequest {
                            url: format!("http://{address}/files"),
                            api_key: "synthetic-key".into(),
                            filename: "image.png".into(),
                            mime: "image/png".into(),
                            bytes: b"synthetic".to_vec(),
                        },
                        flag,
                    )
                    .await
            });
            ready.await.unwrap();
            if cancel_request {
                cancel.store(true, Ordering::Release);
            }
            let error = tokio::time::timeout(Duration::from_secs(2), operation)
                .await
                .expect("upload must settle")
                .unwrap()
                .unwrap_err();
            assert!(error.to_string().contains(if cancel_request {
                "cancelled"
            } else {
                "timeout"
            }));
            release.send(()).unwrap();
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn files_upload_uses_documented_multipart_contract_and_parses_expiration() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut received = Vec::new();
            let header_end;
            let body_length;
            loop {
                let mut chunk = [0; 1024];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&chunk[..count]);
                if let Some(end) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                    header_end = end + 4;
                    let headers = std::str::from_utf8(&received[..end]).unwrap();
                    assert!(headers
                        .to_ascii_lowercase()
                        .contains("authorization: bearer synthetic-files-key"));
                    assert!(headers
                        .to_ascii_lowercase()
                        .contains("content-type: multipart/form-data; boundary="));
                    body_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break;
                }
            }
            while received.len() < header_end + body_length {
                let mut chunk = [0; 1024];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                received.extend_from_slice(&chunk[..count]);
            }
            let body = String::from_utf8_lossy(&received[header_end..header_end + body_length]);
            for expected in [
                "name=\"purpose\"",
                "user_data",
                "name=\"expires_after[anchor]\"",
                "created_at",
                "name=\"expires_after[seconds]\"",
                &crate::file_references::FILES_TTL_SECONDS.to_string(),
                "name=\"file\"; filename=\"image.png\"",
                "image/png",
                "SYNTHETIC_IMAGE_PAYLOAD",
            ] {
                assert!(body.contains(expected), "multipart form missing {expected}");
            }
            let response_body = r#"{"id":"file-api-offline-1","expires_at":1800000000}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });

        let transport =
            ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap());
        let uploaded = transport
            .upload_file(FileUploadRequest {
                url: format!("http://{address}/v1/files"),
                api_key: "synthetic-files-key".into(),
                filename: "image.png".into(),
                mime: "image/png".into(),
                bytes: b"SYNTHETIC_IMAGE_PAYLOAD".to_vec(),
            })
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(uploaded.id, "file-api-offline-1");
        assert_eq!(uploaded.expires_at, Some(1_800_000_000));
    }

    #[tokio::test]
    async fn audit_active_provider_stream_is_not_cut_off_by_total_deadline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        async fn write_chunk(
            socket: &mut tokio::net::TcpStream,
            data: &[u8],
        ) -> std::io::Result<()> {
            socket
                .write_all(format!("{:X}\r\n", data.len()).as_bytes())
                .await?;
            socket.write_all(data).await?;
            socket.write_all(b"\r\n").await
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (response_started, response_ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") {
                    break;
                }
            }

            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            response_started.send(()).unwrap();

            let mut writes_ok = true;
            for index in 0..8 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let event = format!("data: {{\"type\":\"progress\",\"index\":{index}}}\n\n");
                if write_chunk(&mut socket, event.as_bytes()).await.is_err() {
                    writes_ok = false;
                    break;
                }
            }
            if writes_ok {
                let _ = write_chunk(&mut socket, b"data: [DONE]\n\n").await;
                let _ = socket.write_all(b"0\r\n\r\n").await;
            }
        });

        let transport =
            ReqwestTransport::with_client(reqwest::Client::builder().no_proxy().build().unwrap())
                .with_timeouts(TransportTimeouts {
                    connect: Duration::from_secs(1),
                    first_byte: Duration::from_secs(1),
                    idle: Duration::from_secs(1),
                    total: Duration::from_millis(250),
                });
        let request = TransportRequest {
            url: format!("http://{address}/responses"),
            api_key: "audit-placeholder".to_string(),
            body: "{}".to_string(),
        };
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_events = events.clone();
        let mut sink = move |data: &str| {
            sink_events.lock().unwrap().push(data.to_string());
            Ok(data == "[DONE]")
        };
        let operation = tokio::spawn(async move { transport.stream(request, &mut sink).await });
        tokio::time::timeout(Duration::from_secs(1), response_ready)
            .await
            .unwrap()
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), operation)
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();
        let events = events.lock().unwrap().clone();

        assert!(
            result.is_ok(),
            "active provider stream ended after {} progress events and before its terminal event: {result:?}",
            events.len()
        );
        assert_eq!(events.last().map(String::as_str), Some("[DONE]"));
    }

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
        let (s, n) = decode_utf8_prefix("hello".as_bytes()).unwrap();
        assert_eq!(s, "hello");
        assert_eq!(n, 5);
    }

    #[test]
    fn retains_partial_codepoint() {
        // "é" is 0xC3 0xA9; feed only the first byte.
        let bytes = [b'a', 0xC3];
        let (s, n) = decode_utf8_prefix(&bytes).unwrap();
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
