//! Platform-agnostic domain types, capabilities and errors for the DeepAgent
//! Mobile subsystem.
//!
//! This crate is the foundation of the mobile crate graph. It defines the
//! vocabulary types that every other mobile crate and every consumer (runtime,
//! protocol, android, ios, app-core, builtins) depends on. It intentionally
//! has **no** dependency on any other `deepagent-mobile-*` crate.

mod device;
mod error;

pub use device::{
    BackendStatus, DeviceCapabilities, DeviceConnection, DeviceKind, DeviceState, MobileDevice,
    MobilePlatform, ToolPath,
};
pub use error::{MobileError, MobileResult};

/// Opaque reference to a persisted binary artifact (screenshot, logcat dump,
/// screen recording, etc.).
///
/// Events never carry raw binary payloads. They carry an `ArtifactRef` that
/// downstream consumers can resolve through the artifact store.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactRef {
    pub artifact_id: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub storage_path: String,
}
