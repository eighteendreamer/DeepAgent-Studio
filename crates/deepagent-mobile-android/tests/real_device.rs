//! Real-device integration tests for AdbBackend.
//!
//! These tests require a real Android device connected via USB (or a running
//! emulator) and adb accessible via PATH, ANDROID_HOME, or well-known SDK
//! directories. They are ignored by default; run with `-- --ignored` when a
//! device is available.

use deepagent_mobile_android::{AdbBackend, SystemAdbRunner, ToolResolver};
use deepagent_mobile_core::DeviceState;
use deepagent_mobile_runtime::{MobileBackend, OperationContext};
use std::sync::Arc;
use std::time::Duration;

fn ctx() -> OperationContext {
    OperationContext::new(
        "op-integration".into(),
        "test-device".into(),
        Duration::from_secs(30),
    )
}

fn real_backend() -> AdbBackend {
    let resolver = ToolResolver::new();
    let runner = Arc::new(SystemAdbRunner::new());
    AdbBackend::new(resolver, runner)
}

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_probe_finds_adb() {
    let backend = real_backend();
    let status = backend.probe().await.expect("probe should succeed");
    assert!(
        status.available,
        "adb should be available; diagnostics: {:?}",
        status.diagnostics
    );
    assert!(
        !status.tool_paths.is_empty(),
        "should have at least adb path"
    );
    let adb_tool = status.tool_paths.iter().find(|t| t.name == "adb");
    assert!(adb_tool.is_some(), "adb should be in tool_paths");
    let adb_path = &adb_tool.unwrap().path;
    assert!(
        adb_path.contains("adb"),
        "adb path should contain 'adb': {adb_path}"
    );
    eprintln!("Real adb found at: {adb_path}");
}

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_list_devices_finds_usb_device() {
    let backend = real_backend();
    let devices = backend
        .list_devices(&ctx())
        .await
        .expect("list_devices should succeed");
    assert!(
        !devices.is_empty(),
        "at least one real device should be connected"
    );
    for device in &devices {
        eprintln!(
            "Real device: id={} name={} state={:?} platform={:?} kind={:?} connection={:?}",
            device.id, device.name, device.state, device.platform, device.kind, device.connection
        );
        assert!(
            matches!(
                device.state,
                DeviceState::Ready | DeviceState::Offline | DeviceState::Unauthorized
            ),
            "device state should be a known state: {:?}",
            device.state
        );
    }
    let ready_devices: Vec<_> = devices
        .iter()
        .filter(|d| d.state == DeviceState::Ready)
        .collect();
    assert!(
        !ready_devices.is_empty(),
        "at least one device should be in Ready state"
    );
}

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_device_info_returns_full_properties() {
    let backend = real_backend();
    let devices = backend
        .list_devices(&ctx())
        .await
        .expect("list_devices should succeed");
    let ready = devices
        .iter()
        .find(|d| d.state == DeviceState::Ready)
        .expect("at least one Ready device required");

    let info = backend
        .device_info(&ready.id, &ctx())
        .await
        .expect("device_info should succeed");
    eprintln!(
        "Device info: id={} name={} os_version={:?} capabilities={:?}",
        info.id, info.name, info.os_version, info.capabilities
    );
    assert_eq!(info.id, ready.id);
    assert!(!info.name.is_empty(), "device name should not be empty");
}
