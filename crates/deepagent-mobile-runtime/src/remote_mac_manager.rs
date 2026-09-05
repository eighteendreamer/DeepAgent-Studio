use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use deepagent_mobile_core::{
    DeviceCapabilities, DeviceConnection, DeviceKind, DeviceState, MobileDevice, MobilePlatform,
};
use deepagent_mobile_protocol::{
    RemoteMacConfig, RemoteMacHealth, RemoteMacMethod, RemoteMacRequest, RemoteMacResponse,
    RemoteMacState,
};

use crate::device_registry::DeviceRegistry;
use crate::remote_mac::{RemoteMacSession, RemoteTransport, RemoteTransportError};

/// Factory that creates a `RemoteTransport` for a given config.
///
/// Implementations determine the actual transport mechanism (SSH, fake, etc.).
/// The manager calls this when connecting a new config.
pub trait TransportFactory: Send + Sync {
    fn create(&self, config: &RemoteMacConfig) -> Box<dyn RemoteTransport>;
}

/// Errors from RemoteMacManager operations.
#[derive(Debug, Clone)]
pub enum RemoteMacManagerError {
    /// Config already registered.
    ConfigAlreadyExists { config_id: String },
    /// Config not found.
    ConfigNotFound { config_id: String },
    /// Transport error from the underlying session.
    Transport(RemoteTransportError),
}

impl std::fmt::Display for RemoteMacManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConfigAlreadyExists { config_id } => {
                write!(f, "config '{config_id}' already registered")
            }
            Self::ConfigNotFound { config_id } => write!(f, "config '{config_id}' not found"),
            Self::Transport(e) => write!(f, "transport error: {e}"),
        }
    }
}

impl std::error::Error for RemoteMacManagerError {}

impl From<RemoteTransportError> for RemoteMacManagerError {
    fn from(e: RemoteTransportError) -> Self {
        Self::Transport(e)
    }
}

/// Aggregated health across all connected Remote Macs.
#[derive(Debug, Clone)]
pub struct AggregatedHealth {
    pub total_macs: usize,
    pub connected_macs: usize,
    pub healthy_macs: usize,
    pub per_mac: Vec<MacHealthEntry>,
}

/// Health entry for a single Remote Mac.
#[derive(Debug, Clone)]
pub struct MacHealthEntry {
    pub config_id: String,
    pub state: RemoteMacState,
    pub health: Option<RemoteMacHealth>,
}

/// Manages multiple Remote Mac connections and integrates remote devices into
/// the DeviceRegistry.
///
/// The manager is the orchestration layer above `RemoteMacSession`. It:
/// - Maintains a registry of configs and their active sessions
/// - Routes requests to the correct session by config_id
/// - Discovers remote devices and registers them in DeviceRegistry
/// - Aggregates health across all connections
pub struct RemoteMacManager {
    inner: Arc<Mutex<ManagerInner>>,
}

struct ManagerInner {
    sessions: HashMap<String, RemoteMacSession>,
    configs: HashMap<String, RemoteMacConfig>,
    registry: DeviceRegistry,
    factory: Box<dyn TransportFactory>,
}

impl RemoteMacManager {
    pub fn new(registry: DeviceRegistry, factory: Box<dyn TransportFactory>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ManagerInner {
                sessions: HashMap::new(),
                configs: HashMap::new(),
                registry,
                factory,
            })),
        }
    }

    /// Register a Remote Mac config. Does not connect yet.
    pub async fn add_config(&self, config: RemoteMacConfig) -> Result<(), RemoteMacManagerError> {
        let mut inner = self.inner.lock().await;
        if inner.configs.contains_key(&config.config_id) {
            return Err(RemoteMacManagerError::ConfigAlreadyExists {
                config_id: config.config_id.clone(),
            });
        }
        tracing::info!(config_id = %config.config_id, host = %config.host, "remote mac config added");
        inner.configs.insert(config.config_id.clone(), config);
        Ok(())
    }

    /// Remove a Remote Mac config. Disconnects if connected.
    pub async fn remove_config(&self, config_id: &str) -> Result<(), RemoteMacManagerError> {
        let mut inner = self.inner.lock().await;
        if inner.configs.remove(config_id).is_none() {
            return Err(RemoteMacManagerError::ConfigNotFound {
                config_id: config_id.to_string(),
            });
        }
        if let Some(session) = inner.sessions.remove(config_id) {
            session.cancel_all();
            let _ = session.disconnect().await;
        }
        tracing::info!(config_id = %config_id, "remote mac config removed");
        Ok(())
    }

    /// Connect to a Remote Mac by config_id.
    pub async fn connect(&self, config_id: &str) -> Result<(), RemoteMacManagerError> {
        let mut inner = self.inner.lock().await;
        let config = inner
            .configs
            .get(config_id)
            .ok_or_else(|| RemoteMacManagerError::ConfigNotFound {
                config_id: config_id.to_string(),
            })?
            .clone();

        if inner.sessions.contains_key(config_id) {
            return Ok(());
        }

        let transport = inner.factory.create(&config);
        let session = RemoteMacSession::new(config.clone(), transport);
        session.connect().await?;

        tracing::info!(config_id = %config_id, "remote mac connected");
        inner.sessions.insert(config_id.to_string(), session);
        Ok(())
    }

    /// Disconnect from a Remote Mac by config_id.
    pub async fn disconnect(&self, config_id: &str) -> Result<(), RemoteMacManagerError> {
        let mut inner = self.inner.lock().await;
        let session = inner.sessions.remove(config_id).ok_or_else(|| {
            RemoteMacManagerError::ConfigNotFound {
                config_id: config_id.to_string(),
            }
        })?;
        session.cancel_all();
        session.disconnect().await?;
        tracing::info!(config_id = %config_id, "remote mac disconnected");
        Ok(())
    }

    /// Send a request to a specific Remote Mac.
    pub async fn send_request(
        &self,
        config_id: &str,
        request: &RemoteMacRequest,
    ) -> Result<RemoteMacResponse, RemoteMacManagerError> {
        let inner = self.inner.lock().await;
        let session =
            inner
                .sessions
                .get(config_id)
                .ok_or_else(|| RemoteMacManagerError::ConfigNotFound {
                    config_id: config_id.to_string(),
                })?;
        Ok(session.send_request(request).await?)
    }

    /// Get health for a single Remote Mac.
    pub async fn health(&self, config_id: &str) -> Result<RemoteMacHealth, RemoteMacManagerError> {
        let inner = self.inner.lock().await;
        let session =
            inner
                .sessions
                .get(config_id)
                .ok_or_else(|| RemoteMacManagerError::ConfigNotFound {
                    config_id: config_id.to_string(),
                })?;
        Ok(session.health_probe().await?)
    }

    /// Get aggregated health across all Remote Macs.
    pub async fn aggregated_health(&self) -> AggregatedHealth {
        let inner = self.inner.lock().await;
        let mut per_mac = Vec::new();
        let mut connected = 0;
        let mut healthy = 0;

        for (config_id, config) in &inner.configs {
            let (state, health) = if let Some(session) = inner.sessions.get(config_id) {
                let state = session.state().await;
                let health = session.health_probe().await.ok();
                if matches!(state, RemoteMacState::Connected) {
                    connected += 1;
                }
                if let Some(h) = &health {
                    if h.healthy {
                        healthy += 1;
                    }
                }
                (state, health)
            } else {
                (RemoteMacState::Disconnected, None)
            };

            per_mac.push(MacHealthEntry {
                config_id: config_id.clone(),
                state,
                health,
            });
            let _ = config;
        }

        AggregatedHealth {
            total_macs: inner.configs.len(),
            connected_macs: connected,
            healthy_macs: healthy,
            per_mac,
        }
    }

    /// Discover remote devices from a connected Mac and register them in the
    /// DeviceRegistry.
    ///
    /// Sends a ListDevices request to the remote agent and registers each
    /// returned device with `DeviceConnection::Remote { host_id }`.
    pub async fn discover_remote_devices(
        &self,
        config_id: &str,
    ) -> Result<Vec<MobileDevice>, RemoteMacManagerError> {
        let inner = self.inner.lock().await;
        let session =
            inner
                .sessions
                .get(config_id)
                .ok_or_else(|| RemoteMacManagerError::ConfigNotFound {
                    config_id: config_id.to_string(),
                })?;

        let request = RemoteMacRequest {
            request_id: format!("discover-{config_id}"),
            protocol_version: 1,
            device_id: String::new(),
            deadline_ms: 10_000,
            method: RemoteMacMethod::ListDevices,
        };

        let response = session.send_request(&request).await?;
        let devices = parse_remote_devices(&response, config_id);

        for device in &devices {
            inner.registry.upsert(device.clone()).await;
        }

        tracing::info!(
            config_id = %config_id,
            device_count = devices.len(),
            "remote devices discovered"
        );

        Ok(devices)
    }

    /// List all registered config IDs.
    pub async fn list_configs(&self) -> Vec<String> {
        let inner = self.inner.lock().await;
        inner.configs.keys().cloned().collect()
    }

    /// Get the connection state for a specific config.
    pub async fn state(&self, config_id: &str) -> Result<RemoteMacState, RemoteMacManagerError> {
        let inner = self.inner.lock().await;
        if let Some(session) = inner.sessions.get(config_id) {
            Ok(session.state().await)
        } else if inner.configs.contains_key(config_id) {
            Ok(RemoteMacState::Disconnected)
        } else {
            Err(RemoteMacManagerError::ConfigNotFound {
                config_id: config_id.to_string(),
            })
        }
    }
}

/// Parse remote devices from a ListDevices response payload.
///
/// The payload is expected to be a JSON array of device descriptors. Each
/// device gets `DeviceConnection::Remote { host_id: config_id }`.
fn parse_remote_devices(response: &RemoteMacResponse, config_id: &str) -> Vec<MobileDevice> {
    if !response.success {
        return vec![];
    }

    let Some(payload) = &response.payload else {
        return vec![];
    };

    let Ok(devices) = serde_json::from_str::<Vec<RemoteDeviceDescriptor>>(payload) else {
        return vec![];
    };

    devices
        .into_iter()
        .map(|desc| {
            let platform = match desc.platform.as_str() {
                "android" => MobilePlatform::Android,
                _ => MobilePlatform::Ios,
            };
            let kind = match desc.kind.as_str() {
                "emulator" => DeviceKind::Emulator,
                "simulator" => DeviceKind::Simulator,
                _ => DeviceKind::Physical,
            };
            MobileDevice {
                id: format!("remote-{config_id}-{}", desc.device_id),
                name: desc.name,
                platform,
                kind,
                connection: DeviceConnection::Remote {
                    host_id: config_id.to_string(),
                },
                state: DeviceState::Ready,
                os_version: desc.os_version,
                capabilities: DeviceCapabilities {
                    screenshot: true,
                    ui_tree: true,
                    input: true,
                    logs: true,
                    install: true,
                    network_inspection: false,
                },
            }
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct RemoteDeviceDescriptor {
    device_id: String,
    name: String,
    platform: String,
    kind: String,
    os_version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_mac::FakeRemoteTransport;
    use deepagent_mobile_protocol::RemoteMacResponse;

    struct FakeFactory;

    impl TransportFactory for FakeFactory {
        fn create(&self, _config: &RemoteMacConfig) -> Box<dyn RemoteTransport> {
            Box::new(FakeRemoteTransport::new())
        }
    }

    struct HealthyFactory;

    impl TransportFactory for HealthyFactory {
        fn create(&self, _config: &RemoteMacConfig) -> Box<dyn RemoteTransport> {
            let transport = FakeRemoteTransport::with_initial_state(
                RemoteMacHealth {
                    healthy: true,
                    agent_version: Some("1.0.0".into()),
                    simulators_available: 2,
                    usb_devices_available: 1,
                    last_probe_ms: 1000,
                    diagnostics: vec![],
                },
                None,
            );
            Box::new(transport)
        }
    }

    fn test_config(id: &str) -> RemoteMacConfig {
        RemoteMacConfig {
            config_id: id.into(),
            host: "192.168.1.100".into(),
            port: 22,
            username: "dev".into(),
            credential_ref: "keychain:test".into(),
            agent_path: "/usr/local/bin/agent".into(),
            connect_timeout_ms: 5_000,
            health_probe_interval_ms: 2_000,
        }
    }

    fn test_registry() -> DeviceRegistry {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        DeviceRegistry::new(tx)
    }

    #[tokio::test]
    async fn add_and_list_configs() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        mgr.add_config(test_config("mac-2")).await.unwrap();

        let configs = mgr.list_configs().await;
        assert_eq!(configs.len(), 2);
    }

    #[tokio::test]
    async fn add_duplicate_config_rejected() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        let result = mgr.add_config(test_config("mac-1")).await;
        assert!(matches!(
            result,
            Err(RemoteMacManagerError::ConfigAlreadyExists { .. })
        ));
    }

    #[tokio::test]
    async fn remove_config_disconnects() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        mgr.connect("mac-1").await.unwrap();
        assert_eq!(mgr.state("mac-1").await.unwrap(), RemoteMacState::Connected);

        mgr.remove_config("mac-1").await.unwrap();
        let result = mgr.state("mac-1").await;
        assert!(matches!(
            result,
            Err(RemoteMacManagerError::ConfigNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn remove_unknown_config_fails() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        let result = mgr.remove_config("nonexistent").await;
        assert!(matches!(
            result,
            Err(RemoteMacManagerError::ConfigNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn connect_and_send_request() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        mgr.connect("mac-1").await.unwrap();

        let request = RemoteMacRequest {
            request_id: "req-1".into(),
            protocol_version: 1,
            device_id: "sim-1".into(),
            deadline_ms: 30_000,
            method: RemoteMacMethod::Screenshot,
        };
        let resp = mgr.send_request("mac-1", &request).await.unwrap();
        assert!(resp.success);
    }

    #[tokio::test]
    async fn send_request_to_disconnected_fails() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();

        let request = RemoteMacRequest {
            request_id: "req-1".into(),
            protocol_version: 1,
            device_id: "sim-1".into(),
            deadline_ms: 30_000,
            method: RemoteMacMethod::Screenshot,
        };
        let result = mgr.send_request("mac-1", &request).await;
        assert!(matches!(
            result,
            Err(RemoteMacManagerError::ConfigNotFound { .. })
        ));
    }

    #[tokio::test]
    async fn aggregated_health_all_disconnected() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        mgr.add_config(test_config("mac-2")).await.unwrap();

        let health = mgr.aggregated_health().await;
        assert_eq!(health.total_macs, 2);
        assert_eq!(health.connected_macs, 0);
        assert_eq!(health.healthy_macs, 0);
    }

    #[tokio::test]
    async fn aggregated_health_connected() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(HealthyFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        mgr.connect("mac-1").await.unwrap();

        let health = mgr.aggregated_health().await;
        assert_eq!(health.total_macs, 1);
        assert_eq!(health.connected_macs, 1);
        assert_eq!(health.healthy_macs, 1);
    }

    #[tokio::test]
    async fn discover_remote_devices_registers_in_registry() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let registry = DeviceRegistry::new(tx);

        let payload = serde_json::to_string(&vec![
            serde_json::json!({
                "device_id": "sim-1",
                "name": "iPhone 15",
                "platform": "ios",
                "kind": "simulator",
                "os_version": "17.0"
            }),
            serde_json::json!({
                "device_id": "sim-2",
                "name": "iPad Pro",
                "platform": "ios",
                "kind": "simulator",
                "os_version": "17.0"
            }),
        ])
        .unwrap();

        let response = RemoteMacResponse {
            request_id: "discover-mac-1".into(),
            success: true,
            error: None,
            payload: Some(payload),
            artifact_ids: vec![],
        };

        struct ResponseFactory(RemoteMacResponse);
        impl TransportFactory for ResponseFactory {
            fn create(&self, _config: &RemoteMacConfig) -> Box<dyn RemoteTransport> {
                let transport = FakeRemoteTransport::with_initial_state(
                    RemoteMacHealth {
                        healthy: false,
                        agent_version: None,
                        simulators_available: 0,
                        usb_devices_available: 0,
                        last_probe_ms: 0,
                        diagnostics: vec![],
                    },
                    Some(self.0.clone()),
                );
                Box::new(transport)
            }
        }

        let mgr = RemoteMacManager::new(registry, Box::new(ResponseFactory(response)));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        mgr.connect("mac-1").await.unwrap();

        let devices = mgr.discover_remote_devices("mac-1").await.unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "iPhone 15");
        assert!(matches!(
            devices[0].connection,
            DeviceConnection::Remote { ref host_id } if host_id == "mac-1"
        ));
        assert_eq!(devices[1].name, "iPad Pro");
        assert!(matches!(devices[1].kind, DeviceKind::Simulator));
    }

    #[tokio::test]
    async fn state_returns_disconnected_for_unconnected_config() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        mgr.add_config(test_config("mac-1")).await.unwrap();
        assert_eq!(
            mgr.state("mac-1").await.unwrap(),
            RemoteMacState::Disconnected
        );
    }

    #[tokio::test]
    async fn connect_unknown_config_fails() {
        let mgr = RemoteMacManager::new(test_registry(), Box::new(FakeFactory));
        let result = mgr.connect("nonexistent").await;
        assert!(matches!(
            result,
            Err(RemoteMacManagerError::ConfigNotFound { .. })
        ));
    }

    #[test]
    fn parse_remote_devices_empty_payload() {
        let response = RemoteMacResponse {
            request_id: "r".into(),
            success: true,
            error: None,
            payload: None,
            artifact_ids: vec![],
        };
        let devices = parse_remote_devices(&response, "mac-1");
        assert!(devices.is_empty());
    }

    #[test]
    fn parse_remote_devices_failed_response() {
        let response = RemoteMacResponse {
            request_id: "r".into(),
            success: false,
            error: Some("agent crashed".into()),
            payload: None,
            artifact_ids: vec![],
        };
        let devices = parse_remote_devices(&response, "mac-1");
        assert!(devices.is_empty());
    }

    #[test]
    fn manager_error_display() {
        let err = RemoteMacManagerError::ConfigNotFound {
            config_id: "mac-1".into(),
        };
        assert!(format!("{err}").contains("mac-1"));
    }
}
