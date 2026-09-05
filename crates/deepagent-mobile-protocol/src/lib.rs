//! Stable DTOs, commands, events and versioning for the DeepAgent Mobile
//! subsystem.
//!
//! This crate defines the wire-level types that cross crate boundaries:
//! operation requests, UI tree structures, mobile events, and the protocol
//! version. It depends only on `deepagent-mobile-core`.

mod app_sdk;
mod artifact;
mod events;
mod framework;
mod ios;
mod network;
mod operations;
mod remote_mac;
mod ui;

pub use app_sdk::{
    AppLifecycleState, AppSdkEnvelope, AppSdkKind, AppSdkPayload, ConsoleLogEntry, ConsoleLogLevel,
    HelloPayload, LifecyclePayload, NetworkRecordPayload, SdkCapabilities,
    APP_SDK_PROTOCOL_VERSION,
};
pub use artifact::{
    ArtifactKind, ArtifactLifecycle, ArtifactListRequest, ArtifactListResponse,
    ArtifactPurgeRequest, ArtifactPurgeResult, ArtifactQuery, ArtifactRecord, MAX_ARTIFACT_SIZE,
};
pub use events::MobileEvent;
pub use framework::{
    BusinessEvent, ComponentNode, ComponentTree, DebugProfile, FrameworkKind, SdkManifest,
};
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
pub use remote_mac::{
    validate_config_no_plaintext_secret, CancellationSource, RemoteMacConfig, RemoteMacEvent,
    RemoteMacHealth, RemoteMacMethod, RemoteMacRequest, RemoteMacResponse, RemoteMacState,
};
pub use ui::{Bounds, UiNode, UiNodeSource, UiRole, UiSnapshot};

/// Protocol version for the mobile subsystem. Bump on breaking changes.
pub const MOBILE_PROTOCOL_VERSION: u32 = 1;
