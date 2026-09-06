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

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_screenshot_produces_valid_png() {
    let backend = real_backend();
    let devices = backend
        .list_devices(&ctx())
        .await
        .expect("list_devices should succeed");
    let ready = devices
        .iter()
        .find(|d| d.state == DeviceState::Ready)
        .expect("at least one Ready device required");

    let artifact = backend
        .screenshot(&ready.id, &ctx())
        .await
        .expect("screenshot should succeed");

    eprintln!(
        "Screenshot: id={} mime={} size={} path={}",
        artifact.artifact_id, artifact.mime, artifact.size_bytes, artifact.storage_path
    );

    assert_eq!(artifact.mime, "image/png");
    assert!(
        artifact.size_bytes > 0,
        "screenshot should have non-zero size"
    );

    let storage_path = std::path::Path::new(&artifact.storage_path);
    assert!(
        storage_path.exists(),
        "artifact file should exist at {}",
        artifact.storage_path
    );

    let bytes = std::fs::read(storage_path).expect("should be able to read artifact file");
    assert_eq!(
        bytes.len() as u64,
        artifact.size_bytes,
        "file size should match reported size"
    );

    // PNG magic bytes: 89 50 4E 47 0D 0A 1A 0A
    assert!(
        bytes.len() >= 8,
        "file should have at least 8 bytes for PNG header"
    );
    assert_eq!(
        &bytes[..8],
        &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        "file should start with valid PNG magic bytes"
    );

    // Clean up
    let _ = std::fs::remove_file(storage_path);
}
