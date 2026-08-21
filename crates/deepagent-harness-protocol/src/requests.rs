use serde::{Deserialize, Serialize};

/// The current version of the machine protocol.
pub const PROTOCOL_VERSION: u32 = 1;

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
    #[serde(rename = "turn/start")]
    TurnStart(TurnStartRequest),
    #[serde(rename = "turn/interrupt")]
    TurnInterrupt(TurnInterruptRequest),
    #[serde(rename = "turn/steer")]
    TurnSteer(TurnSteerRequest),
    #[serde(rename = "approval/respond")]
    ApprovalRespond(ApprovalRespondRequest),
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStartRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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
pub struct ThreadListRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadReadRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    #[serde(
        rename = "afterSequence",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub after_sequence: Option<u64>,
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
pub struct TurnStartRequest {
    #[serde(rename = "threadId")]
    pub thread_id: String,
    pub input: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ToolListRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ConfigReadRequest {}

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
    pub params: serde_json::Value,
}

impl RpcResponse {
    pub fn success(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
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
