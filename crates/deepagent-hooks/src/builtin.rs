//! A small library of ready-to-use hooks.
//!
//! These cover common needs and double as worked examples of the [`Hook`]
//! trait. The most important is [`ToolAllowlistHook`], which implements the
//! access-control pattern: a `BeforeToolUse` hook that denies any tool not on
//! an allow-list.

use std::collections::BTreeSet;

use async_trait::async_trait;

use deepagent_core::error::Result;

use crate::hook::{Hook, HookOutcome};
use crate::lifecycle::{HookContext, HookData};

/// Denies any tool call whose name is not in the allow-list. Intended for
/// registration at [`crate::lifecycle::HookPoint::BeforeToolUse`].
pub struct ToolAllowlistHook {
    allowed: BTreeSet<String>,
}

impl ToolAllowlistHook {
    /// Build from an iterator of allowed tool names.
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }
}

#[async_trait]
impl Hook for ToolAllowlistHook {
    fn name(&self) -> &str {
        "tool_allowlist"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if let HookData::Tool { name, .. } = &ctx.data {
            if !self.allowed.contains(name) {
                return Ok(HookOutcome::Deny(format!(
                    "tool '{name}' is not in the allow-list"
                )));
            }
        }
        Ok(HookOutcome::Continue)
    }
}

/// Denies tool calls whose JSON arguments contain a blocked substring (a crude
/// guard against, e.g., dangerous shell fragments). Demonstrates argument
/// inspection in a `BeforeToolUse` hook.
pub struct ArgumentGuardHook {
    blocked_substrings: Vec<String>,
}

impl ArgumentGuardHook {
    /// Build from blocked substrings.
    pub fn new(blocked: impl IntoIterator<Item = String>) -> Self {
        Self {
            blocked_substrings: blocked.into_iter().collect(),
        }
    }
}

#[async_trait]
impl Hook for ArgumentGuardHook {
    fn name(&self) -> &str {
        "argument_guard"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if let HookData::Tool { arguments, .. } = &ctx.data {
            let serialized = arguments.to_string();
            for needle in &self.blocked_substrings {
                if serialized.contains(needle) {
                    return Ok(HookOutcome::Deny(format!(
                        "argument contains blocked pattern '{needle}'"
                    )));
                }
            }
        }
        Ok(HookOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{HookContext, HookData, HookPoint};
    use deepagent_core::id::SessionId;

    fn tool_ctx(name: &str, args: serde_json::Value) -> HookContext {
        HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool(name, args),
        )
    }

    #[tokio::test]
    async fn allowlist_permits_listed_tool() {
        let hook = ToolAllowlistHook::new(["read_file".to_string(), "list_dir".to_string()]);
        let out = hook
            .run(&tool_ctx("read_file", serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn allowlist_denies_unlisted_tool() {
        let hook = ToolAllowlistHook::new(["read_file".to_string()]);
        let out = hook
            .run(&tool_ctx("rm_rf", serde_json::json!({})))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn argument_guard_blocks_pattern() {
        let hook = ArgumentGuardHook::new(["rm -rf".to_string()]);
        let out = hook
            .run(&tool_ctx("shell", serde_json::json!({"cmd": "rm -rf /"})))
            .await
            .unwrap();
        assert!(out.is_deny());
    }

    #[tokio::test]
    async fn argument_guard_allows_clean_args() {
        let hook = ArgumentGuardHook::new(["rm -rf".to_string()]);
        let out = hook
            .run(&tool_ctx("shell", serde_json::json!({"cmd": "ls -la"})))
            .await
            .unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }
}
