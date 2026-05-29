//! The verification engine (开发计划.md Phase 7 §Verification Engine).
//!
//! A [`Verifier`] runs an ordered suite of [`VerificationStep`]s (build, test,
//! lint, …) via a [`CommandRunner`] and produces a [`VerificationReport`]. Each
//! step's failure is classified into a [`FailureKind`] to guide reflection and
//! retry decisions.

use serde::{Deserialize, Serialize};

use deepagent_core::error::Result;

use crate::runner::{Command, CommandRunner};

/// A single check in a verification suite.
#[derive(Debug, Clone)]
pub struct VerificationStep {
    /// Human label (e.g. "build", "test", "lint").
    pub name: String,
    /// The command to run.
    pub command: Command,
    /// Whether a failure of this step should abort the rest of the suite.
    /// (e.g. a failed build makes tests pointless.)
    pub fatal: bool,
}

impl VerificationStep {
    /// Build a step.
    pub fn new(name: impl Into<String>, command: Command, fatal: bool) -> Self {
        Self {
            name: name.into(),
            command,
            fatal,
        }
    }

    /// A fatal build step.
    pub fn build(command: Command) -> Self {
        Self::new("build", command, true)
    }

    /// A non-fatal test step.
    pub fn test(command: Command) -> Self {
        Self::new("test", command, false)
    }

    /// A non-fatal lint step.
    pub fn lint(command: Command) -> Self {
        Self::new("lint", command, false)
    }
}

/// Classification of a verification failure, used to route reflection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Compilation / build error.
    Build,
    /// Test assertion failure.
    Test,
    /// Lint / style violation.
    Lint,
    /// Type error.
    Type,
    /// Could not even run (missing tool, spawn error).
    Infrastructure,
    /// Unrecognized failure.
    Unknown,
}

impl FailureKind {
    /// Heuristically classify a failure from a step name + captured output.
    pub fn classify(step_name: &str, output: &str) -> FailureKind {
        let lower = output.to_lowercase();
        let step = step_name.to_lowercase();

        if lower.contains("error[e")
            || lower.contains("cannot find")
            || lower.contains("mismatched types")
        {
            if lower.contains("mismatched types")
                || lower.contains("expected") && lower.contains("found")
            {
                return FailureKind::Type;
            }
            return FailureKind::Build;
        }
        if lower.contains("test result: failed")
            || lower.contains("assertion")
            || lower.contains("panicked")
        {
            return FailureKind::Test;
        }
        if lower.contains("clippy") || lower.contains("warning:") || step.contains("lint") {
            return FailureKind::Lint;
        }
        if lower.contains("command not found")
            || lower.contains("no such file")
            || lower.contains("failed to spawn")
        {
            return FailureKind::Infrastructure;
        }
        match step.as_str() {
            "build" => FailureKind::Build,
            "test" => FailureKind::Test,
            "lint" => FailureKind::Lint,
            _ => FailureKind::Unknown,
        }
    }
}

/// The result of a single verification step.
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    /// Step name.
    pub name: String,
    /// Whether it passed.
    pub passed: bool,
    /// Classified failure kind (only meaningful when `!passed`).
    pub failure_kind: Option<FailureKind>,
    /// A trimmed excerpt of the failure output.
    pub detail: String,
}

/// The overall report from running a suite.
#[derive(Debug, Clone, PartialEq)]
pub struct VerificationReport {
    /// Per-step results, in execution order.
    pub steps: Vec<StepResult>,
    /// Whether all (executed) steps passed.
    pub passed: bool,
}

impl VerificationReport {
    /// The first failing step, if any.
    pub fn first_failure(&self) -> Option<&StepResult> {
        self.steps.iter().find(|s| !s.passed)
    }

    /// All failing steps.
    pub fn failures(&self) -> Vec<&StepResult> {
        self.steps.iter().filter(|s| !s.passed).collect()
    }
}

/// Runs verification suites.
pub struct Verifier<R: CommandRunner> {
    runner: R,
    /// Max characters of failure output to retain per step.
    detail_limit: usize,
}

impl<R: CommandRunner> Verifier<R> {
    /// Build a verifier over a command runner.
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            detail_limit: 2_000,
        }
    }

    /// Run an ordered suite, stopping early if a `fatal` step fails.
    pub async fn run_suite(&self, steps: &[VerificationStep]) -> Result<VerificationReport> {
        let mut results = Vec::with_capacity(steps.len());
        let mut all_passed = true;

        for step in steps {
            let output = self.runner.run(&step.command).await?;
            if output.success() {
                results.push(StepResult {
                    name: step.name.clone(),
                    passed: true,
                    failure_kind: None,
                    detail: String::new(),
                });
            } else {
                all_passed = false;
                let combined = output.combined();
                let kind = FailureKind::classify(&step.name, &combined);
                let detail: String = combined.chars().take(self.detail_limit).collect();
                tracing::warn!(step = %step.name, ?kind, "verification step failed");
                results.push(StepResult {
                    name: step.name.clone(),
                    passed: false,
                    failure_kind: Some(kind),
                    detail,
                });
                if step.fatal {
                    break; // don't run later steps (e.g. tests after a failed build)
                }
            }
        }

        Ok(VerificationReport {
            steps: results,
            passed: all_passed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    fn suite() -> Vec<VerificationStep> {
        vec![
            VerificationStep::build(Command::parse("cargo build")),
            VerificationStep::test(Command::parse("cargo test")),
            VerificationStep::lint(Command::parse("cargo clippy")),
        ]
    }

    #[tokio::test]
    async fn all_pass_reports_success() {
        let runner = MockRunner::new(); // unknown => success
        let report = Verifier::new(runner).run_suite(&suite()).await.unwrap();
        assert!(report.passed);
        assert_eq!(report.steps.len(), 3);
        assert!(report.first_failure().is_none());
    }

    #[tokio::test]
    async fn fatal_build_failure_stops_suite() {
        let runner = MockRunner::new().with_failure("cargo build", "error[E0277]: trait bound");
        let report = Verifier::new(runner).run_suite(&suite()).await.unwrap();
        assert!(!report.passed);
        // Build failed and is fatal: test/lint were skipped.
        assert_eq!(report.steps.len(), 1);
        assert_eq!(report.steps[0].failure_kind, Some(FailureKind::Build));
    }

    #[tokio::test]
    async fn non_fatal_test_failure_continues() {
        let runner = MockRunner::new().with_failure("cargo test", "test result: FAILED. 1 failed");
        let report = Verifier::new(runner).run_suite(&suite()).await.unwrap();
        assert!(!report.passed);
        // build ok, test failed (non-fatal), lint still ran.
        assert_eq!(report.steps.len(), 3);
        assert_eq!(report.steps[1].failure_kind, Some(FailureKind::Test));
        assert!(report.steps[2].passed);
    }

    #[test]
    fn classification_heuristics() {
        assert_eq!(
            FailureKind::classify("test", "test result: FAILED"),
            FailureKind::Test
        );
        assert_eq!(
            FailureKind::classify("build", "error[E0432]: unresolved import"),
            FailureKind::Build
        );
        assert_eq!(
            FailureKind::classify("lint", "warning: unused variable"),
            FailureKind::Lint
        );
        assert_eq!(
            FailureKind::classify("test", "failed to spawn 'cargo'"),
            FailureKind::Infrastructure
        );
    }
}
