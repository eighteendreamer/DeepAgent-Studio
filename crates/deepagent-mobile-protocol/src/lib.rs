//! Stable DTOs, commands, events and versioning for the DeepAgent Mobile
//! subsystem.
//!
//! This crate defines the wire-level types that cross crate boundaries:
//! operation requests, UI tree structures, mobile events, and the protocol
//! version. It depends only on `deepagent-mobile-core`.

mod events;
mod operations;
mod ui;

pub use events::MobileEvent;
pub use operations::{
    AppTarget, AvdInfo, FindNodeRequest, FindNodeResult, InputAction, InputRequest, InputResult,
    InstallRequest, LaunchRequest, LogPage, LogRecord, LogRequest, MobileOperation,
    MobileOperationKind, NodeFilter, StartEmulatorRequest, StopEmulatorRequest,
};
pub use ui::{Bounds, UiNode, UiNodeSource, UiRole, UiSnapshot};

/// Protocol version for the mobile subsystem. Bump on breaking changes.
pub const MOBILE_PROTOCOL_VERSION: u32 = 1;
