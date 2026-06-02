//! Plan mode (gap-closure spec, Phase 3A): a read-only planning mode.
//!
//! When plan mode is active, the agent may explore (read files, search, inspect
//! git) but every write/mutating tool is denied with a clear failure
//! observation — so the model plans first and only executes after the user (or
//! the model itself) exits plan mode. This mirrors Claude Code's plan mode.
//!
//! Components:
//! - [`PlanMode`] — a cheap, clonable shared flag (`Arc<AtomicBool>`).
//! - [`EnterPlanModeTool`] / [`ExitPlanModeTool`] — Safe tools the model calls
//!   to toggle the flag.
//! - [`PlanModeHook`] — a `BeforeToolUse` hook that denies non-read-only tools
//!   while the flag is set.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_hooks::{DecisionSource, Hook, HookContext, HookData, HookOutcome};
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

/// A shared, clonable plan-mode flag. Cheap to clone (`Arc`); all clones view
/// the same underlying state.
#[derive(Clone, Default)]
pub struct PlanMode(Arc<AtomicBool>);

impl PlanMode {
    /// A fresh flag, inactive (normal mode).
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// Whether plan mode is currently active.
    pub fn is_active(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Set the active state.
    pub fn set(&self, active: bool) {
        self.0.store(active, Ordering::Relaxed);
    }
}

impl std::fmt::Debug for PlanMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlanMode")
            .field("active", &self.is_active())
            .finish()
    }
}

/// Tool names that remain allowed while plan mode is active: read-only
/// inspection + the plan-mode toggles themselves + planning bookkeeping.
/// Everything else (writes, commits, bash, …) is denied.
pub const PLAN_SAFE_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "glob",
    "grep",
    "web_search",
    "web_fetch",
    "knowledge_search",
    "git_status",
    "git_diff",
    "git_log",
    "todo_write",
    "task_list",
    "ask_user_question",
    "enter_plan_mode",
    "exit_plan_mode",
];

/// Whether `name` is allowed to run while plan mode is active.
pub fn is_plan_safe_tool(name: &str) -> bool {
    PLAN_SAFE_TOOLS.contains(&name)
}

/// `enter_plan_mode`: switch the session into read-only planning mode.
#[derive(Debug)]
pub struct EnterPlanModeTool {
    plan: PlanMode,
}

impl EnterPlanModeTool {
    /// Build over the shared plan-mode flag.
    pub fn new(plan: PlanMode) -> Self {
        Self { plan }
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "enter_plan_mode".into(),
            description: "Switch to read-only PLAN mode: you may read/search/inspect but all \
                write operations are disabled. Use this to plan before executing. No args."
                .into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::from_iter_perms([Permission::ReadOnly]),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        self.plan.set(true);
        Ok(ToolOutput::success(serde_json::json!({
            "plan_mode": true,
            "message": "Entered plan mode. Write operations are now disabled; explore and propose a plan, then call exit_plan_mode to execute."
        })))
    }
}

/// `exit_plan_mode`: leave planning mode and restore normal (write) permissions.
#[derive(Debug)]
pub struct ExitPlanModeTool {
    plan: PlanMode,
}

impl ExitPlanModeTool {
    /// Build over the shared plan-mode flag.
    pub fn new(plan: PlanMode) -> Self {
        Self { plan }
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "exit_plan_mode".into(),
            description: "Leave PLAN mode and restore normal permissions so write operations \
                can run again. Call this once the plan is ready to execute. No args."
                .into(),
            parameters: serde_json::json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::from_iter_perms([Permission::ReadOnly]),
        }
    }

    async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
        self.plan.set(false);
        Ok(ToolOutput::success(serde_json::json!({
            "plan_mode": false,
            "message": "Exited plan mode. Normal permissions restored."
        })))
    }
}

/// A `BeforeToolUse` hook that denies any non-read-only tool while plan mode is
/// active. The denial becomes a failure observation the model sees, nudging it
/// to keep planning (or exit plan mode) rather than prompting for approval.
#[derive(Debug)]
pub struct PlanModeHook {
    plan: PlanMode,
}

impl PlanModeHook {
    /// Build over the shared plan-mode flag.
    pub fn new(plan: PlanMode) -> Self {
        Self { plan }
    }
}

#[async_trait]
impl Hook for PlanModeHook {
    fn name(&self) -> &str {
        "plan_mode_guard"
    }

    async fn run(&self, ctx: &HookContext) -> Result<HookOutcome> {
        if !self.plan.is_active() {
            return Ok(HookOutcome::Continue);
        }
        if let HookData::Tool { name, .. } = &ctx.data {
            if !is_plan_safe_tool(name) {
                return Ok(HookOutcome::deny_from(
                    format!(
                        "Plan mode: write operations are disabled (tool '{name}' blocked). \
                         Keep exploring read-only, or call exit_plan_mode to execute."
                    ),
                    DecisionSource::Policy,
                ));
            }
        }
        Ok(HookOutcome::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::id::SessionId;
    use deepagent_hooks::HookPoint;

    fn tool_ctx(name: &str) -> HookContext {
        HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool(name, serde_json::json!({})),
        )
    }

    #[tokio::test]
    async fn enter_and_exit_toggle_flag() {
        let plan = PlanMode::new();
        assert!(!plan.is_active());

        let enter = EnterPlanModeTool::new(plan.clone());
        enter.invoke(serde_json::json!({})).await.unwrap();
        assert!(plan.is_active());

        let exit = ExitPlanModeTool::new(plan.clone());
        exit.invoke(serde_json::json!({})).await.unwrap();
        assert!(!plan.is_active());
    }

    #[tokio::test]
    async fn hook_allows_everything_when_inactive() {
        let plan = PlanMode::new();
        let hook = PlanModeHook::new(plan);
        let out = hook.run(&tool_ctx("write_file")).await.unwrap();
        assert_eq!(out, HookOutcome::Continue);
    }

    #[tokio::test]
    async fn hook_denies_writes_when_active() {
        let plan = PlanMode::new();
        plan.set(true);
        let hook = PlanModeHook::new(plan);

        // A write tool is denied.
        let out = hook.run(&tool_ctx("write_file")).await.unwrap();
        assert!(out.is_deny());
        assert!(out.deny_reason().unwrap().contains("Plan mode"));

        // git_commit / bash are also denied.
        assert!(hook.run(&tool_ctx("git_commit")).await.unwrap().is_deny());
        assert!(hook.run(&tool_ctx("bash")).await.unwrap().is_deny());
    }

    #[tokio::test]
    async fn hook_allows_readonly_when_active() {
        let plan = PlanMode::new();
        plan.set(true);
        let hook = PlanModeHook::new(plan);
        for name in ["read_file", "grep", "git_status", "exit_plan_mode"] {
            assert_eq!(
                hook.run(&tool_ctx(name)).await.unwrap(),
                HookOutcome::Continue,
                "{name} should be allowed in plan mode"
            );
        }
    }
}
