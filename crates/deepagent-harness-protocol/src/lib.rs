//! Stable machine-facing contracts for the DeepAgent Harness.
//!
//! This crate owns wire DTOs and the one-way projection from the existing
//! runtime event stream. It intentionally does not own execution, persistence,
//! approvals, cancellation, or tool registration.

mod events;
mod requests;

pub use events::{project_runtime_event, EventContext, HarnessEvent, ItemPayload};
pub use requests::{
    ApprovalRespondRequest, ConfigReadRequest, EventAckRequest, HarnessRequest, InitializeRequest,
    RpcError, RpcNotification, RpcRequest, RpcResponse, SandboxStatusRequest,
    ThreadArchiveRequest, ThreadForkRequest, ThreadListRequest, ThreadReadRequest,
    ThreadResumeRequest, ThreadStartRequest, ToolListRequest, TurnInterruptRequest,
    TurnStartRequest, TurnSteerRequest, PROTOCOL_VERSION,
};

/// Version of the durable run/action/approval projection returned by
/// `thread/read`.
pub const CONTROL_PROJECTION_VERSION: u32 = 1;
