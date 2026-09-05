use serde::{Deserialize, Serialize};
use std::fmt;

/// Structured error for all mobile operations.
///
/// Every variant carries enough context for the runtime to produce a stable
/// error code, an actionable diagnostic message, and an auditable log entry.
#[derive(Debug, thiserror::Error)]
pub enum MobileError {
    #[error("device not found: {device_id}")]
    DeviceNotFound { device_id: String },

    #[error("device not ready: {device_id} (state={state:?})")]
    DeviceNotReady {
        device_id: String,
        state: super::DeviceState,
    },

    #[error("device busy: {device_id}")]
    DeviceBusy { device_id: String },

    #[error("operation cancelled: {operation_id}")]
    Cancelled { operation_id: String },

    #[error("operation timed out after {elapsed_ms}ms: {operation_id}")]
    Timeout {
        operation_id: String,
        elapsed_ms: u64,
    },

    #[error("tool not available: {tool_name}")]
    ToolNotFound { tool_name: String },

    #[error("tool execution failed: {tool_name} exit={exit_code}: {stderr}")]
    ToolExecutionFailed {
        tool_name: String,
        exit_code: i32,
        stderr: String,
    },

    #[error("unauthorized device: {device_id}")]
    Unauthorized { device_id: String },

    #[error("permission denied for operation {operation} on {device_id}")]
    PermissionDenied {
        operation: String,
        device_id: String,
    },

    #[error("stale UI node {node_id} in snapshot {snapshot_id}")]
    StaleUiNode {
        snapshot_id: String,
        node_id: String,
    },

    #[error("artifact too large: {size_bytes} bytes (limit {limit_bytes})")]
    ArtifactTooLarge { size_bytes: u64, limit_bytes: u64 },

    #[error("backend unavailable: {reason}")]
    BackendUnavailable { reason: String },

    #[error("protocol version mismatch: expected {expected}, got {actual}")]
    ProtocolVersionMismatch { expected: String, actual: String },

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias.
pub type MobileResult<T> = Result<T, MobileError>;

/// Machine-readable error codes for wire protocol and UI consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileErrorCode {
    DeviceNotFound,
    DeviceNotReady,
    DeviceBusy,
    Cancelled,
    Timeout,
    ToolNotFound,
    ToolExecutionFailed,
    Unauthorized,
    PermissionDenied,
    StaleUiNode,
    ArtifactTooLarge,
    BackendUnavailable,
    ProtocolVersionMismatch,
    Internal,
}

impl MobileError {
    pub fn error_code(&self) -> MobileErrorCode {
        match self {
            Self::DeviceNotFound { .. } => MobileErrorCode::DeviceNotFound,
            Self::DeviceNotReady { .. } => MobileErrorCode::DeviceNotReady,
            Self::DeviceBusy { .. } => MobileErrorCode::DeviceBusy,
            Self::Cancelled { .. } => MobileErrorCode::Cancelled,
            Self::Timeout { .. } => MobileErrorCode::Timeout,
            Self::ToolNotFound { .. } => MobileErrorCode::ToolNotFound,
            Self::ToolExecutionFailed { .. } => MobileErrorCode::ToolExecutionFailed,
            Self::Unauthorized { .. } => MobileErrorCode::Unauthorized,
            Self::PermissionDenied { .. } => MobileErrorCode::PermissionDenied,
            Self::StaleUiNode { .. } => MobileErrorCode::StaleUiNode,
            Self::ArtifactTooLarge { .. } => MobileErrorCode::ArtifactTooLarge,
            Self::BackendUnavailable { .. } => MobileErrorCode::BackendUnavailable,
            Self::ProtocolVersionMismatch { .. } => MobileErrorCode::ProtocolVersionMismatch,
            Self::Serialization(_) | Self::Internal(_) => MobileErrorCode::Internal,
        }
    }
}

impl fmt::Display for MobileErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = serde_json::to_string(self).unwrap_or_else(|_| "\"unknown\"".into());
        f.write_str(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_stable() {
        let code = MobileErrorCode::StaleUiNode;
        let json = serde_json::to_string(&code).unwrap();
        assert_eq!(json, "\"stale_ui_node\"");
        let back: MobileErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(code, back);
    }

    #[test]
    fn error_to_code_mapping() {
        let err = MobileError::DeviceNotFound {
            device_id: "abc".into(),
        };
        assert_eq!(err.error_code(), MobileErrorCode::DeviceNotFound);
    }
}
