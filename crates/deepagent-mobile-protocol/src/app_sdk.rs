//! App SDK message protocol.
//!
//! Defines the wire-level messages exchanged between the in-app SDK and the
//! mobile runtime. The SDK runs inside the target app (Android/iOS/uni-app/
//! React Native/Compose/SwiftUI) and communicates with the runtime over a
//! local channel (WebSocket, Unix socket, or ADB forward).
//!
//! All messages are JSON-serializable and versioned.

use serde::{Deserialize, Serialize};

use crate::framework::{BusinessEvent, ComponentTree, FrameworkKind};

/// Protocol version for the App SDK bridge. Bump on breaking changes.
pub const APP_SDK_PROTOCOL_VERSION: u32 = 1;

/// Envelope for all messages between SDK and runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSdkEnvelope {
    /// Protocol version.
    pub version: u32,
    /// Message type tag.
    pub kind: AppSdkKind,
    /// Session ID (ties messages to a debug session).
    pub session_id: String,
    /// Device ID.
    pub device_id: String,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: u64,
    /// Message payload (type depends on `kind`).
    pub payload: AppSdkPayload,
}

/// Message type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppSdkKind {
    /// SDK → Runtime: SDK connected and ready.
    Hello,
    /// Runtime → SDK: Acknowledge hello, start session.
    SessionStarted,
    /// SDK → Runtime: App lifecycle event (start/stop/foreground/background).
    Lifecycle,
    /// SDK → Runtime: Component tree snapshot.
    ComponentTreeSnapshot,
    /// SDK → Runtime: Business event.
    BusinessEvent,
    /// SDK → Runtime: Console log entry.
    ConsoleLog,
    /// SDK → Runtime: Network request record.
    NetworkRecord,
    /// Runtime → SDK: Request a component tree snapshot.
    RequestComponentTree,
    /// Runtime → SDK: Request to enable/disable debug features.
    SetDebugMode,
    /// SDK → Runtime: SDK disconnecting.
    Goodbye,
    /// Runtime → SDK: Ping for liveness.
    Ping,
    /// SDK → Runtime: Pong response.
    Pong,
}

/// Message payload variants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppSdkPayload {
    /// Hello payload: framework info and SDK version.
    Hello(HelloPayload),
    /// Session started: assigned session token.
    SessionStarted { session_token: String },
    /// App lifecycle event.
    Lifecycle(LifecyclePayload),
    /// Component tree snapshot.
    ComponentTreeSnapshot(ComponentTree),
    /// Business event.
    BusinessEvent(BusinessEvent),
    /// Console log entry.
    ConsoleLog(ConsoleLogEntry),
    /// Network request record.
    NetworkRecord(NetworkRecordPayload),
    /// Request component tree: empty (just trigger).
    RequestComponentTree,
    /// Set debug mode.
    SetDebugMode { enabled: bool },
    /// Goodbye: reason for disconnect.
    Goodbye { reason: String },
    /// Ping: sequence number.
    Ping { seq: u64 },
    /// Pong: echo sequence number.
    Pong { seq: u64 },
}

/// Hello payload sent when SDK connects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloPayload {
    /// Framework kind.
    pub framework: FrameworkKind,
    /// SDK version.
    pub sdk_version: String,
    /// App package/bundle identifier.
    pub app_id: String,
    /// App version.
    pub app_version: String,
    /// Whether debug profile is active.
    pub debug_enabled: bool,
    /// Declared capabilities.
    pub capabilities: SdkCapabilities,
}

/// App lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppLifecycleState {
    /// App started (process created).
    Started,
    /// App entered foreground.
    Foreground,
    /// App entered background.
    Background,
    /// App stopped (process destroyed).
    Stopped,
}

/// Lifecycle event payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecyclePayload {
    pub state: AppLifecycleState,
}

/// Console log entry from the App SDK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleLogEntry {
    /// Log level.
    pub level: ConsoleLogLevel,
    /// Log message.
    pub message: String,
    /// Optional tag/category.
    pub tag: Option<String>,
    /// Source location if available.
    pub source: Option<String>,
    /// Timestamp in milliseconds since epoch.
    pub timestamp_ms: u64,
}

/// Console log levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLogLevel {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
}

/// Network record from the App SDK.
///
/// Carries the same fields as the device-level `NetworkRecord` but sourced
/// from in-app interceptors (OkHttp, URLSession, uni.request, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkRecordPayload {
    pub request_id: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub duration_ms: u64,
    pub request_headers_redacted: Vec<(String, String)>,
    pub response_headers_redacted: Vec<(String, String)>,
    pub request_body_redacted: Option<String>,
    pub response_body_redacted: Option<String>,
    pub error: Option<String>,
}

/// SDK capabilities declared in the Hello message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SdkCapabilities {
    /// Can provide component tree snapshots.
    pub component_tree: bool,
    /// Can capture business events.
    pub business_events: bool,
    /// Can capture console logs.
    pub console_logs: bool,
    /// Can capture network records.
    pub network_records: bool,
}

impl AppSdkEnvelope {
    /// Create a Hello envelope.
    pub fn hello(session_id: &str, device_id: &str, payload: HelloPayload) -> Self {
        Self {
            version: APP_SDK_PROTOCOL_VERSION,
            kind: AppSdkKind::Hello,
            session_id: session_id.into(),
            device_id: device_id.into(),
            timestamp_ms: 0,
            payload: AppSdkPayload::Hello(payload),
        }
    }

    /// Create a Goodbye envelope.
    pub fn goodbye(session_id: &str, device_id: &str, reason: &str) -> Self {
        Self {
            version: APP_SDK_PROTOCOL_VERSION,
            kind: AppSdkKind::Goodbye,
            session_id: session_id.into(),
            device_id: device_id.into(),
            timestamp_ms: 0,
            payload: AppSdkPayload::Goodbye {
                reason: reason.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_carries_protocol_version() {
        let env = AppSdkEnvelope::goodbye("s", "d", "test");
        assert_eq!(env.version, APP_SDK_PROTOCOL_VERSION);
    }

    #[test]
    fn envelope_kind_tag_is_snake_case() {
        let json = serde_json::to_string(&AppSdkKind::ComponentTreeSnapshot).unwrap();
        assert_eq!(json, "\"component_tree_snapshot\"");
    }

    #[test]
    fn hello_payload_serde() {
        let payload = HelloPayload {
            framework: FrameworkKind::UniApp,
            sdk_version: "1.0.0".into(),
            app_id: "com.example.uni".into(),
            app_version: "2.0.0".into(),
            debug_enabled: true,
            capabilities: SdkCapabilities {
                component_tree: true,
                business_events: true,
                console_logs: true,
                network_records: true,
            },
        };
        let json = serde_json::to_string(&payload).unwrap();
        let back: HelloPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload, back);
    }

    #[test]
    fn hello_envelope_convenience() {
        let env = AppSdkEnvelope::hello(
            "sess-1",
            "dev-1",
            HelloPayload {
                framework: FrameworkKind::ReactNative,
                sdk_version: "1.0.0".into(),
                app_id: "com.example.rn".into(),
                app_version: "1.0.0".into(),
                debug_enabled: true,
                capabilities: SdkCapabilities {
                    component_tree: true,
                    business_events: false,
                    console_logs: true,
                    network_records: false,
                },
            },
        );
        assert_eq!(env.kind, AppSdkKind::Hello);
        assert_eq!(env.version, APP_SDK_PROTOCOL_VERSION);
    }

    #[test]
    fn goodbye_envelope_convenience() {
        let env = AppSdkEnvelope::goodbye("sess-1", "dev-1", "user closed app");
        assert_eq!(env.kind, AppSdkKind::Goodbye);
        if let AppSdkPayload::Goodbye { reason } = env.payload {
            assert_eq!(reason, "user closed app");
        } else {
            panic!("expected Goodbye payload");
        }
    }

    #[test]
    fn lifecycle_states_serialize() {
        let states = [
            AppLifecycleState::Started,
            AppLifecycleState::Foreground,
            AppLifecycleState::Background,
            AppLifecycleState::Stopped,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let back: AppLifecycleState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn console_log_entry_serde() {
        let entry = ConsoleLogEntry {
            level: ConsoleLogLevel::Warn,
            message: "deprecated API called".into(),
            tag: Some("Vue".into()),
            source: Some("component.vue:42".into()),
            timestamp_ms: 7000,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("deprecated API called"));
        let back: ConsoleLogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn network_record_payload_serde() {
        let record = NetworkRecordPayload {
            request_id: "req-1".into(),
            method: "GET".into(),
            url: "https://api.example.com/data".into(),
            status: Some(200),
            duration_ms: 150,
            request_headers_redacted: vec![("Authorization".into(), "[REDACTED]".into())],
            response_headers_redacted: vec![],
            request_body_redacted: None,
            response_body_redacted: Some(r#"{"data":"ok"}"#.into()),
            error: None,
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: NetworkRecordPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn sdk_capabilities_serde() {
        let caps = SdkCapabilities {
            component_tree: true,
            business_events: false,
            console_logs: true,
            network_records: true,
        };
        let json = serde_json::to_string(&caps).unwrap();
        let back: SdkCapabilities = serde_json::from_str(&json).unwrap();
        assert_eq!(caps, back);
    }

    #[test]
    fn all_sdk_kinds_serialize() {
        let kinds = [
            AppSdkKind::Hello,
            AppSdkKind::SessionStarted,
            AppSdkKind::Lifecycle,
            AppSdkKind::ComponentTreeSnapshot,
            AppSdkKind::BusinessEvent,
            AppSdkKind::ConsoleLog,
            AppSdkKind::NetworkRecord,
            AppSdkKind::RequestComponentTree,
            AppSdkKind::SetDebugMode,
            AppSdkKind::Goodbye,
            AppSdkKind::Ping,
            AppSdkKind::Pong,
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let back: AppSdkKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    #[test]
    fn ping_pong_round_trip() {
        let ping = AppSdkEnvelope {
            version: APP_SDK_PROTOCOL_VERSION,
            kind: AppSdkKind::Ping,
            session_id: "sess-1".into(),
            device_id: "dev-1".into(),
            timestamp_ms: 8000,
            payload: AppSdkPayload::Ping { seq: 42 },
        };
        let json = serde_json::to_string(&ping).unwrap();
        let back: AppSdkEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(ping, back);
    }
}
