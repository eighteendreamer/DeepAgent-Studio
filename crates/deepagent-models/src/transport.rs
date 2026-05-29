//! Transport abstraction.
//!
//! The model client is decoupled from any concrete HTTP stack via the
//! [`HttpTransport`] trait. This keeps all the streaming-assembly logic
//! testable offline (via [`MockTransport`]) and lets the real `reqwest`-based
//! transport live behind the optional `http` feature.

use async_trait::async_trait;

use deepagent_core::error::Result;

/// An outbound chat request at the transport level (already serialized).
#[derive(Debug, Clone)]
pub struct TransportRequest {
    /// Full endpoint URL.
    pub url: String,
    /// Bearer API key.
    pub api_key: String,
    /// JSON request body.
    pub body: String,
}

/// A transport that can stream an SSE response.
///
/// Implementations push each decoded SSE `data:` payload to `on_event`. The
/// boxed closure returns `Ok(true)` to signal the caller wants to stop early
/// (e.g. `[DONE]` was seen).
#[async_trait]
pub trait HttpTransport: Send + Sync {
    /// Perform the request and drive `sink` with each SSE payload string.
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()>;
}

/// Receives decoded SSE payloads as they arrive.
pub trait EventSink: Send {
    /// Handle one `data:` payload. Returning `Ok(true)` requests early stop.
    fn on_event(&mut self, data: &str) -> Result<bool>;
}

impl<F> EventSink for F
where
    F: FnMut(&str) -> Result<bool> + Send,
{
    fn on_event(&mut self, data: &str) -> Result<bool> {
        self(data)
    }
}

/// A deterministic transport that replays canned SSE payloads. Used in tests
/// and for offline development of the streaming pipeline.
#[derive(Debug, Clone, Default)]
pub struct MockTransport {
    /// Raw SSE payload strings to emit, in order (each as one `data:` event).
    pub events: Vec<String>,
}

impl MockTransport {
    /// Build from a list of payload strings.
    pub fn new(events: impl IntoIterator<Item = String>) -> Self {
        Self {
            events: events.into_iter().collect(),
        }
    }
}

#[async_trait]
impl HttpTransport for MockTransport {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        for ev in &self.events {
            if sink.on_event(ev)? {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_transport_replays_events() {
        let transport =
            MockTransport::new(["a".to_string(), "b".to_string(), "[DONE]".to_string()]);
        let mut seen = Vec::new();
        let mut sink = |data: &str| {
            seen.push(data.to_string());
            Ok(data == "[DONE]")
        };
        transport
            .stream(
                TransportRequest {
                    url: "x".into(),
                    api_key: "k".into(),
                    body: "{}".into(),
                },
                &mut sink,
            )
            .await
            .unwrap();
        assert_eq!(seen, vec!["a", "b", "[DONE]"]);
    }
}
