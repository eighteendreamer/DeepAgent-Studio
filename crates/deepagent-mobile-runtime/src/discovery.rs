//! Periodic device discovery loop.
//!
//! Polls a `MobileBackend` for visible devices at a configurable interval,
//! diffs the results against the `DeviceRegistry`, and lets the registry emit
//! the appropriate `DeviceDiscovered` / `DeviceStateChanged` /
//! `DeviceDisconnected` events.
//!
//! The loop is cancellable via a `CancellationToken` and tolerates transient
//! backend errors (logging them) without terminating.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use deepagent_mobile_core::MobileDevice;
use deepagent_mobile_protocol::MobileEvent;
use tokio::sync::mpsc;

use crate::backend::MobileBackend;
use crate::device_registry::DeviceRegistry;
use crate::operation::OperationContext;

/// Default poll interval for device discovery (3 seconds).
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Configuration for the discovery loop.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// How often to poll the backend for devices.
    pub poll_interval: Duration,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// Run a discovery loop that periodically calls `backend.list_devices()` and
/// reconciles the result with the `DeviceRegistry`.
///
/// The loop runs until `cancel` is triggered. Transient errors from
/// `list_devices` are logged and do not terminate the loop.
///
/// Returns the event receiver so callers can observe discovery events. The
/// receiver is the same one the registry writes to; callers who only need
/// discovery events should filter on `DeviceDiscovered`, `DeviceStateChanged`,
/// and `DeviceDisconnected`.
pub async fn run_discovery_loop(
    backend: Arc<dyn MobileBackend>,
    registry: Arc<DeviceRegistry>,
    config: DiscoveryConfig,
    cancel: CancellationToken,
    event_tx: mpsc::UnboundedSender<MobileEvent>,
) {
    let mut tick = tokio::time::interval(config.poll_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("discovery loop cancelled");
                break;
            }
            _ = tick.tick() => {}
        }

        let ctx = OperationContext::new("discovery".into(), String::new(), config.poll_interval);

        match backend.list_devices(&ctx).await {
            Ok(devices) => {
                reconcile(&registry, &devices).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "discovery scan failed");
                let _ = event_tx.send(MobileEvent::BackendError {
                    message: format!("discovery scan failed: {e}"),
                });
            }
        }
    }
}

/// Diff the scanned devices against the registry: upsert each scanned device,
/// then remove any registry device that was not in the scan.
async fn reconcile(registry: &DeviceRegistry, scanned: &[MobileDevice]) {
    let scanned_ids: HashSet<&str> = scanned.iter().map(|d| d.id.as_str()).collect();

    for device in scanned {
        registry.upsert(device.clone()).await;
    }

    let known = registry.list().await;
    for device in known {
        if !scanned_ids.contains(device.id.as_str()) {
            registry
                .remove(&device.id, "no longer visible in backend scan")
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MobileBackend;
    use deepagent_mobile_core::*;
    use deepagent_mobile_protocol::*;
    use std::sync::Mutex;

    struct FakeBackend {
        scans: Mutex<Vec<Vec<MobileDevice>>>,
    }

    impl FakeBackend {
        fn new(scans: Vec<Vec<MobileDevice>>) -> Self {
            Self {
                scans: Mutex::new(scans),
            }
        }
    }

    fn device(id: &str, state: DeviceState) -> MobileDevice {
        MobileDevice {
            id: id.into(),
            name: "Fake".into(),
            platform: MobilePlatform::Android,
            kind: DeviceKind::Physical,
            connection: DeviceConnection::Usb,
            state,
            os_version: None,
            capabilities: DeviceCapabilities {
                screenshot: true,
                ui_tree: true,
                input: true,
                logs: true,
                install: true,
                network_inspection: false,
            },
        }
    }

    #[async_trait::async_trait]
    impl MobileBackend for FakeBackend {
        async fn probe(&self) -> MobileResult<BackendStatus> {
            Ok(BackendStatus {
                platform: MobilePlatform::Android,
                available: true,
                toolchain_version: None,
                tool_paths: vec![],
                diagnostics: vec![],
            })
        }

        async fn list_devices(&self, _ctx: &OperationContext) -> MobileResult<Vec<MobileDevice>> {
            let mut scans = self.scans.lock().unwrap();
            if scans.is_empty() {
                Ok(vec![])
            } else {
                Ok(scans.remove(0))
            }
        }

        async fn device_info(
            &self,
            _device_id: &str,
            _ctx: &OperationContext,
        ) -> MobileResult<MobileDevice> {
            unimplemented!()
        }

        async fn screenshot(
            &self,
            _device_id: &str,
            _ctx: &OperationContext,
        ) -> MobileResult<ArtifactRef> {
            unimplemented!()
        }

        async fn ui_snapshot(
            &self,
            _device_id: &str,
            _ctx: &OperationContext,
        ) -> MobileResult<UiSnapshot> {
            unimplemented!()
        }

        async fn install(
            &self,
            _request: &InstallRequest,
            _ctx: &OperationContext,
        ) -> MobileResult<()> {
            unimplemented!()
        }

        async fn launch(
            &self,
            _request: &LaunchRequest,
            _ctx: &OperationContext,
        ) -> MobileResult<()> {
            unimplemented!()
        }

        async fn terminate(
            &self,
            _target: &AppTarget,
            _ctx: &OperationContext,
        ) -> MobileResult<()> {
            unimplemented!()
        }

        async fn input(
            &self,
            _request: &InputRequest,
            _ctx: &OperationContext,
        ) -> MobileResult<InputResult> {
            unimplemented!()
        }

        async fn read_logs(
            &self,
            _request: &LogRequest,
            _ctx: &OperationContext,
        ) -> MobileResult<LogPage> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn discovery_emits_discovered_on_first_scan() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registry = Arc::new(DeviceRegistry::new(tx.clone()));
        let backend = Arc::new(FakeBackend::new(vec![vec![device(
            "dev-1",
            DeviceState::Ready,
        )]]));
        let cancel = CancellationToken::new();

        let config = DiscoveryConfig {
            poll_interval: Duration::from_millis(50),
        };

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            run_discovery_loop(backend, registry, config, cancel_clone, tx).await;
        });

        let event = rx.recv().await.unwrap();
        assert!(matches!(
            event,
            MobileEvent::DeviceDiscovered { device_id } if device_id == "dev-1"
        ));

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn discovery_removes_gone_devices() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registry = Arc::new(DeviceRegistry::new(tx.clone()));
        let backend = Arc::new(FakeBackend::new(vec![
            vec![
                device("dev-1", DeviceState::Ready),
                device("dev-2", DeviceState::Ready),
            ],
            vec![device("dev-1", DeviceState::Ready)],
        ]));
        let cancel = CancellationToken::new();

        let config = DiscoveryConfig {
            poll_interval: Duration::from_millis(50),
        };

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            run_discovery_loop(backend, registry, config, cancel_clone, tx).await;
        });

        // First scan: two devices discovered
        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();
        assert!(matches!(e1, MobileEvent::DeviceDiscovered { .. }));
        assert!(matches!(e2, MobileEvent::DeviceDiscovered { .. }));

        // Second scan: dev-2 removed
        let e3 = rx.recv().await.unwrap();
        assert!(matches!(
            e3,
            MobileEvent::DeviceDisconnected { device_id, .. } if device_id == "dev-2"
        ));

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn discovery_detects_state_change() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registry = Arc::new(DeviceRegistry::new(tx.clone()));
        let backend = Arc::new(FakeBackend::new(vec![
            vec![device("dev-1", DeviceState::Unauthorized)],
            vec![device("dev-1", DeviceState::Ready)],
        ]));
        let cancel = CancellationToken::new();

        let config = DiscoveryConfig {
            poll_interval: Duration::from_millis(50),
        };

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            run_discovery_loop(backend, registry, config, cancel_clone, tx).await;
        });

        // First scan: discovered as Unauthorized
        let e1 = rx.recv().await.unwrap();
        assert!(matches!(e1, MobileEvent::DeviceDiscovered { .. }));

        // Second scan: state changed to Ready
        let e2 = rx.recv().await.unwrap();
        assert!(matches!(
            e2,
            MobileEvent::DeviceStateChanged {
                from: DeviceState::Unauthorized,
                to: DeviceState::Ready,
                ..
            }
        ));

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn discovery_tolerates_backend_errors() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let registry = Arc::new(DeviceRegistry::new(tx.clone()));

        struct FailBackend;

        #[async_trait::async_trait]
        impl MobileBackend for FailBackend {
            async fn probe(&self) -> MobileResult<BackendStatus> {
                unimplemented!()
            }
            async fn list_devices(
                &self,
                _ctx: &OperationContext,
            ) -> MobileResult<Vec<MobileDevice>> {
                Err(MobileError::ToolNotFound {
                    tool_name: "adb".into(),
                })
            }
            async fn device_info(
                &self,
                _device_id: &str,
                _ctx: &OperationContext,
            ) -> MobileResult<MobileDevice> {
                unimplemented!()
            }
            async fn screenshot(
                &self,
                _device_id: &str,
                _ctx: &OperationContext,
            ) -> MobileResult<ArtifactRef> {
                unimplemented!()
            }
            async fn ui_snapshot(
                &self,
                _device_id: &str,
                _ctx: &OperationContext,
            ) -> MobileResult<UiSnapshot> {
                unimplemented!()
            }
            async fn install(
                &self,
                _request: &InstallRequest,
                _ctx: &OperationContext,
            ) -> MobileResult<()> {
                unimplemented!()
            }
            async fn launch(
                &self,
                _request: &LaunchRequest,
                _ctx: &OperationContext,
            ) -> MobileResult<()> {
                unimplemented!()
            }
            async fn terminate(
                &self,
                _target: &AppTarget,
                _ctx: &OperationContext,
            ) -> MobileResult<()> {
                unimplemented!()
            }
            async fn input(
                &self,
                _request: &InputRequest,
                _ctx: &OperationContext,
            ) -> MobileResult<InputResult> {
                unimplemented!()
            }
            async fn read_logs(
                &self,
                _request: &LogRequest,
                _ctx: &OperationContext,
            ) -> MobileResult<LogPage> {
                unimplemented!()
            }
        }

        let backend = Arc::new(FailBackend);
        let cancel = CancellationToken::new();

        let config = DiscoveryConfig {
            poll_interval: Duration::from_millis(50),
        };

        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            run_discovery_loop(backend, registry, config, cancel_clone, tx).await;
        });

        // Should get a BackendError event, not a crash
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, MobileEvent::BackendError { .. }));

        cancel.cancel();
        let _ = handle.await;
    }
}
