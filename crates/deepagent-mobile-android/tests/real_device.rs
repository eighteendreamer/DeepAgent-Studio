//! Real-device integration tests for AdbBackend.
//!
//! These tests require a real Android device connected via USB (or a running
//! emulator) and adb accessible via PATH, ANDROID_HOME, or well-known SDK
//! directories. They are ignored by default; run with `-- --ignored` when a
//! device is available.

use deepagent_mobile_android::{AdbBackend, SystemAdbRunner, ToolResolver};
use deepagent_mobile_core::DeviceState;
use deepagent_mobile_protocol::{AppTarget, LaunchRequest};
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

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_launch_and_terminate_system_app() {
    let backend = real_backend();
    let devices = backend
        .list_devices(&ctx())
        .await
        .expect("list_devices should succeed");
    let ready = devices
        .iter()
        .find(|d| d.state == DeviceState::Ready)
        .expect("at least one Ready device required");

    let launch_req = LaunchRequest {
        device_id: ready.id.clone(),
        package: "com.android.settings".into(),
        activity: Some("com.android.settings.Settings".into()),
    };

    backend
        .launch(&launch_req, &ctx())
        .await
        .expect("launch com.android.settings should succeed");
    eprintln!("Launched com.android.settings on {}", ready.id);

    // Give the app a moment to start
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Take a screenshot to prove the app is visible
    let artifact = backend
        .screenshot(&ready.id, &ctx())
        .await
        .expect("screenshot after launch should succeed");
    assert!(
        artifact.size_bytes > 0,
        "post-launch screenshot should have content"
    );
    eprintln!(
        "Post-launch screenshot: {} bytes at {}",
        artifact.size_bytes, artifact.storage_path
    );

    let terminate_target = AppTarget {
        device_id: ready.id.clone(),
        package: "com.android.settings".into(),
    };

    backend
        .terminate(&terminate_target, &ctx())
        .await
        .expect("terminate com.android.settings should succeed");
    eprintln!("Terminated com.android.settings on {}", ready.id);

    // Clean up screenshot
    let _ = std::fs::remove_file(&artifact.storage_path);
}

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_ui_snapshot_returns_full_tree() {
    let backend = real_backend();
    let devices = backend
        .list_devices(&ctx())
        .await
        .expect("list_devices should succeed");
    let ready = devices
        .iter()
        .find(|d| d.state == DeviceState::Ready)
        .expect("at least one Ready device required");

    let snapshot = backend
        .ui_snapshot(&ready.id, &ctx())
        .await
        .expect("ui_snapshot should succeed");

    eprintln!(
        "UI snapshot: id={} nodes={} max_depth={} root={}",
        snapshot.snapshot_id,
        snapshot.nodes.len(),
        snapshot.max_depth(),
        snapshot.root_node_id
    );

    assert!(
        !snapshot.snapshot_id.is_empty(),
        "snapshot_id should not be empty"
    );
    assert!(!snapshot.nodes.is_empty(), "should have at least one node");

    let nodes_with_bounds = snapshot
        .nodes
        .iter()
        .filter(|n| n.bounds.width > 0 && n.bounds.height > 0)
        .count();
    assert!(
        nodes_with_bounds > 0,
        "at least some nodes should have non-zero bounds"
    );

    let has_role = snapshot
        .nodes
        .iter()
        .any(|n| n.role != deepagent_mobile_protocol::UiRole::Unknown);
    assert!(has_role, "at least some nodes should have a known role");

    assert!(snapshot.max_depth() > 0, "tree should have depth > 0");
}

#[tokio::test]
#[ignore = "requires real Android device or emulator"]
async fn real_network_capture_chain_works() {
    let backend = real_backend();
    let devices = backend
        .list_devices(&ctx())
        .await
        .expect("list_devices should succeed");
    let ready = devices
        .iter()
        .find(|d| d.state == DeviceState::Ready)
        .expect("at least one Ready device required");

    assert!(
        ready.capabilities.network_inspection,
        "device should report network_inspection capability"
    );

    backend
        .start_network_capture(&ready.id, &ctx())
        .await
        .expect("start_network_capture should succeed");
    eprintln!("Network capture started on {}", ready.id);

    let launch_req = LaunchRequest {
        device_id: ready.id.clone(),
        package: "com.android.settings".into(),
        activity: Some("com.android.settings.Settings".into()),
    };
    backend
        .launch(&launch_req, &ctx())
        .await
        .expect("launch should succeed");

    tokio::time::sleep(Duration::from_secs(3)).await;

    let records = backend
        .get_network_records(&ready.id, &ctx())
        .await
        .expect("get_network_records should succeed");
    eprintln!(
        "Captured {} network records (logcat may not have HTTP entries for all apps)",
        records.len()
    );

    for record in &records {
        assert!(
            !record.record_id.is_empty(),
            "record_id should not be empty"
        );
        assert_eq!(record.device_id, ready.id, "device_id should match");
        eprintln!(
            "  Record: {} {} -> {:?}",
            record.request.method,
            record.request.url,
            record.response.as_ref().map(|r| r.status_code),
        );
    }

    backend
        .stop_network_capture(&ready.id, &ctx())
        .await
        .expect("stop_network_capture should succeed");
    eprintln!("Network capture stopped on {}", ready.id);

    let terminate_target = AppTarget {
        device_id: ready.id.clone(),
        package: "com.android.settings".into(),
    };
    let _ = backend.terminate(&terminate_target, &ctx()).await;
}
