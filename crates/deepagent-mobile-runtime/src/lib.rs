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

mod artifact_store;
mod backend;
mod device_registry;
mod discovery;
mod mobile_service;
mod operation;
mod remote_mac;
mod remote_mac_manager;
mod snapshot_store;

pub use artifact_store::{ArtifactStore, ArtifactStoreError, RegisterArtifactRequest};
pub use backend::MobileBackend;
pub use device_registry::DeviceRegistry;
pub use discovery::{run_discovery_loop, DiscoveryConfig};
pub use mobile_service::MobileService;
pub use operation::{OperationContext, OperationHandle};
pub use remote_mac::{
    FakeRemoteTransport, RemoteMacSession, RemoteTransport, RemoteTransportError,
};
pub use remote_mac_manager::{
    AggregatedHealth, MacHealthEntry, RemoteMacManager, RemoteMacManagerError, TransportFactory,
};
pub use snapshot_store::SnapshotStore;
