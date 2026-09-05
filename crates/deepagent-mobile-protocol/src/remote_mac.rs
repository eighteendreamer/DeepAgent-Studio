use serde::{Deserialize, Serialize};

/// Configuration for connecting to a Remote Mac agent.
///
/// Credentials are **never** stored in plaintext. The `credential_ref` points
/// to an entry in the OS keychain or encrypted secret store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMacConfig {
    /// Unique identifier for this remote configuration.
    pub config_id: String,
    /// Mac hostname or IP address.
    pub host: String,
    /// SSH port (default 22).
    pub port: u16,
    /// SSH username.
    pub username: String,
    /// Reference to credentials in the OS keychain or encrypted store.
    /// Never contains plaintext passwords or private keys.
    pub credential_ref: String,
    /// Path to the remote agent binary on the Mac.
    pub agent_path: String,
    /// Connection timeout in milliseconds.
    pub connect_timeout_ms: u64,
    /// Health probe interval in milliseconds.
    pub health_probe_interval_ms: u64,
}

impl Default for RemoteMacConfig {
    fn default() -> Self {
        Self {
            config_id: String::new(),
            host: String::new(),
            port: 22,
            username: String::new(),
            credential_ref: String::new(),
            agent_path: "/usr/local/bin/deepagent-mac-agent".into(),
            connect_timeout_ms: 10_000,
            health_probe_interval_ms: 5_000,
        }
    }
}

/// Connection state of a Remote Mac session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMacState {
    /// Not yet connected.
    Disconnected,
    /// SSH connection in progress.
    Connecting,
    /// Agent bootstrap in progress.
    Bootstrapping,
    /// Connected and healthy.
    Connected,
    /// Connection lost, attempting recovery.
    Reconnecting,
    /// Permanently failed.
    Failed,
}

/// Health status of a Remote Mac agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMacHealth {
    /// Whether the agent is reachable and responsive.
    pub healthy: bool,
    /// Agent version string.
    pub agent_version: Option<String>,
    /// Available Simulator devices on the Mac.
    pub simulators_available: u32,
    /// Connected USB iOS devices.
    pub usb_devices_available: u32,
    /// Last successful probe timestamp (ms since epoch).
    pub last_probe_ms: u64,
    /// Diagnostic messages from the last probe.
    pub diagnostics: Vec<String>,
}

/// A request sent to the Remote Mac agent.
///
/// Every request carries a `request_id` for bidirectional cancellation:
/// either side can cancel an in-flight request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMacRequest {
    /// Unique request ID. Used for cancellation and correlation.
    pub request_id: String,
    /// Protocol version for compatibility checking.
    pub protocol_version: u32,
    /// Target device ID on the Mac (simulator UDID or USB device UDID).
    pub device_id: String,
    /// Deadline in milliseconds from request creation.
    pub deadline_ms: u64,
    /// The operation to perform.
    pub method: RemoteMacMethod,
}

/// Methods supported by the Remote Mac agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMacMethod {
    /// List available devices (simulators + USB).
    ListDevices,
    /// Boot a simulator.
    BootSimulator { udid: String },
    /// Shutdown a simulator.
    ShutdownSimulator { udid: String },
    /// Install an app (artifact must be transferred first).
    InstallApp {
        artifact_id: String,
        bundle_id: String,
    },
    /// Launch an app.
    LaunchApp { bundle_id: String },
    /// Terminate an app.
    TerminateApp { bundle_id: String },
    /// Capture a screenshot.
    Screenshot,
    /// Capture UI hierarchy.
    UiSnapshot,
    /// Perform an input action.
    Input { action_json: String },
    /// Read device logs.
    ReadLogs { max_lines: u32 },
    /// Transfer an artifact to the Mac.
    TransferArtifact {
        artifact_id: String,
        size_bytes: u64,
    },
    /// Cancel an in-flight request.
    CancelRequest { target_request_id: String },
}

/// Response from the Remote Mac agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMacResponse {
    /// Correlates with the request's `request_id`.
    pub request_id: String,
    /// Whether the operation succeeded.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Response payload (method-specific).
    pub payload: Option<String>,
    /// Artifacts produced by the operation.
    pub artifact_ids: Vec<String>,
}

/// Events emitted by the Remote Mac session.
///
/// These are replayable and support cursor-based resumption after disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteMacEvent {
    /// SSH connection established.
    Connected {
        config_id: String,
        agent_version: String,
    },
    /// SSH connection lost.
    Disconnected { config_id: String, reason: String },
    /// Agent bootstrap completed.
    BootstrapComplete { config_id: String },
    /// Health probe result.
    HealthProbe {
        config_id: String,
        health: RemoteMacHealth,
    },
    /// Request sent to agent.
    RequestSent { request_id: String, method: String },
    /// Response received from agent.
    ResponseReceived { request_id: String, success: bool },
    /// Request cancelled (by either side).
    RequestCancelled {
        request_id: String,
        by: CancellationSource,
    },
    /// Artifact transfer started.
    ArtifactTransferStarted {
        artifact_id: String,
        size_bytes: u64,
    },
    /// Artifact transfer completed.
    ArtifactTransferCompleted { artifact_id: String },
    /// Remote device discovered.
    RemoteDeviceDiscovered {
        device_id: String,
        name: String,
        is_simulator: bool,
    },
    /// Remote device lost.
    RemoteDeviceLost { device_id: String, reason: String },
}

/// Source of a cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancellationSource {
    /// Cancelled by the Windows host.
    Host,
    /// Cancelled by the Remote Mac agent.
    Agent,
    /// Cancelled due to timeout.
    Timeout,
    /// Cancelled due to disconnect.
    Disconnect,
}

/// Validate that a config does not contain plaintext credentials.
pub fn validate_config_no_plaintext_secret(config: &RemoteMacConfig) -> Result<(), String> {
    if config.credential_ref.is_empty() {
        return Err("credential_ref must not be empty".into());
    }
    if config.credential_ref.contains("password") || config.credential_ref.contains("-----BEGIN") {
        return Err("credential_ref must be a keychain/store reference, not plaintext".into());
    }
    if config.host.is_empty() {
        return Err("host must not be empty".into());
    }
    if config.username.is_empty() {
        return Err("username must not be empty".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_serde() {
        let config = RemoteMacConfig {
            config_id: "mac-1".into(),
            host: "192.168.1.100".into(),
            port: 22,
            username: "dev".into(),
            credential_ref: "keychain:deepagent-mac-1".into(),
            agent_path: "/usr/local/bin/deepagent-mac-agent".into(),
            connect_timeout_ms: 10_000,
            health_probe_interval_ms: 5_000,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: RemoteMacConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }

    #[test]
    fn config_default() {
        let config = RemoteMacConfig::default();
        assert_eq!(config.port, 22);
        assert_eq!(config.connect_timeout_ms, 10_000);
    }

    #[test]
    fn remote_mac_state_serde() {
        let state = RemoteMacState::Connected;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"connected\"");
        let back: RemoteMacState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn health_serde() {
        let health = RemoteMacHealth {
            healthy: true,
            agent_version: Some("1.0.0".into()),
            simulators_available: 3,
            usb_devices_available: 1,
            last_probe_ms: 1000,
            diagnostics: vec![],
        };
        let json = serde_json::to_string(&health).unwrap();
        let back: RemoteMacHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(health, back);
    }

    #[test]
    fn request_serde() {
        let req = RemoteMacRequest {
            request_id: "req-1".into(),
            protocol_version: 1,
            device_id: "sim-1".into(),
            deadline_ms: 30_000,
            method: RemoteMacMethod::Screenshot,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: RemoteMacRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn response_serde() {
        let resp = RemoteMacResponse {
            request_id: "req-1".into(),
            success: true,
            error: None,
            payload: Some("data".into()),
            artifact_ids: vec!["art-1".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RemoteMacResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, back);
    }

    #[test]
    fn event_serde() {
        let event = RemoteMacEvent::Connected {
            config_id: "mac-1".into(),
            agent_version: "1.0.0".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"connected\""));
        let back: RemoteMacEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn event_disconnect_serde() {
        let event = RemoteMacEvent::Disconnected {
            config_id: "mac-1".into(),
            reason: "SSH timeout".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: RemoteMacEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn cancellation_source_serde() {
        let src = CancellationSource::Agent;
        let json = serde_json::to_string(&src).unwrap();
        assert_eq!(json, "\"agent\"");
        let back: CancellationSource = serde_json::from_str(&json).unwrap();
        assert_eq!(src, back);
    }

    #[test]
    fn validate_config_rejects_empty_credential_ref() {
        let config = RemoteMacConfig {
            config_id: "mac-1".into(),
            host: "192.168.1.100".into(),
            username: "dev".into(),
            credential_ref: "".into(),
            ..Default::default()
        };
        assert!(validate_config_no_plaintext_secret(&config).is_err());
    }

    #[test]
    fn validate_config_rejects_plaintext_password() {
        let config = RemoteMacConfig {
            config_id: "mac-1".into(),
            host: "192.168.1.100".into(),
            username: "dev".into(),
            credential_ref: "password:hunter2".into(),
            ..Default::default()
        };
        assert!(validate_config_no_plaintext_secret(&config).is_err());
    }

    #[test]
    fn validate_config_rejects_private_key() {
        let config = RemoteMacConfig {
            config_id: "mac-1".into(),
            host: "192.168.1.100".into(),
            username: "dev".into(),
            credential_ref: "-----BEGIN RSA PRIVATE KEY-----".into(),
            ..Default::default()
        };
        assert!(validate_config_no_plaintext_secret(&config).is_err());
    }

    #[test]
    fn validate_config_accepts_keychain_ref() {
        let config = RemoteMacConfig {
            config_id: "mac-1".into(),
            host: "192.168.1.100".into(),
            username: "dev".into(),
            credential_ref: "keychain:deepagent-mac-1".into(),
            ..Default::default()
        };
        assert!(validate_config_no_plaintext_secret(&config).is_ok());
    }

    #[test]
    fn validate_config_rejects_empty_host() {
        let config = RemoteMacConfig {
            config_id: "mac-1".into(),
            host: "".into(),
            username: "dev".into(),
            credential_ref: "keychain:mac-1".into(),
            ..Default::default()
        };
        assert!(validate_config_no_plaintext_secret(&config).is_err());
    }

    #[test]
    fn all_methods_serialize() {
        let methods = vec![
            RemoteMacMethod::ListDevices,
            RemoteMacMethod::BootSimulator { udid: "x".into() },
            RemoteMacMethod::ShutdownSimulator { udid: "x".into() },
            RemoteMacMethod::InstallApp {
                artifact_id: "a".into(),
                bundle_id: "b".into(),
            },
            RemoteMacMethod::LaunchApp {
                bundle_id: "b".into(),
            },
            RemoteMacMethod::TerminateApp {
                bundle_id: "b".into(),
            },
            RemoteMacMethod::Screenshot,
            RemoteMacMethod::UiSnapshot,
            RemoteMacMethod::Input {
                action_json: "{}".into(),
            },
            RemoteMacMethod::ReadLogs { max_lines: 100 },
            RemoteMacMethod::TransferArtifact {
                artifact_id: "a".into(),
                size_bytes: 1024,
            },
            RemoteMacMethod::CancelRequest {
                target_request_id: "r".into(),
            },
        ];
        for method in methods {
            let json = serde_json::to_string(&method).unwrap();
            let back: RemoteMacMethod = serde_json::from_str(&json).unwrap();
            assert_eq!(method, back);
        }
    }
}
