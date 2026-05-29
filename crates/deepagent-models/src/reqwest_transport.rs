//! Real HTTP transport backed by `reqwest` (only compiled with `--features http`).
//!
//! Streams the response body in chunks, decodes UTF-8 incrementally, and runs
//! each line through the [`SseParser`] so partial network frames never drop
//! data.

use async_trait::async_trait;
use futures::StreamExt;

use deepagent_core::error::{CoreError, Result};

use crate::sse::SseParser;
use crate::transport::{EventSink, HttpTransport, TransportRequest};

/// `reqwest`-backed transport with connection reuse.
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestTransport {
    /// Build a transport with a fresh client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    /// Build from an existing client (to share pools / config).
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let response = self
            .client
            .post(&request.url)
            .bearer_auth(&request.api_key)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(request.body)
            .send()
            .await
            .map_err(|e| CoreError::other(format!("request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            return Err(CoreError::other(format!(
                "model API returned {status}: {detail}"
            )));
        }

        let mut parser = SseParser::new();
        let mut body = response.bytes_stream();
        let mut pending = Vec::new();

        while let Some(item) = body.next().await {
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
}
