use deepagent_mobile_core::{DeviceState, MobileDevice, MobileError, MobileResult};
use deepagent_mobile_protocol::MobileEvent;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Single source of truth for device state.
///
/// All state transitions are centralized here. No other crate may infer device
/// state from raw tool output. Every transition emits a
/// `DeviceStateChanged` event through the event channel.
pub struct DeviceRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

struct RegistryInner {
    devices: HashMap<String, MobileDevice>,
    event_tx: tokio::sync::mpsc::UnboundedSender<MobileEvent>,
}

impl DeviceRegistry {
    pub fn new(event_tx: tokio::sync::mpsc::UnboundedSender<MobileEvent>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                devices: HashMap::new(),
                event_tx,
            })),
        }
    }

    /// Insert or update a device. Emits `DeviceDiscovered` for new devices and
    /// `DeviceStateChanged` when the state changes.
    pub async fn upsert(&self, device: MobileDevice) {
        let mut inner = self.inner.lock().await;
        let existing = inner.devices.get(&device.id).cloned();

        match existing {
            None => {
                tracing::info!(device_id = %device.id, state = ?device.state, "device discovered");
                let _ = inner.event_tx.send(MobileEvent::DeviceDiscovered {
                    device_id: device.id.clone(),
                });
                inner.devices.insert(device.id.clone(), device);
            }
            Some(old) if old.state != device.state => {
                tracing::info!(
                    device_id = %device.id,
                    from = ?old.state,
                    to = ?device.state,
                    "device state changed"
                );
                let _ = inner.event_tx.send(MobileEvent::DeviceStateChanged {
                    device_id: device.id.clone(),
                    from: old.state,
                    to: device.state,
                });
                inner.devices.insert(device.id.clone(), device);
            }
            Some(_) => {
                inner.devices.insert(device.id.clone(), device);
            }
        }
    }

    /// Remove a device and emit `DeviceDisconnected`.
    pub async fn remove(&self, device_id: &str, reason: &str) {
        let mut inner = self.inner.lock().await;
        if inner.devices.remove(device_id).is_some() {
            tracing::info!(device_id = %device_id, reason = %reason, "device disconnected");
            let _ = inner.event_tx.send(MobileEvent::DeviceDisconnected {
                device_id: device_id.to_string(),
                reason: reason.to_string(),
            });
        }
    }

    /// Get a snapshot of all known devices.
    pub async fn list(&self) -> Vec<MobileDevice> {
        let inner = self.inner.lock().await;
        inner.devices.values().cloned().collect()
    }

    /// Get a single device by ID.
    pub async fn get(&self, device_id: &str) -> MobileResult<MobileDevice> {
        let inner = self.inner.lock().await;
        inner
            .devices
            .get(device_id)
            .cloned()
            .ok_or_else(|| MobileError::DeviceNotFound {
                device_id: device_id.to_string(),
            })
    }

    /// Transition a device to `Busy` if it is currently `Ready`. Returns an
    /// error if the device is not ready.
    pub async fn acquire(&self, device_id: &str) -> MobileResult<()> {
        let mut inner = self.inner.lock().await;
        let device =
            inner
                .devices
                .get_mut(device_id)
                .ok_or_else(|| MobileError::DeviceNotFound {
                    device_id: device_id.to_string(),
                })?;
        match device.state {
            DeviceState::Ready => {
                let old = device.state;
                device.state = DeviceState::Busy;
                let _ = inner.event_tx.send(MobileEvent::DeviceStateChanged {
                    device_id: device_id.to_string(),
                    from: old,
                    to: DeviceState::Busy,
                });
                Ok(())
            }
            DeviceState::Busy => Err(MobileError::DeviceBusy {
                device_id: device_id.to_string(),
            }),
            other => Err(MobileError::DeviceNotReady {
                device_id: device_id.to_string(),
                state: other,
            }),
        }
    }

    /// Release a device from `Busy` back to `Ready`.
    pub async fn release(&self, device_id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(device) = inner.devices.get_mut(device_id) {
            if device.state == DeviceState::Busy {
                let old = device.state;
                device.state = DeviceState::Ready;
                let _ = inner.event_tx.send(MobileEvent::DeviceStateChanged {
                    device_id: device_id.to_string(),
                    from: old,
                    to: DeviceState::Ready,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_mobile_core::*;

    fn test_device(id: &str, state: DeviceState) -> MobileDevice {
        MobileDevice {
            id: id.into(),
            name: "Test".into(),
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

    #[tokio::test]
    async fn upsert_new_device_emits_discovered() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = DeviceRegistry::new(tx);
        reg.upsert(test_device("dev-1", DeviceState::Ready)).await;
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, MobileEvent::DeviceDiscovered { .. }));
        assert_eq!(reg.list().await.len(), 1);
    }

    #[tokio::test]
    async fn upsert_state_change_emits_event() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = DeviceRegistry::new(tx);
        reg.upsert(test_device("dev-1", DeviceState::Connecting))
            .await;
        let _ = rx.recv().await;
        reg.upsert(test_device("dev-1", DeviceState::Ready)).await;
        let event = rx.recv().await.unwrap();
        assert!(matches!(
            event,
            MobileEvent::DeviceStateChanged {
                from: DeviceState::Connecting,
                to: DeviceState::Ready,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn remove_emits_disconnected() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = DeviceRegistry::new(tx);
        reg.upsert(test_device("dev-1", DeviceState::Ready)).await;
        let _ = rx.recv().await;
        reg.remove("dev-1", "usb disconnected").await;
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, MobileEvent::DeviceDisconnected { .. }));
        assert!(reg.list().await.is_empty());
    }

    #[tokio::test]
    async fn acquire_release_cycle() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = DeviceRegistry::new(tx);
        reg.upsert(test_device("dev-1", DeviceState::Ready)).await;

        reg.acquire("dev-1").await.unwrap();
        assert_eq!(reg.get("dev-1").await.unwrap().state, DeviceState::Busy);

        reg.acquire("dev-1").await.unwrap_err();

        reg.release("dev-1").await;
        assert_eq!(reg.get("dev-1").await.unwrap().state, DeviceState::Ready);
    }

    #[tokio::test]
    async fn acquire_not_ready_returns_error() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let reg = DeviceRegistry::new(tx);
        reg.upsert(test_device("dev-1", DeviceState::Unauthorized))
            .await;
        let err = reg.acquire("dev-1").await.unwrap_err();
        assert!(matches!(err, MobileError::DeviceNotReady { .. }));
    }
}
