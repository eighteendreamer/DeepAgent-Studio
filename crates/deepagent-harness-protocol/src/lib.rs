//! Stable machine-facing contracts for the DeepAgent Harness.
//!
//! This crate owns wire DTOs and the one-way projection from the existing
//! runtime event stream. It intentionally does not own execution, persistence,
//! approvals, cancellation, or tool registration.

mod events;
mod requests;

pub use events::{project_runtime_event, EventContext, HarnessEvent, ItemPayload};
pub use requests::{
    ApprovalRespondRequest, ConfigReadRequest, HarnessRequest, InitializeRequest, RpcError,
    RpcNotification, RpcRequest, RpcResponse, SandboxStatusRequest, ThreadArchiveRequest,
    ThreadForkRequest, ThreadListRequest, ThreadReadRequest, ThreadResumeRequest,
    ThreadStartRequest, ToolListRequest, TurnInterruptRequest, TurnStartRequest, TurnSteerRequest,
    PROTOCOL_VERSION,
};
