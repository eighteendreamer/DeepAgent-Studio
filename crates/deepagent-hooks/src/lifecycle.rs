//! Hook lifecycle points and the data carried at each.
//!
//! Mirrors the lifecycle in 开发提示词.md §13:
//!
//! | Hook                | 时机     |
//! | ------------------- | -------- |
//! | SessionStart        | 会话开始  |
//! | BeforePlan          | 规划前   |
//! | BeforeToolUse       | 工具前   |
//! | AfterToolUse        | 工具后   |
//! | BeforeCompact       | 压缩前   |
//! | BeforeResponse      | 输出前   |
//! | VerificationFailed  | 验证失败 |
//! | SessionEnd          | 会话结束 |

use serde::{Deserialize, Serialize};

use deepagent_core::id::SessionId;

/// A point in the runtime loop where hooks may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPoint {
    /// Fired once when a session is created / resumed.
    SessionStart,
    InstructionsLoaded,
    /// Fired when a user prompt is submitted, before it becomes a task. Hooks
    /// here may **deny** (reject the input) or **modify** (rewrite/augment it).
    UserPromptSubmit,
    /// Before the planner decides the next move.
    BeforePlan,
    /// Before a tool is executed. Hooks here may **deny** the call.
    BeforeToolUse,
    PermissionRequest,
    PermissionDenied,
    /// After a tool has executed (success or failure).
    AfterToolUse,
    PostToolUseFailure,
    PostToolBatch,
    /// Before the context pipeline compacts.
    BeforeCompact,
    PostCompact,
    /// Before the assistant response is emitted to the user.
    BeforeResponse,
    Stop,
    StopFailure,
    SubagentStart,
    SubagentStop,
    TaskCreated,
    TaskCompleted,
    WorktreeCreate,
    WorktreeRemove,
    CwdChanged,
    FileChanged,
    Notification,
    /// After a verification step failed (build/test/lint).
    VerificationFailed,
    /// Fired once when a session ends.
    SessionEnd,
}

impl HookPoint {
    /// Stable string label.
    pub const fn label(&self) -> &'static str {
        match self {
            HookPoint::SessionStart => "session_start",
            HookPoint::InstructionsLoaded => "instructions_loaded",
            HookPoint::UserPromptSubmit => "user_prompt_submit",
            HookPoint::BeforePlan => "before_plan",
            HookPoint::BeforeToolUse => "before_tool_use",
            HookPoint::PermissionRequest => "permission_request",
            HookPoint::PermissionDenied => "permission_denied",
            HookPoint::AfterToolUse => "after_tool_use",
            HookPoint::PostToolUseFailure => "post_tool_use_failure",
            HookPoint::PostToolBatch => "post_tool_batch",
            HookPoint::BeforeCompact => "before_compact",
            HookPoint::PostCompact => "post_compact",
            HookPoint::BeforeResponse => "before_response",
            HookPoint::Stop => "stop",
            HookPoint::StopFailure => "stop_failure",
            HookPoint::SubagentStart => "subagent_start",
            HookPoint::SubagentStop => "subagent_stop",
            HookPoint::TaskCreated => "task_created",
            HookPoint::TaskCompleted => "task_completed",
            HookPoint::WorktreeCreate => "worktree_create",
            HookPoint::WorktreeRemove => "worktree_remove",
            HookPoint::CwdChanged => "cwd_changed",
            HookPoint::FileChanged => "file_changed",
            HookPoint::Notification => "notification",
            HookPoint::VerificationFailed => "verification_failed",
            HookPoint::SessionEnd => "session_end",
        }
    }

    /// Whether hooks at this point are allowed to deny / halt the operation.
    /// Only the "before"/submission gates are vetoable; observational points
    /// are not.
    pub const fn is_vetoable(&self) -> bool {
        matches!(
            self,
            HookPoint::UserPromptSubmit
                | HookPoint::BeforePlan
                | HookPoint::BeforeToolUse
                | HookPoint::BeforeCompact
                | HookPoint::BeforeResponse
                | HookPoint::PermissionRequest
                | HookPoint::PostToolBatch
                | HookPoint::Stop
                | HookPoint::SubagentStop
                | HookPoint::WorktreeCreate
        )
    }
}

/// Context passed to a hook when it fires. Variants carry point-specific data.
#[derive(Debug, Clone, PartialEq)]
pub struct HookContext {
    /// The session the hook is firing within.
    pub session_id: SessionId,
    /// The lifecycle point.
    pub point: HookPoint,
    /// Point-specific payload.
    pub data: HookData,
    /// Host-owned common input. Absent for legacy/custom callers that have no
    /// real transcript; serializers never manufacture a path from a session id.
    pub base_input: Option<HookBaseInput>,
}

/// A real host workspace and its explicitly versioned, non-authoritative view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookBaseInput {
    /// Absolute execution workspace, not the application's ambient directory.
    pub cwd: String,
    /// Absolute readable snapshot file, not a directory or compressed log.
    pub transcript_path: String,
    /// Local format identifier; this does not claim Claude message JSONL.
    pub transcript_format: String,
}

impl HookContext {
    /// Build a context.
    pub fn new(session_id: SessionId, point: HookPoint, data: HookData) -> Self {
        Self {
            session_id,
            point,
            data,
            base_input: None,
        }
    }

    /// Attach a host-prepared common input without changing event identity.
    pub fn with_base_input(mut self, base: HookBaseInput) -> Self {
        self.base_input = Some(base);
        self
    }
}

/// Semantic source supplied by the session lifecycle owner. These wire values
/// describe a real start; selecting one does not itself dispatch a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionStartSource {
    Startup,
    Resume,
    Clear,
    Compact,
    Fork,
}

/// External lifecycle reason, distinct from the authoritative event log's
/// internal reason. Explicit persistent-session /end maps to Other, not to an
/// application exit, logout, or clearing the chat surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionEndReason {
    Clear,
    Resume,
    Logout,
    PromptInputExit,
    Other,
}

/// Instruction category exposed by the Claude-compatible load hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionMemoryType {
    User,
    Project,
}

impl InstructionMemoryType {
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Project => "Project",
        }
    }
}

/// Reason an instruction file entered the current model context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionLoadReason {
    SessionStart,
    NestedTraversal,
}

impl InstructionLoadReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::NestedTraversal => "nested_traversal",
        }
    }
}

/// Completion gate that rejected a candidate, not a failed model request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionHook {
    BeforeResponse,
    Stop,
}

/// Native StopFailure facts. DeepSeek Harness retains structured failures but
/// does not implement this hook; this projection is a local, versioned adapter,
/// not Claude's assistant-error enum. Optional provider facts stay optional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
#[non_exhaustive]
pub enum StopFailureError {
    HookRejection {
        hook_point: CompletionHook,
        message: String,
    },
    Provider {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
}

/// Point-specific data carried by a [`HookContext`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum HookData {
    /// No additional data (plan gates or legacy custom callers). Serializers
    /// must not guess a session lifecycle source/reason from this variant.
    None,
    SessionStart {
        source: SessionStartSource,
    },
    SessionEnd {
        reason: SessionEndReason,
    },
    /// A user prompt was submitted (carried at
    /// [`HookPoint::UserPromptSubmit`]).
    Prompt {
        /// The submitted prompt text (post-dispatch effective prompt).
        text: String,
    },
    /// A tool is about to run / has run.
    Tool {
        /// Tool name.
        name: String,
        /// JSON arguments.
        arguments: serde_json::Value,
        /// For [`HookPoint::AfterToolUse`], whether it succeeded.
        ok: Option<bool>,
        /// Provider or runtime call identity, when available.
        call_id: Option<String>,
        /// Canonical result exposed to the caller after normal runtime policies.
        result: Option<serde_json::Value>,
        /// Failure detail for PostToolUseFailure.
        error: Option<String>,
    },
    /// A batch of tool calls finished in one model turn.
    ToolBatch {
        /// Tool results in the model's original call order.
        tools: Vec<ToolBatchItem>,
    },
    /// A verification step failed.
    Verification {
        /// The command / check that failed.
        command: String,
        /// Captured failure detail.
        detail: String,
    },
    /// A response is about to be emitted.
    Response {
        /// The candidate response text.
        content: String,
    },
    /// Legacy display content plus the failure owner's structured facts.
    StopFailure {
        content: String,
        error: StopFailureError,
    },
    Instructions {
        paths: Vec<String>,
    },
    /// One newly loaded instruction file and its known source semantics.
    InstructionsLoaded {
        file_path: String,
        memory_type: InstructionMemoryType,
        load_reason: InstructionLoadReason,
    },
    Permission {
        tool: String,
        arguments: serde_json::Value,
        reason: String,
    },
    /// A tool invocation rejected before execution, with its stable call identity.
    PermissionDenied {
        call_id: String,
        tool: String,
        arguments: serde_json::Value,
        reason: String,
    },
    Compact {
        trigger: String,
        summary: Option<String>,
    },
    Subagent {
        agent_id: String,
        agent_type: String,
        summary: Option<String>,
    },
    Task {
        task_id: String,
        subject: String,
    },
    FileChange {
        path: String,
        kind: String,
    },
    Path {
        path: String,
    },
    Notification {
        message: String,
    },
}

/// One entry in a [`HookData::ToolBatch`] payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolBatchItem {
    /// Tool name.
    pub name: String,
    /// Model/tool call id, when available.
    pub call_id: Option<String>,
    /// Validated JSON arguments that were executed.
    pub arguments: serde_json::Value,
    /// Whether the tool result was successful.
    pub ok: bool,
    /// Bounded preview of the structured output. Large outputs should already
    /// be artifact-backed before this payload is built.
    pub output_preview: serde_json::Value,
}

impl HookData {
    /// Convenience constructor for a user-prompt-submit payload.
    pub fn prompt(text: impl Into<String>) -> Self {
        HookData::Prompt { text: text.into() }
    }

    /// Convenience constructor for a pre-tool-use payload.
    pub fn before_tool(name: impl Into<String>, arguments: serde_json::Value) -> Self {
        HookData::Tool {
            name: name.into(),
            arguments,
            ok: None,
            call_id: None,
            result: None,
            error: None,
        }
    }

    /// Pre-tool payload retaining the call identity known by the pipeline.
    pub fn before_tool_with_id(
        name: impl Into<String>,
        arguments: serde_json::Value,
        call_id: Option<String>,
    ) -> Self {
        HookData::Tool {
            name: name.into(),
            arguments,
            ok: None,
            call_id,
            result: None,
            error: None,
        }
    }

    /// Convenience constructor for a post-tool-use payload.
    pub fn after_tool(name: impl Into<String>, arguments: serde_json::Value, ok: bool) -> Self {
        HookData::Tool {
            name: name.into(),
            arguments,
            ok: Some(ok),
            call_id: None,
            result: None,
            error: None,
        }
    }

    /// Post-tool payload carrying the canonical value or standard failure text.
    pub fn after_tool_with_result(
        name: impl Into<String>,
        arguments: serde_json::Value,
        call_id: Option<String>,
        ok: bool,
        result: serde_json::Value,
    ) -> Self {
        let error = (!ok)
            .then(|| result.get("error").and_then(serde_json::Value::as_str))
            .flatten()
            .map(str::to_string);
        HookData::Tool {
            name: name.into(),
            arguments,
            ok: Some(ok),
            call_id,
            result: ok.then_some(result),
            error,
        }
    }

    /// Convenience constructor for a post-tool-batch payload.
    pub fn tool_batch(tools: Vec<ToolBatchItem>) -> Self {
        HookData::ToolBatch { tools }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vetoable_points() {
        assert!(HookPoint::BeforeToolUse.is_vetoable());
        assert!(HookPoint::UserPromptSubmit.is_vetoable());
        assert!(!HookPoint::AfterToolUse.is_vetoable());
        assert!(!HookPoint::SessionStart.is_vetoable());
    }

    #[test]
    fn user_prompt_submit_label_and_payload() {
        assert_eq!(HookPoint::UserPromptSubmit.label(), "user_prompt_submit");
        let json = serde_json::to_string(&HookPoint::UserPromptSubmit).unwrap();
        assert_eq!(json, "\"user_prompt_submit\"");
        assert_eq!(
            HookData::prompt("hi"),
            HookData::Prompt { text: "hi".into() }
        );
    }

    #[test]
    fn labels_are_snake_case() {
        assert_eq!(HookPoint::BeforeToolUse.label(), "before_tool_use");
        let json = serde_json::to_string(&HookPoint::SessionEnd).unwrap();
        assert_eq!(json, "\"session_end\"");
    }
}
