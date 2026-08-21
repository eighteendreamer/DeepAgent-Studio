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
