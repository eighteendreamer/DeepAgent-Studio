//! H-50: real MobileService/AdbBackend dispatch, only the ADB process is offline.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::tool_resolver::ResolvedTool;
use crate::{AdbBackend, AdbCommandOutput, AdbCommandRunner, ToolResolver};
use async_trait::async_trait;
use deepagent_mobile_core::{DeviceState, MobileError, MobilePlatform, MobileResult};
use deepagent_mobile_protocol::{InputAction, InputRequest};
use deepagent_mobile_runtime::{
    ArtifactStore, DeviceRegistry, MobileBackend, MobileService, OperationContext, SnapshotStore,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

struct Gate {
    entered: Semaphore,
    release: Semaphore,
}
impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        })
    }
    async fn entered(&self) {
        tokio::time::timeout(Duration::from_secs(2), self.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
    fn release(&self) {
        self.release.add_permits(1);
    }
}

#[derive(Default)]
struct RecordingAdb {
    calls: Mutex<Vec<Vec<String>>>,
    gate: Mutex<Option<(&'static str, Arc<Gate>)>>,
    fail_input: std::sync::atomic::AtomicBool,
    fail_capture: std::sync::atomic::AtomicBool,
}
impl RecordingAdb {
    fn gate(&self, operation: &'static str) -> Arc<Gate> {
        let gate = Gate::new();
        *self.gate.lock().unwrap() = Some((operation, gate.clone()));
        gate
    }
    fn count(&self, command: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|args| args.get(3).is_some_and(|arg| arg == command))
            .count()
    }
}
#[async_trait]
impl AdbCommandRunner for RecordingAdb {
    async fn run(
        &self,
        _: &str,
        args: &[&str],
        _: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput> {
        self.calls
            .lock()
            .unwrap()
            .push(args.iter().map(|arg| arg.to_string()).collect());
        let gate = {
            let mut slot = self.gate.lock().unwrap();
            if slot
                .as_ref()
                .is_some_and(|(command, _)| args.get(3) == Some(command))
            {
                slot.take().map(|(_, gate)| gate)
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.entered.add_permits(1);
            tokio::select! {
                _ = cancel.cancelled() => return Err(MobileError::Cancelled { operation_id: "fixture".into() }),
                permit = gate.release.acquire() => permit.unwrap().forget(),
            }
        }
        if args.get(3) == Some(&"input")
            && self.fail_input.load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(MobileError::Internal(
                "offline input failure after dispatch".into(),
            ));
        }
        if args.get(3) == Some(&"uiautomator")
            && self.fail_capture.load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(MobileError::Internal("offline capture failure".into()));
        }
        let stdout = if args.first() == Some(&"devices") {
            b"List of devices attached\nABC123\tdevice\tmodel:Audit\nXYZ987\tdevice\tmodel:Audit\n"
                .to_vec()
        } else if args.get(2) == Some(&"exec-out") {
            br#"<?xml version="1.0" encoding="UTF-8"?><hierarchy><node text="Audit" class="android.widget.Button" bounds="[0,0][100,100]" /></hierarchy>"#.to_vec()
        } else {
            Vec::new()
        };
        Ok(AdbCommandOutput {
            exit_code: Some(0),
            stdout,
            stderr: String::new(),
        })
    }
}
fn ctx() -> OperationContext {
    OperationContext::new(
        "h50".into(),
        "android-ABC123".into(),
        Duration::from_secs(2),
    )
}
fn input(device: &str, snapshot: Option<&str>) -> InputRequest {
    InputRequest {
        device_id: device.into(),
        snapshot_id: snapshot.map(str::to_string),
        action: InputAction::Tap { x: 50, y: 50 },
    }
}
async fn fixture() -> (Arc<MobileService>, Arc<RecordingAdb>) {
    let runner = Arc::new(RecordingAdb::default());
    let resolver = ToolResolver::new();
    resolver.insert(ResolvedTool {
        name: "adb".into(),
        path: "/audit/non-executable-adb".into(),
        version: None,
    });
    let backend = Arc::new(AdbBackend::new(resolver, runner.clone()));
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let registry = DeviceRegistry::new(tx);
    for device in backend.list_devices(&ctx()).await.unwrap() {
        registry.upsert(device).await;
    }
    let service = Arc::new(MobileService::new(
        registry,
        ArtifactStore::new(),
        SnapshotStore::new(),
    ));
    service
        .register_backend(MobilePlatform::Android, backend)
        .await;
    (service, runner)
}
const DEVICE: &str = "android-ABC123";

#[tokio::test]
async fn references_are_device_connection_scoped_and_consumed_before_dispatch() {
    let (service, runner) = fixture().await;
    let first = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let other = service.ui_snapshot("android-XYZ987", &ctx()).await.unwrap();
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&other.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert_eq!(runner.count("input"), 0);
    assert!(
        service
            .input(&input(DEVICE, Some(&first.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&first.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert_eq!(runner.count("input"), 1);
    let snap = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    assert!(
        service
            .input(&input(DEVICE, None), &ctx())
            .await
            .unwrap()
            .accepted
    );
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&snap.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    let snap = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    runner
        .fail_input
        .store(true, std::sync::atomic::Ordering::Release);
    assert!(service
        .input(&input(DEVICE, Some(&snap.snapshot_id)), &ctx())
        .await
        .is_err());
    runner
        .fail_input
        .store(false, std::sync::atomic::Ordering::Release);
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&snap.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert_eq!(runner.count("input"), 3);
}

#[tokio::test]
async fn reconnect_and_offline_transition_revoke_same_device_references() {
    let (service, runner) = fixture().await;
    for remove in [true, false] {
        let snap = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
        let device = service.registry().get(DEVICE).await.unwrap();
        if remove {
            service
                .registry()
                .remove(DEVICE, "offline fixture disconnect")
                .await;
        } else {
            let mut offline = device.clone();
            offline.state = DeviceState::Offline;
            service.registry().upsert(offline).await;
        }
        service.registry().upsert(device).await;
        assert!(matches!(
            service
                .input(&input(DEVICE, Some(&snap.snapshot_id)), &ctx())
                .await,
            Err(MobileError::StaleUiNode { .. })
        ));
        assert_eq!(runner.count("input"), 0);
    }
    let snap = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    assert!(
        service
            .input(&input(DEVICE, Some(&snap.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
}

#[tokio::test]
async fn capture_blocks_stale_input_but_other_devices_progress() {
    let (service, runner) = fixture().await;
    let old = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let gate = runner.gate("uiautomator");
    let capture = {
        let service = service.clone();
        tokio::spawn(async move { service.ui_snapshot(DEVICE, &ctx()).await })
    };
    gate.entered().await;
    let request = input(DEVICE, Some(&old.snapshot_id));
    let context = ctx();
    let mut pending = Box::pin(service.input(&request, &context));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut pending)
            .await
            .is_err()
    );
    assert_eq!(runner.count("input"), 0);
    service.ui_snapshot("android-XYZ987", &ctx()).await.unwrap();
    gate.release();
    let latest = capture.await.unwrap().unwrap();
    assert!(matches!(
        pending.await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert!(
        service
            .input(&input(DEVICE, Some(&latest.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
}

#[tokio::test]
async fn input_owns_device_until_reply_and_waiting_operations_are_cancellable() {
    let (service, runner) = fixture().await;
    let snap = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let gate = runner.gate("input");
    let sending = {
        let service = service.clone();
        tokio::spawn(async move {
            service
                .input(&input(DEVICE, Some(&snap.snapshot_id)), &ctx())
                .await
        })
    };
    gate.entered().await;
    let cancelled = ctx();
    cancelled.cancel();
    assert!(matches!(
        service.ui_snapshot(DEVICE, &cancelled).await,
        Err(MobileError::Cancelled { .. })
    ));
    let short = OperationContext::new("short".into(), DEVICE.into(), Duration::from_millis(20));
    assert!(matches!(
        service.ui_snapshot(DEVICE, &short).await,
        Err(MobileError::Timeout { .. })
    ));
    let captures = runner.count("uiautomator");
    let context = ctx();
    let mut capture = Box::pin(service.ui_snapshot(DEVICE, &context));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut capture)
            .await
            .is_err()
    );
    assert_eq!(runner.count("uiautomator"), captures);
    gate.release();
    sending.await.unwrap().unwrap();
    capture.await.unwrap();
}

#[tokio::test]
async fn disconnect_during_capture_cannot_publish_into_new_connection() {
    let (service, runner) = fixture().await;
    let gate = runner.gate("uiautomator");
    let capture = {
        let service = service.clone();
        tokio::spawn(async move { service.ui_snapshot(DEVICE, &ctx()).await })
    };
    gate.entered().await;
    let device = service.registry().get(DEVICE).await.unwrap();
    service.registry().remove(DEVICE, "fixture").await;
    service.registry().upsert(device).await;
    gate.release();
    assert!(matches!(
        capture.await.unwrap(),
        Err(MobileError::StaleUiNode { .. })
    ));
    assert!(service.snapshot_store().current(DEVICE).await.is_none());
    assert_eq!(runner.count("input"), 0);
}

#[tokio::test]
async fn failed_and_cancelled_capture_revoke_previous_reference() {
    let (service, runner) = fixture().await;
    for cancel in [false, true] {
        let old = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
        let context = ctx();
        if cancel {
            let gate = runner.gate("uiautomator");
            let capture = {
                let service = service.clone();
                let context = context.clone();
                tokio::spawn(async move { service.ui_snapshot(DEVICE, &context).await })
            };
            gate.entered().await;
            context.cancel();
            assert!(matches!(
                capture.await.unwrap(),
                Err(MobileError::Cancelled { .. })
            ));
        } else {
            runner
                .fail_capture
                .store(true, std::sync::atomic::Ordering::Release);
            assert!(service.ui_snapshot(DEVICE, &context).await.is_err());
            runner
                .fail_capture
                .store(false, std::sync::atomic::Ordering::Release);
        }
        assert!(service.snapshot_store().current(DEVICE).await.is_none());
        assert!(matches!(
            service
                .input(&input(DEVICE, Some(&old.snapshot_id)), &ctx())
                .await,
            Err(MobileError::StaleUiNode { .. })
        ));
    }
    assert_eq!(runner.count("input"), 0);
    let fresh = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    assert!(
        service
            .input(&input(DEVICE, Some(&fresh.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
}

#[tokio::test]
async fn explicit_clear_waits_for_capture_then_revokes_its_result() {
    let (service, runner) = fixture().await;
    let gate = runner.gate("uiautomator");
    let capture = {
        let service = service.clone();
        tokio::spawn(async move { service.ui_snapshot(DEVICE, &ctx()).await })
    };
    gate.entered().await;
    let mut clear = Box::pin(service.snapshot_store().clear(DEVICE));
    assert!(tokio::time::timeout(Duration::from_millis(30), &mut clear)
        .await
        .is_err());
    gate.release();
    let snapshot = capture.await.unwrap().unwrap();
    clear.await;
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&snapshot.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert_eq!(runner.count("input"), 0);
}

#[tokio::test]
async fn stable_discovery_and_busy_lease_preserve_snapshot_but_route_change_revokes_it() {
    let (service, runner) = fixture().await;
    let first = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let device = service.registry().get(DEVICE).await.unwrap();
    service.registry().upsert(device.clone()).await;
    service.registry().acquire(DEVICE).await.unwrap();
    service.registry().release(DEVICE).await;
    assert!(
        service
            .input(&input(DEVICE, Some(&first.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
    let old = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let mut changed = device;
    changed.connection = deepagent_mobile_core::DeviceConnection::Remote {
        host_id: "offline-other-host".into(),
    };
    service.registry().upsert(changed).await;
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&old.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert_eq!(runner.count("input"), 1);
}

#[tokio::test]
async fn ui_mutations_and_cancelled_input_consume_snapshot() {
    use deepagent_mobile_protocol::{
        AppTarget, InstallRequest, LaunchRequest, StopEmulatorRequest,
    };
    let (service, runner) = fixture().await;
    for operation in [
        "install",
        "launch",
        "terminate",
        "stop_emulator",
        "cancel_input",
    ] {
        let old = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
        match operation {
            "install" => service
                .install(
                    &InstallRequest {
                        device_id: DEVICE.into(),
                        artifact_path: "/offline/app.apk".into(),
                    },
                    &ctx(),
                )
                .await
                .unwrap(),
            "launch" => service
                .launch(
                    &LaunchRequest {
                        device_id: DEVICE.into(),
                        package: "offline.audit".into(),
                        activity: None,
                    },
                    &ctx(),
                )
                .await
                .unwrap(),
            "terminate" => service
                .terminate(
                    &AppTarget {
                        device_id: DEVICE.into(),
                        package: "offline.audit".into(),
                    },
                    &ctx(),
                )
                .await
                .unwrap(),
            "stop_emulator" => service
                .stop_emulator(
                    &StopEmulatorRequest {
                        serial: "ABC123".into(),
                    },
                    &ctx(),
                )
                .await
                .unwrap(),
            _ => {
                let context = ctx();
                let gate = runner.gate("input");
                let sending = {
                    let service = service.clone();
                    let context = context.clone();
                    let request = input(DEVICE, Some(&old.snapshot_id));
                    tokio::spawn(async move { service.input(&request, &context).await })
                };
                gate.entered().await;
                context.cancel();
                assert!(matches!(
                    sending.await.unwrap(),
                    Err(MobileError::Cancelled { .. })
                ));
            }
        }
        assert!(
            matches!(
                service
                    .input(&input(DEVICE, Some(&old.snapshot_id)), &ctx())
                    .await,
                Err(MobileError::StaleUiNode { .. })
            ),
            "{operation}"
        );
    }
    assert_eq!(runner.count("input"), 1);
}

#[tokio::test]
async fn service_identity_does_not_depend_on_backend_unique_ids() {
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let registry = DeviceRegistry::new(tx);
    registry
        .upsert(crate::FakeAndroidBackend::fake_device(
            DEVICE,
            DeviceState::Ready,
        ))
        .await;
    let service = MobileService::new(registry, ArtifactStore::new(), SnapshotStore::new());
    let backend = Arc::new(crate::FakeAndroidBackend::new());
    let raw = backend.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    assert_eq!(
        raw.snapshot_id,
        backend
            .ui_snapshot(DEVICE, &ctx())
            .await
            .unwrap()
            .snapshot_id
    );
    service
        .register_backend(MobilePlatform::Android, backend)
        .await;
    let old = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let fresh = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    assert_ne!(old.snapshot_id, fresh.snapshot_id);
    assert_eq!(
        service.snapshot_store().current(DEVICE).await.as_deref(),
        Some(fresh.snapshot_id.as_str())
    );
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&old.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert!(
        service
            .input(&input(DEVICE, Some(&fresh.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
}

#[tokio::test]
async fn backend_replacement_revokes_published_and_inflight_snapshots() {
    let (service, runner) = fixture().await;
    let old = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    let gate = runner.gate("uiautomator");
    let capture = {
        let service = service.clone();
        tokio::spawn(async move { service.ui_snapshot(DEVICE, &ctx()).await })
    };
    gate.entered().await;
    service
        .register_backend(
            MobilePlatform::Android,
            Arc::new(crate::FakeAndroidBackend::new()),
        )
        .await;
    gate.release();
    assert!(matches!(
        capture.await.unwrap(),
        Err(MobileError::StaleUiNode { .. })
    ));
    assert!(service.snapshot_store().current(DEVICE).await.is_none());
    let latest = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    assert!(
        service
            .input(&input(DEVICE, Some(&latest.snapshot_id)), &ctx())
            .await
            .unwrap()
            .accepted
    );
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&old.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    let latest = service.ui_snapshot(DEVICE, &ctx()).await.unwrap();
    service
        .register_backend(
            MobilePlatform::Android,
            Arc::new(crate::FakeAndroidBackend::new()),
        )
        .await;
    assert!(matches!(
        service
            .input(&input(DEVICE, Some(&latest.snapshot_id)), &ctx())
            .await,
        Err(MobileError::StaleUiNode { .. })
    ));
    assert_eq!(runner.count("input"), 0);
}
