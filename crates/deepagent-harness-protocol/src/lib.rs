//! Stable machine-facing contracts for the DeepAgent Harness.
//!
//! This crate owns wire DTOs and the one-way projection from the existing
//! runtime event stream. It intentionally does not own execution, persistence,
//! approvals, cancellation, or tool registration.

mod events;
mod requests;
mod results;

pub use deepagent_core::session_catalog::{
    SessionSortDirection, SessionSortKey, DEFAULT_SESSION_PAGE_SIZE, MAX_SESSION_PAGE_SIZE,
};
pub use events::{project_runtime_event, EventContext, HarnessEvent, ItemPayload};
pub use requests::{
    ApprovalRespondRequest, ConfigReadRequest, EventAckRequest, HarnessRequest,
    InitializeCapabilities, InitializeRequest, InitializeResult, RpcError, RpcNotification,
    RpcRequest, RpcResponse, SandboxStatusRequest, ThreadArchiveRequest, ThreadForkRequest,
    ThreadListRequest, ThreadReadRequest, ThreadResumeRequest, ThreadStartRequest,
    ThreadUnarchiveRequest, ToolListRequest, TurnInterruptRequest, TurnStartRequest,
    TurnSteerRequest, JSONRPC_VERSION, PROTOCOL_VERSION,
};
pub use results::{
    control_result_identity, decode_thread_list_page, validate_control_result,
    ApprovalRespondResult, ControlResultError, EventAckResult, ThreadArchiveResult,
    ThreadForkResult, ThreadLifecycleResult, ThreadListEntry, ThreadListPage, TurnResult,
    TurnSteerResult,
};

/// Version of the durable run/action/approval projection returned by
/// `thread/read`.
pub const CONTROL_PROJECTION_VERSION: u32 = 1;
