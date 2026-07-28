//! Auto-discovered build/test acceptance plans (Phase E, CompletionGate
//! deepening; intent-layer cleanup 2026-07-28).
//!
//! Claude Code verifies a coding task against the project's own toolchain
//! before accepting completion, and it does so based on WHAT ACTUALLY
//! HAPPENED (non-trivial implementation on this turn), never by grepping the
//! user's prompt for "code"/"compile"/"refactor" keywords. This module
//! mirrors that split:
//! - **Discovery** (here) is purely structural: if the workspace has a
//!   recognized build system, a [`VerificationPlan`] is produced. No prompt
//!   inspection — guessing intent from prompt text is the anti-pattern we
//!   removed.
//! - **Triggering** lives in the runtime loop, which runs the plan only when
//!   the run actually created/modified files (fact-based gate). A pure
//!   question in a cargo repo mutates nothing, so no build runs.
//!
//! Discovery stays conservative on cost: only fast, side-effect-free checks
//! (`cargo check`, `tsc --noEmit`, `python -m compileall`); full test suites
//! stay opt-in.

use std::path::Path;
use std::sync::Arc;

use deepagent_runtime::VerificationPlan;
use deepagent_verification::{Command, SystemCommandRunner, VerificationStep};

/// Discover a workspace-level acceptance plan for `root`.
///
/// Purely structural: returns a plan when a recognized build system exists,
/// `None` otherwise. Whether the plan actually runs is decided by the runtime
/// loop based on real filesystem mutations — this function never inspects the
/// prompt (that was the intent-guessing anti-pattern).
pub(crate) fn discover_verification_plan(root: &Path) -> Option<VerificationPlan> {
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
    fn build_system_yields_plan_regardless_of_prompt() {
        // Discovery is structural, not prompt-driven: a cargo workspace always
        // yields a cargo-check plan. Whether it RUNS is the loop's fact-based
        // (mutation) decision, tested in the runtime crate.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("Cargo.toml"), "[package]").unwrap();
        let plan =
            discover_verification_plan(tmp.path()).expect("a cargo workspace must yield a plan");
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].command.program, "cargo");
        assert!(plan.max_attempts >= 1);
    }

    #[test]
    fn workspace_without_build_system_gets_no_plan() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(discover_verification_plan(tmp.path()).is_none());
    }

    #[test]
    fn typescript_workspace_gets_tsc_plan() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("tsconfig.json"), "{}").unwrap();
        let plan = discover_verification_plan(tmp.path()).expect("ts workspace must yield a plan");
        assert_eq!(plan.steps[0].command.program, "tsc");
    }
}
