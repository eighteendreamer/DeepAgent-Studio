//! Stable DTOs, commands, events and versioning for the DeepAgent Mobile
//! subsystem.
//!
//! This crate defines the wire-level types that cross crate boundaries:
//! operation requests, UI tree structures, mobile events, and the protocol
//! version. It depends only on `deepagent-mobile-core`.

mod artifact;
mod events;
mod ios;
mod network;
mod operations;
mod ui;

pub use artifact::{
    ArtifactKind, ArtifactLifecycle, ArtifactListRequest, ArtifactListResponse,
    ArtifactPurgeRequest, ArtifactPurgeResult, ArtifactQuery, ArtifactRecord, MAX_ARTIFACT_SIZE,
};
pub use events::MobileEvent;
pub use ios::{
    classify_ios_error, IosErrorKind, IosToolError, SimDevice, SimDeviceState, SimRuntime,
    SimctlListOutput,
};
pub use network::{
    NetworkRecord, NetworkRequest, NetworkResponse, MAX_BODY_SIZE, SENSITIVE_HEADERS,
};
pub use operations::{
    AppTarget, AvdInfo, FindNodeRequest, FindNodeResult, InputAction, InputRequest, InputResult,
    InstallRequest, LaunchRequest, LogPage, LogRecord, LogRequest, MobileOperation,
    MobileOperationKind, NodeFilter, StartEmulatorRequest, StopEmulatorRequest,
};
pub use ui::{Bounds, UiNode, UiNodeSource, UiRole, UiSnapshot};

/// Protocol version for the mobile subsystem. Bump on breaking changes.
pub const MOBILE_PROTOCOL_VERSION: u32 = 1;
