//! Auto-discovered build/test acceptance plans for code tasks (Phase E,
//! CompletionGate deepening).
//!
//! Claude Code verifies a coding task against the project's own toolchain
//! before accepting completion. This module mirrors that: when the prompt
//! implies code changes AND the workspace has a recognized build system, the
//! run gets a [`VerificationPlan`] whose steps run after the model declares
//! completion. Failures feed back as structured observations through the
//! runtime's [`ReflectionEngine`](deepagent_verification::ReflectionEngine)
//! with bounded repair rounds; repeated identical failures give up cleanly.
//!
//! Discovery is intentionally conservative:
//! - Read-only / question prompts produce NO plan (reads need no build).
//! - Only fast, side-effect-free checks are used (`cargo check`,
//!   `tsc --noEmit`, `python -m compileall`); full test suites stay opt-in.

use std::path::Path;
use std::sync::Arc;

use deepagent_runtime::VerificationPlan;
use deepagent_verification::{Command, SystemCommandRunner, VerificationStep};

/// Code-ish prompt markers (EN + ZH). Combined with mutation intent so a
/// prompt like "创建一个 txt 笔记" doesn't trigger a build.
const CODE_HINTS: &[&str] = &[
    "code",
    "compile",
    "build",
    "bug",
    "fix",
    "refactor",
    "implement",
    "function",
    "class",
    "test",
    "重构",
    "修复",
    "编译",
    "实现",
    "函数",
    "代码",
    "接口",
    "模块",
    "单元测试",
    ".rs",
    ".ts",
    ".tsx",
    ".js",
    ".py",
    ".java",
    ".go",
];

/// Mutation verbs common in coding tasks that `CompletionPolicy::from_prompt`
/// (which tracks filesystem effects only) does not treat as create/modify.
const CODE_MUTATION_HINTS: &[&str] = &[
    "fix",
    "repair",
    "implement",
    "refactor",
    "optimize",
    "debug",
    "修复",
    "重构",
    "实现",
    "优化",
    "调试",
    "改造",
];

/// Discover a workspace-level acceptance plan for `prompt` in `root`.
/// Returns `None` when the prompt does not look like a code-mutation task or
/// no recognized build system exists.
pub(crate) fn discover_verification_plan(root: &Path, prompt: &str) -> Option<VerificationPlan> {
    let lower = prompt.to_lowercase();
    let mutation = deepagent_runtime::CompletionPolicy::from_prompt(prompt);
    let has_mutation_intent =
        !mutation.is_empty() || CODE_MUTATION_HINTS.iter().any(|hint| lower.contains(hint));
    if !has_mutation_intent {
        return None;
    }
    if !CODE_HINTS.iter().any(|hint| lower.contains(hint)) {
        return None;
    }

    let steps = discover_steps(root);
    if steps.is_empty() {
        return None;
    }
    Some(VerificationPlan::new(steps, Arc::new(SystemCommandRunner)))
}

/// Build-system detection, first match wins per ecosystem; all steps are
/// fatal (a broken build blocks completion within the repair budget).
fn discover_steps(root: &Path) -> Vec<VerificationStep> {
    let mut steps = Vec::new();
    let dir = root.to_string_lossy().to_string();
    if root.join("Cargo.toml").exists() {
        steps.push(VerificationStep::new(
            "cargo check",
            Command::new("cargo", ["check", "--offline", "--quiet"].map(String::from))
                .in_dir(dir.clone()),
            true,
        ));
    }
    if root.join("tsconfig.json").exists() {
        steps.push(VerificationStep::new(
            "tsc --noEmit",
            Command::new("tsc", ["--noEmit"].map(String::from)).in_dir(dir.clone()),
            true,
        ));
    }
    if steps.is_empty()
        && (root.join("pyproject.toml").exists()
            || root.join("requirements.txt").exists()
            || root.join("setup.py").exists())
    {
        steps.push(VerificationStep::new(
            "python compileall",
            Command::new("python", ["-m", "compileall", "-q", "."].map(String::from)).in_dir(dir),
            true,
        ));
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_prompts_produce_no_plan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        assert!(discover_verification_plan(tmp.path(), "解释一下这个函数的作用").is_none());
        assert!(discover_verification_plan(tmp.path(), "list the files").is_none());
    }

    #[test]
    fn non_code_write_prompts_produce_no_plan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        assert!(
            discover_verification_plan(tmp.path(), "创建一个 notes.txt 写入今天的日程").is_none()
        );
    }

    #[test]
    fn rust_code_task_gets_cargo_check_plan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        let plan =
            discover_verification_plan(tmp.path(), "修复 src/main.rs 里的编译错误并重构这个函数")
                .expect("code task in a cargo workspace must get a plan");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].command.program, "cargo");
        assert!(plan.max_attempts >= 1);
    }

    #[test]
    fn workspace_without_build_system_gets_no_plan() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            discover_verification_plan(tmp.path(), "fix the bug in code and edit main.rs")
                .is_none()
        );
    }

    #[test]
    fn typescript_workspace_gets_tsc_plan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("tsconfig.json"), "{}").unwrap();
        let plan = discover_verification_plan(tmp.path(), "implement the parser function in .ts")
            .expect("ts workspace code task must get a plan");
        assert_eq!(plan.steps[0].command.program, "tsc");
    }
}
