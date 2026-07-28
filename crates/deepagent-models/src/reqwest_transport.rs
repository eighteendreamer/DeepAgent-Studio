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
        let response = self
            .client
            .get(url)
            .bearer_auth(api_key)
            .header("accept", "application/json")
            .send()
            .await
            .map_err(|e| CoreError::other(format!("GET {url} failed: {e}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(CoreError::other(format!(
                "GET {url} returned {status}: {detail}"
            )));
        }
        response
            .text()
            .await
            .map_err(|e| CoreError::other(format!("reading GET {url} body failed: {e}")))
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
            let detail = response.text().await.unwrap_or_default();
            return Err(provider_error(status.as_u16(), &detail));
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
}

fn provider_error(status: u16, body: &str) -> CoreError {
    let parsed = serde_json::from_str::<serde_json::Value>(body).ok();
    let error = parsed
        .as_ref()
        .and_then(|value| value.get("error"))
        .unwrap_or_else(|| parsed.as_ref().unwrap_or(&serde_json::Value::Null));
    let code = error
        .get("code")
        .and_then(serde_json::Value::as_str)
        .or_else(|| error.get("type").and_then(serde_json::Value::as_str))
        .map(str::to_string);
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(body);
    CoreError::provider(Some(status), code, sanitize_provider_message(message))
}

fn sanitize_provider_message(message: &str) -> String {
    let mut sanitized = message
        .split_whitespace()
        .map(|word| {
            if word.starts_with("sk-") && word.len() > 7 {
                "sk-…".to_string()
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if sanitized.len() > 2_000 {
        sanitized.truncate(2_000);
        sanitized.push_str("…");
    }
    if sanitized.is_empty() {
        "provider request failed without an error message".to_string()
    } else {
        sanitized
    }
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
            } if code.as_deref() == Some("context_length_exceeded")
                && message == "maximum context window exceeded"
        ));
    }

    #[test]
    fn provider_error_redacts_api_key_shaped_tokens() {
        let error = provider_error(401, "bad key sk-secret-value");
        assert!(!error.to_string().contains("secret-value"));
    }
}
