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
    /// Before the planner decides the next move.
    BeforePlan,
    /// Before a tool is executed. Hooks here may **deny** the call.
    BeforeToolUse,
    /// After a tool has executed (success or failure).
    AfterToolUse,
    /// Before the context pipeline compacts.
    BeforeCompact,
    /// Before the assistant response is emitted to the user.
    BeforeResponse,
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
            HookPoint::BeforePlan => "before_plan",
            HookPoint::BeforeToolUse => "before_tool_use",
            HookPoint::AfterToolUse => "after_tool_use",
            HookPoint::BeforeCompact => "before_compact",
            HookPoint::BeforeResponse => "before_response",
            HookPoint::VerificationFailed => "verification_failed",
            HookPoint::SessionEnd => "session_end",
        }
    }

    /// Whether hooks at this point are allowed to deny / halt the operation.
    /// Only the "before" gates are vetoable; observational points are not.
    pub const fn is_vetoable(&self) -> bool {
        matches!(
            self,
            HookPoint::BeforePlan
                | HookPoint::BeforeToolUse
                | HookPoint::BeforeCompact
                | HookPoint::BeforeResponse
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
}

impl HookContext {
    /// Build a context.
    pub fn new(session_id: SessionId, point: HookPoint, data: HookData) -> Self {
        Self {
            session_id,
            point,
            data,
        }
    }
}

/// Point-specific data carried by a [`HookContext`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum HookData {
    /// No additional data (session lifecycle, plan gates).
    None,
    /// A tool is about to run / has run.
    Tool {
        /// Tool name.
        name: String,
        /// JSON arguments.
        arguments: serde_json::Value,
        /// For [`HookPoint::AfterToolUse`], whether it succeeded.
        ok: Option<bool>,
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
}

impl HookData {
    /// Convenience constructor for a pre-tool-use payload.
    pub fn before_tool(name: impl Into<String>, arguments: serde_json::Value) -> Self {
        HookData::Tool {
            name: name.into(),
            arguments,
            ok: None,
        }
    }

    /// Convenience constructor for a post-tool-use payload.
    pub fn after_tool(name: impl Into<String>, arguments: serde_json::Value, ok: bool) -> Self {
        HookData::Tool {
            name: name.into(),
            arguments,
            ok: Some(ok),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vetoable_points() {
        assert!(HookPoint::BeforeToolUse.is_vetoable());
        assert!(!HookPoint::AfterToolUse.is_vetoable());
        assert!(!HookPoint::SessionStart.is_vetoable());
    }

    #[test]
    fn labels_are_snake_case() {
        assert_eq!(HookPoint::BeforeToolUse.label(), "before_tool_use");
        let json = serde_json::to_string(&HookPoint::SessionEnd).unwrap();
        assert_eq!(json, "\"session_end\"");
    }
}
