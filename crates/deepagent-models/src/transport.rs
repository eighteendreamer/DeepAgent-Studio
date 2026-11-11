//! Transport abstraction.
//!
//! The model client is decoupled from any concrete HTTP stack via the
//! [`HttpTransport`] trait. This keeps all the streaming-assembly logic
//! testable offline (via [`MockTransport`]) and lets the real `reqwest`-based
//! transport live behind the optional `http` feature.

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_tools::ToolExecutionContext;

/// An outbound Responses request at the transport level (already serialized).
#[derive(Debug, Clone)]
pub struct TransportRequest {
    /// Full endpoint URL.
    pub url: String,
    /// Bearer API key.
    pub api_key: String,
    /// JSON request body.
    pub body: String,
}

/// One authenticated DeepSeek Files API upload of original image bytes.
#[derive(Clone)]
pub struct FileUploadRequest {
    pub url: String,
    pub api_key: String,
    pub filename: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Provider file metadata needed for the bounded request-local cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedFile {
    pub id: String,
    pub expires_at: Option<i64>,
}

/// A transport that can stream an SSE response and perform simple GET requests.
///
/// Implementations push each decoded SSE `data:` payload to `on_event`. The
/// boxed closure returns `Ok(true)` to signal the caller wants to stop early
/// (e.g. a Responses terminal event was seen).
#[async_trait]
pub trait HttpTransport: Send + Sync {
    fn supports_file_upload(&self) -> bool {
        false
    }
    /// Perform the request and drive `sink` with each SSE payload string.
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()>;

    /// Cancel-aware streaming variant. Dropping the in-flight stream future is
    /// the common local boundary; transports may override it to also interrupt
    /// their socket read. This keeps test transports and embedders cooperative
    /// without requiring a second cancellation API.
    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        if cancel.load(Ordering::Acquire) {
            return Err(CoreError::other("request cancelled"));
        }
        let execution = ToolExecutionContext::new(cancel);
        tokio::select! {
            result = self.stream(request, sink) => result,
            _ = execution.cancelled() => Err(CoreError::other("request cancelled")),
        }
    }

    /// Perform an authenticated `GET` and return the raw JSON body. Used for
    /// non-streaming endpoints like `GET /models` (model discovery).
    ///
    /// The default implementation errors so transports that only stream are not
    /// forced to implement it; the real HTTP transport and the mock override it.
    async fn get_json(&self, _url: &str, _api_key: &str) -> Result<String> {
        Err(deepagent_core::error::CoreError::other(
            "this transport does not support GET requests",
        ))
    }

    /// Cancel-aware variant for management requests such as model discovery
    /// and balance lookup. Existing transports remain source-compatible: the
    /// default checks an already-cancelled request and otherwise delegates to
    /// [`Self::get_json`]. Real transports should override this method so an
    /// in-flight header or body read can be interrupted as well.
    async fn get_json_cancelled(
        &self,
        url: &str,
        api_key: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<String> {
        if cancel.load(Ordering::Relaxed) {
            return Err(CoreError::other("request cancelled"));
        }
        self.get_json(url, api_key).await
    }

    /// POST one original image to the provider's Files API. Existing test and
    /// embedding transports remain compatible and fail closed until they opt in.
    async fn upload_file(&self, _request: FileUploadRequest) -> Result<UploadedFile> {
        Err(CoreError::other(
            "this transport does not support file upload",
        ))
    }

    /// Cancel-aware upload shares the current model-attempt cancellation owner.
    async fn upload_file_cancelled(
        &self,
        request: FileUploadRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<UploadedFile> {
        if cancel.load(Ordering::Acquire) {
            return Err(CoreError::other("request cancelled"));
        }
        let execution = ToolExecutionContext::new(cancel);
        tokio::select! {
            result = self.upload_file(request) => result,
            _ = execution.cancelled() => Err(CoreError::other("request cancelled")),
        }
    }

    /// Delete only a provider file created by the current failed preparation.
    /// This is best effort compensation, not general history/retention cleanup.
    async fn delete_file(&self, _url: &str, _api_key: &str, _file_id: &str) -> Result<()> {
        Err(CoreError::other(
            "this transport does not support file deletion",
        ))
    }

    async fn delete_file_cancelled(
        &self,
        url: &str,
        api_key: &str,
        file_id: &str,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        if cancel.load(Ordering::Acquire) {
            return Err(CoreError::other("request cancelled"));
        }
        let execution = ToolExecutionContext::new(cancel);
        tokio::select! {
            result = self.delete_file(url, api_key, file_id) => result,
            _ = execution.cancelled() => Err(CoreError::other("request cancelled")),
        }
    }
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
    /// Canned JSON body returned by [`HttpTransport::get_json`].
    pub get_response: Option<String>,
}

impl MockTransport {
    /// Build from a list of payload strings.
    pub fn new(events: impl IntoIterator<Item = String>) -> Self {
        Self {
            events: events.into_iter().collect(),
            get_response: None,
        }
    }

    /// Build a transport that answers `get_json` with `body` (for discovery
    /// tests). Streaming events default to empty.
    pub fn with_get_json(body: impl Into<String>) -> Self {
        Self {
            events: Vec::new(),
            get_response: Some(body.into()),
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

    async fn get_json(&self, _url: &str, _api_key: &str) -> Result<String> {
        self.get_response.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other("mock transport has no canned GET response")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    #[tokio::test]
    async fn mock_transport_replays_events() {
        let terminal =
            r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string();
        let transport = MockTransport::new(["a".to_string(), "b".to_string(), terminal.clone()]);
        let mut seen = Vec::new();
        let mut sink = |data: &str| {
            seen.push(data.to_string());
            Ok(data.contains(r#""type":"response.completed""#))
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
        assert_eq!(seen, vec!["a", "b", terminal.as_str()]);
    }

    struct NonCooperativeTransport {
        entered: tokio::sync::Notify,
        dropped: Arc<AtomicBool>,
    }

    struct StreamGuard(Arc<AtomicBool>);

    impl Drop for StreamGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[async_trait]
    impl HttpTransport for NonCooperativeTransport {
        async fn stream(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
        ) -> Result<()> {
            let _guard = StreamGuard(self.dropped.clone());
            self.entered.notify_one();
            std::future::pending::<()>().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_cancel_aware_transport_drops_a_pending_stream() {
        let transport = NonCooperativeTransport {
            entered: tokio::sync::Notify::new(),
            dropped: Arc::new(AtomicBool::new(false)),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let request = TransportRequest {
            url: "offline://test".into(),
            api_key: String::new(),
            body: "{}".into(),
        };
        let mut sink = |_data: &str| Ok(false);
        let stream = transport.stream_cancelled(request, &mut sink, cancel.clone());
        let stop = async {
            transport.entered.notified().await;
            cancel.store(true, Ordering::Release);
        };
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(stream, stop)
        })
        .await
        .expect("cancel-aware stream must settle");
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(transport.dropped.load(Ordering::Acquire));
    }
}
