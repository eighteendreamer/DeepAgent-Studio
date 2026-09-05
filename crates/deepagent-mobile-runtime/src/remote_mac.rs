use async_trait::async_trait;
use deepagent_mobile_protocol::{
    RemoteMacConfig, RemoteMacHealth, RemoteMacRequest, RemoteMacResponse, RemoteMacState,
};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Transport abstraction for Remote Mac communication.
///
/// Implementations wrap SSH or other transport mechanisms. The trait is
/// designed for testability: a `FakeRemoteTransport` is provided for CI.
#[async_trait]
pub trait RemoteTransport: Send + Sync {
    /// Connect to the remote Mac.
    async fn connect(&self, config: &RemoteMacConfig) -> Result<(), RemoteTransportError>;

    /// Disconnect from the remote Mac.
    async fn disconnect(&self) -> Result<(), RemoteTransportError>;

    /// Send a request and wait for a response.
    ///
    /// The `cancel` token allows either side to cancel the in-flight request.
    /// Implementations must check `cancel.is_cancelled()` and return promptly.
    async fn send_request(
        &self,
        request: &RemoteMacRequest,
        cancel: &CancellationToken,
    ) -> Result<RemoteMacResponse, RemoteTransportError>;

    /// Run a health probe. Returns the current health status.
    async fn health_probe(&self) -> Result<RemoteMacHealth, RemoteTransportError>;

    /// Get the current connection state.
    async fn state(&self) -> RemoteMacState;
}

/// Errors from remote transport operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteTransportError {
    /// SSH connection failed.
    ConnectionFailed { message: String },
    /// SSH connection lost during operation.
    ConnectionLost { message: String },
    /// Request timed out.
    Timeout { request_id: String },
    /// Request was cancelled.
    Cancelled { request_id: String },
    /// Protocol version mismatch.
    ProtocolMismatch { expected: u32, actual: u32 },
    /// Agent bootstrap failed.
    BootstrapFailed { message: String },
    /// Transport not available (e.g., no SSH client).
    NotAvailable { message: String },
}

impl std::fmt::Display for RemoteTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectionFailed { message } => write!(f, "connection failed: {message}"),
            Self::ConnectionLost { message } => write!(f, "connection lost: {message}"),
            Self::Timeout { request_id } => write!(f, "request {request_id} timed out"),
            Self::Cancelled { request_id } => write!(f, "request {request_id} cancelled"),
            Self::ProtocolMismatch { expected, actual } => {
                write!(f, "protocol mismatch: expected {expected}, got {actual}")
            }
            Self::BootstrapFailed { message } => write!(f, "bootstrap failed: {message}"),
            Self::NotAvailable { message } => write!(f, "transport not available: {message}"),
        }
    }
}

impl std::error::Error for RemoteTransportError {}

/// Fake remote transport for testing without a real Mac.
///
/// Returns configurable responses and simulates connection lifecycle.
pub struct FakeRemoteTransport {
    state: Arc<Mutex<RemoteMacState>>,
    health: Arc<Mutex<RemoteMacHealth>>,
    response: Arc<Mutex<Option<RemoteMacResponse>>>,
}

impl FakeRemoteTransport {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RemoteMacState::Disconnected)),
            health: Arc::new(Mutex::new(RemoteMacHealth {
                healthy: false,
                agent_version: None,
                simulators_available: 0,
                usb_devices_available: 0,
                last_probe_ms: 0,
                diagnostics: vec!["fake transport".into()],
            })),
            response: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a transport pre-configured with initial health and response.
    ///
    /// This constructor is synchronous and safe to call from async contexts.
    pub fn with_initial_state(
        health: RemoteMacHealth,
        response: Option<RemoteMacResponse>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RemoteMacState::Disconnected)),
            health: Arc::new(Mutex::new(health)),
            response: Arc::new(Mutex::new(response)),
        }
    }

    pub async fn set_state(&self, state: RemoteMacState) {
        *self.state.lock().await = state;
    }

    pub async fn set_health(&self, health: RemoteMacHealth) {
        *self.health.lock().await = health;
    }

    pub async fn set_response(&self, response: RemoteMacResponse) {
        *self.response.lock().await = Some(response);
    }
}

impl Default for FakeRemoteTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RemoteTransport for FakeRemoteTransport {
    async fn connect(&self, _config: &RemoteMacConfig) -> Result<(), RemoteTransportError> {
        *self.state.lock().await = RemoteMacState::Connected;
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), RemoteTransportError> {
        *self.state.lock().await = RemoteMacState::Disconnected;
        Ok(())
    }

    async fn send_request(
        &self,
        request: &RemoteMacRequest,
        cancel: &CancellationToken,
    ) -> Result<RemoteMacResponse, RemoteTransportError> {
        if cancel.is_cancelled() {
            return Err(RemoteTransportError::Cancelled {
                request_id: request.request_id.clone(),
            });
        }

        let state = self.state.lock().await;
        if *state != RemoteMacState::Connected {
            return Err(RemoteTransportError::ConnectionFailed {
                message: "not connected".into(),
            });
        }
        drop(state);

        let response = self.response.lock().await;
        match &*response {
            Some(resp) => Ok(resp.clone()),
            None => Ok(RemoteMacResponse {
                request_id: request.request_id.clone(),
                success: true,
                error: None,
                payload: None,
                artifact_ids: vec![],
            }),
        }
    }

    async fn health_probe(&self) -> Result<RemoteMacHealth, RemoteTransportError> {
        Ok(self.health.lock().await.clone())
    }

    async fn state(&self) -> RemoteMacState {
        *self.state.lock().await
    }
}

/// Remote Mac session manager.
///
/// Manages the lifecycle of a connection to a Remote Mac agent, including
/// health probing and bidirectional cancellation.
pub struct RemoteMacSession {
    transport: Box<dyn RemoteTransport>,
    config: RemoteMacConfig,
    cancel: CancellationToken,
}

impl RemoteMacSession {
    pub fn new(config: RemoteMacConfig, transport: Box<dyn RemoteTransport>) -> Self {
        Self {
            transport,
            config,
            cancel: CancellationToken::new(),
        }
    }

    /// Connect to the remote Mac and bootstrap the agent.
    pub async fn connect(&self) -> Result<(), RemoteTransportError> {
        deepagent_mobile_protocol::validate_config_no_plaintext_secret(&self.config)
            .map_err(|e| RemoteTransportError::ConnectionFailed { message: e })?;
        self.transport.connect(&self.config).await
    }

    /// Disconnect from the remote Mac.
    pub async fn disconnect(&self) -> Result<(), RemoteTransportError> {
        self.cancel.cancel();
        self.transport.disconnect().await
    }

    /// Send a request to the remote agent.
    pub async fn send_request(
        &self,
        request: &RemoteMacRequest,
    ) -> Result<RemoteMacResponse, RemoteTransportError> {
        self.transport.send_request(request, &self.cancel).await
    }

    /// Run a health probe.
    pub async fn health_probe(&self) -> Result<RemoteMacHealth, RemoteTransportError> {
        self.transport.health_probe().await
    }

    /// Get the current connection state.
    pub async fn state(&self) -> RemoteMacState {
        self.transport.state().await
    }

    /// Cancel all in-flight requests.
    pub fn cancel_all(&self) {
        self.cancel.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_mobile_protocol::RemoteMacMethod;

    fn test_config() -> RemoteMacConfig {
        RemoteMacConfig {
            config_id: "mac-test".into(),
            host: "192.168.1.100".into(),
            port: 22,
            username: "dev".into(),
            credential_ref: "keychain:test".into(),
            agent_path: "/usr/local/bin/agent".into(),
            connect_timeout_ms: 5_000,
            health_probe_interval_ms: 2_000,
        }
    }

    fn test_request() -> RemoteMacRequest {
        RemoteMacRequest {
            request_id: "req-1".into(),
            protocol_version: 1,
            device_id: "sim-1".into(),
            deadline_ms: 30_000,
            method: RemoteMacMethod::Screenshot,
        }
    }

    #[tokio::test]
    async fn fake_transport_connect_disconnect() {
        let transport = FakeRemoteTransport::new();
        assert_eq!(transport.state().await, RemoteMacState::Disconnected);

        transport.connect(&test_config()).await.unwrap();
        assert_eq!(transport.state().await, RemoteMacState::Connected);

        transport.disconnect().await.unwrap();
        assert_eq!(transport.state().await, RemoteMacState::Disconnected);
    }

    #[tokio::test]
    async fn fake_transport_send_request_when_connected() {
        let transport = FakeRemoteTransport::new();
        transport.connect(&test_config()).await.unwrap();

        let cancel = CancellationToken::new();
        let resp = transport
            .send_request(&test_request(), &cancel)
            .await
            .unwrap();
        assert!(resp.success);
        assert_eq!(resp.request_id, "req-1");
    }

    #[tokio::test]
    async fn fake_transport_send_request_when_disconnected_fails() {
        let transport = FakeRemoteTransport::new();
        let cancel = CancellationToken::new();
        let result = transport.send_request(&test_request(), &cancel).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_transport_cancel_before_send() {
        let transport = FakeRemoteTransport::new();
        transport.connect(&test_config()).await.unwrap();

        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = transport.send_request(&test_request(), &cancel).await;
        assert!(matches!(
            result,
            Err(RemoteTransportError::Cancelled { .. })
        ));
    }

    #[tokio::test]
    async fn fake_transport_health_probe() {
        let transport = FakeRemoteTransport::new();
        let health = transport.health_probe().await.unwrap();
        assert!(!health.healthy);
    }

    #[tokio::test]
    async fn fake_transport_custom_health() {
        let transport = FakeRemoteTransport::new();
        transport
            .set_health(RemoteMacHealth {
                healthy: true,
                agent_version: Some("1.0.0".into()),
                simulators_available: 3,
                usb_devices_available: 1,
                last_probe_ms: 1000,
                diagnostics: vec![],
            })
            .await;

        let health = transport.health_probe().await.unwrap();
        assert!(health.healthy);
        assert_eq!(health.simulators_available, 3);
    }

    #[tokio::test]
    async fn session_connect_validates_config() {
        let transport = FakeRemoteTransport::new();
        let bad_config = RemoteMacConfig {
            credential_ref: "".into(),
            ..test_config()
        };
        let session = RemoteMacSession::new(bad_config, Box::new(transport));
        let result = session.connect().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn session_connect_and_send() {
        let transport = FakeRemoteTransport::new();
        let session = RemoteMacSession::new(test_config(), Box::new(transport));
        session.connect().await.unwrap();

        let resp = session.send_request(&test_request()).await.unwrap();
        assert!(resp.success);
    }

    #[tokio::test]
    async fn session_cancel_all() {
        let transport = FakeRemoteTransport::new();
        let session = RemoteMacSession::new(test_config(), Box::new(transport));
        session.connect().await.unwrap();
        session.cancel_all();

        let result = session.send_request(&test_request()).await;
        assert!(matches!(
            result,
            Err(RemoteTransportError::Cancelled { .. })
        ));
    }

    #[test]
    fn transport_error_display() {
        let err = RemoteTransportError::Timeout {
            request_id: "req-1".into(),
        };
        assert!(format!("{err}").contains("req-1"));
    }

    #[test]
    fn transport_error_protocol_mismatch() {
        let err = RemoteTransportError::ProtocolMismatch {
            expected: 1,
            actual: 2,
        };
        assert!(format!("{err}").contains("expected 1"));
    }
}
