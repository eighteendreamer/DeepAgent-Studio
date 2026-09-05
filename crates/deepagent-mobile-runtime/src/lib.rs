//! Device state machine, operation lifecycle and backend orchestration for the
//! DeepAgent Mobile subsystem.
//!
//! This crate owns:
//! - The `MobileBackend` trait that platform backends implement.
//! - `DeviceRegistry`, the single source of truth for device state.
//! - `OperationContext` for cancellation, timeout and audit correlation.
//!
//! It does **not** own persistence, Tauri commands, UI DTOs, or agent tool
//! registration.

mod backend;
mod device_registry;
mod discovery;
mod operation;
mod snapshot_store;

pub use backend::MobileBackend;
pub use device_registry::DeviceRegistry;
pub use discovery::{run_discovery_loop, DiscoveryConfig};
pub use operation::{OperationContext, OperationHandle};
pub use snapshot_store::SnapshotStore;
