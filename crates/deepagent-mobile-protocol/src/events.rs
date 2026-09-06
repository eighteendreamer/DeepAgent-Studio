use deepagent_mobile_core::ArtifactRef;
use serde::{Deserialize, Serialize};

/// Events emitted by the mobile subsystem.
///
/// These are projected into `RuntimeEvent::Mobile { .. }` by the runtime layer.
/// Events are serializable, replayable, and support cursor-based resumption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobileEvent {
    DeviceDiscovered {
        device_id: String,
    },
    DeviceConnected {
        device_id: String,
    },
    DeviceDisconnected {
        device_id: String,
        reason: String,
    },
    DeviceStateChanged {
        device_id: String,
        from: deepagent_mobile_core::DeviceState,
        to: deepagent_mobile_core::DeviceState,
    },
    AppInstalled {
        device_id: String,
        package: String,
    },
    AppStarted {
        device_id: String,
        package: String,
    },
    AppStopped {
        device_id: String,
        package: String,
    },
    UiSnapshotCaptured {
        device_id: String,
        snapshot_id: String,
        node_count: u32,
    },
    InputPerformed {
        device_id: String,
        operation_id: String,
        success: bool,
    },
    ScreenshotCaptured {
        device_id: String,
        artifact: ArtifactRef,
    },
    LogReceived {
        device_id: String,
        line_count: u32,
    },
    CrashDetected {
        device_id: String,
        package: Option<String>,
        summary: String,
    },
    BackendDiagnostic {
        platform: deepagent_mobile_core::MobilePlatform,
        message: String,
    },
    BackendError {
        message: String,
    },
    EmulatorStarted {
        avd_name: String,
        serial: String,
    },
    EmulatorStopped {
        serial: String,
        reason: String,
    },
    NetworkRecordCaptured {
        device_id: String,
        record_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_mobile_core::DeviceState;

    #[test]
    fn event_serde_round_trip() {
        let event = MobileEvent::DeviceStateChanged {
            device_id: "dev-1".into(),
            from: DeviceState::Connecting,
            to: DeviceState::Ready,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: MobileEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }

    #[test]
    fn event_tag_is_snake_case() {
        let event = MobileEvent::DeviceDiscovered {
            device_id: "dev-1".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"device_discovered\""));
    }

    #[test]
    fn screenshot_event_carries_artifact() {
        let event = MobileEvent::ScreenshotCaptured {
            device_id: "dev-1".into(),
            artifact: ArtifactRef {
                artifact_id: "art-1".into(),
                mime: "image/png".into(),
                size_bytes: 1024,
                sha256: Some("abc123".into()),
                storage_path: "/tmp/art-1.png".into(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("image/png"));
        let back: MobileEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(event, back);
    }
}
