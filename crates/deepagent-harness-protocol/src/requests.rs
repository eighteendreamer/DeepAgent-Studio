use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The current version of the machine protocol.
pub const PROTOCOL_VERSION: u32 = 1;

/// JSON-RPC envelope version, independent of the Harness machine protocol version.
pub const JSONRPC_VERSION: &str = "2.0";

/// Requests accepted by CLI/app-server transports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum HarnessRequest {
    #[serde(rename = "initialize")]
    Initialize(InitializeRequest),
    #[serde(rename = "thread/start")]
    ThreadStart(ThreadStartRequest),
    #[serde(rename = "thread/resume")]
    ThreadResume(ThreadResumeRequest),
    #[serde(rename = "thread/list")]
    ThreadList(ThreadListRequest),
    #[serde(rename = "thread/read")]
    ThreadRead(ThreadReadRequest),
    #[serde(rename = "thread/fork")]
    ThreadFork(ThreadForkRequest),
    #[serde(rename = "thread/archive")]
    ThreadArchive(ThreadArchiveRequest),
    #[serde(rename = "thread/unarchive")]
    ThreadUnarchive(ThreadUnarchiveRequest),
    #[serde(rename = "turn/start")]
    TurnStart(TurnStartRequest),
    #[serde(rename = "turn/interrupt")]
    TurnInterrupt(TurnInterruptRequest),
    #[serde(rename = "turn/steer")]
    TurnSteer(TurnSteerRequest),
    #[serde(rename = "approval/respond")]
    ApprovalRespond(ApprovalRespondRequest),
    #[serde(rename = "event/ack")]
    EventAck(EventAckRequest),
    #[serde(rename = "tool/list")]
    ToolList(ToolListRequest),
    #[serde(rename = "config/read")]
    ConfigRead(ConfigReadRequest),
    #[serde(rename = "sandbox/status")]
    SandboxStatus(SandboxStatusRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializeRequest {
    #[serde(rename = "clientName")]
    pub client_name: String,
    #[serde(rename = "clientVersion")]
    pub client_version: String,
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
}

/// Versioned success result of `initialize`. Unknown extension fields are allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub server_name: String,
    pub server_version: String,
    pub capabilities: InitializeCapabilities,
}

/// Required capability declarations, not grants or guarantees of execution readiness.
/// A false value is a valid declaration of an unsupported capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeCapabilities {
    pub thread_lifecycle: bool,
    pub streaming: bool,
    pub interrupt: bool,
    pub steer: bool,
    pub approval: bool,
    pub reconnect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStartRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Explicit thread default; individual turns may override it without
    /// changing the durable session configuration. Uses the shared model resolver.
    #[serde(
        rename = "reasoningEffort",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_effort: Option<String>,
    #[serde(
        rename = "permissionProfile",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_profile: Option<String>,
    #[serde(
        rename = "sandboxBackend",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sandbox_backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadResumeRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ThreadListRequest {
    /// Omit for active threads; true selects only archived identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadReadRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    /// Total page items (session events, run envelopes and control rows).
    /// An indivisible source/native group must fit; default 500, range 1..=500.
    #[serde(rename = "pageSize", default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<usize>,
    /// Complete successful JSON-RPC response bytes; default 256 KiB,
    /// range 4096..=1048576. An oversized atomic item fails explicitly.
    #[serde(rename = "maxBytes", default, skip_serializing_if = "Option::is_none")]
    pub max_bytes: Option<usize>,
    /// Opaque history-only continuation, scoped to the database and thread.
    /// Retry is read-only; omit the initial sequence filters or repeat them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(
        rename = "afterSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub after_sequence: Option<u64>,
    #[serde(
        rename = "sessionAfterSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub session_after_sequence: Option<u64>,
    #[serde(
        rename = "runAfterSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub run_after_sequence: Option<u64>,
    #[serde(
        rename = "runAfterSequences",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub run_after_sequences: Option<BTreeMap<String, u64>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadForkRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(
        rename = "atSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub at_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadArchiveRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadUnarchiveRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub input: String,
    /// Optional comparison guard from scoped config/read. Not an execution grant.
    #[serde(
        rename = "expectedConfigSnapshotId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expected_config_snapshot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(
        rename = "reasoningEffort",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_effort: Option<String>,
    #[serde(
        rename = "permissionProfile",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub permission_profile: Option<String>,
    #[serde(
        rename = "sandboxBackend",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sandbox_backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInterruptRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "turnId")]
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSteerRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(rename = "turnId")]
    pub turn_id: String,
    pub input: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRespondRequest {
    #[serde(rename = "approvalId")]
    pub approval_id: String,
    pub approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Advisory receipt bounded by this connection's flushed notification high-water.
/// Not a durable replay cursor; use ThreadReadRequest's session/run cursors after
/// reconnecting. Sequence ordering under concurrent producers is separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAckRequest {
    #[serde(rename = "eventSequence")]
    pub event_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolListRequest {
    #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(rename = "runId", default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Compare the immutable admission snapshot identity, not later wire/schema
    /// generations. Requires thread or run scope; it is not an authorization.
    #[serde(
        rename = "expectedSnapshotId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub expected_snapshot_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConfigReadRequest {
    /// Omit both selectors for host-workspace defaults (including its project
    /// layers), not the most recently created thread. Selectors are exclusive.
    #[serde(rename = "threadId", default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SandboxStatusRequest {}

/// JSON-RPC 2.0 request envelope used by the stdio app-server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC 2.0 error payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// JSON-RPC notification envelope used for streamed Harness events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(
        rename = "eventSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub event_sequence: Option<u64>,
    pub params: serde_json::Value,
}

impl RpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    pub fn is_success(&self) -> bool {
        self.error.is_none() && self.result.is_some()
    }

    pub fn error_code(&self) -> Option<i32> {
        self.error.as_ref().map(|error| error.code)
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error.as_ref().map(|error| error.message.as_str())
    }

    pub fn result(&self) -> &serde_json::Value {
        self.result
            .as_ref()
            .expect("RPC response does not contain a result")
    }
}
