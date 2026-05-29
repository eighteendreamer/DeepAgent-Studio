//! Reflection (开发计划.md Phase 7 §Reflection Engine).
//!
//! When verification fails, the runtime must not blindly retry. The
//! [`ReflectionEngine`] converts a [`VerificationReport`] into a structured
//! [`Reflection`] — a diagnosis plus a suggested next action — that is fed back
//! to the agent. Critically it also performs **loop detection**: if the same
//! failure signature recurs, it recommends stopping rather than retrying
//! forever (the "无限循环可检测" acceptance criterion).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::verifier::{FailureKind, VerificationReport};

/// What the reflection recommends doing next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    /// Everything passed; proceed.
    Proceed,
    /// Retry after attempting a fix (the failure looks addressable).
    Retry,
    /// Stop: the same failure keeps recurring (loop detected) or it is
    /// non-actionable.
    GiveUp,
}

/// A structured reflection on a verification outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reflection {
    /// Recommended next action.
    pub action: NextAction,
    /// Human-readable diagnosis suitable for feeding back to the agent.
    pub diagnosis: String,
    /// The dominant failure kind, if any.
    pub failure_kind: Option<FailureKind>,
    /// The attempt number this reflection corresponds to (1-based).
    pub attempt: u32,
}

/// A compact signature of a failure, used to detect repeats.
fn failure_signature(report: &VerificationReport) -> Option<String> {
    let first = report.first_failure()?;
    // Use step + kind + a normalized prefix of the detail. Normalizing strips
    // volatile bits (line numbers, paths) so "the same error" matches.
    let normalized: String = first
        .detail
        .chars()
        .filter(|c| !c.is_ascii_digit())
        .take(120)
        .collect();
    Some(format!(
        "{}:{:?}:{}",
        first.name,
        first.failure_kind,
        normalized.trim()
    ))
}

/// Drives reflection across retry attempts, remembering past failure
/// signatures to detect loops.
#[derive(Debug, Default)]
pub struct ReflectionEngine {
    /// How many times each failure signature has been seen.
    seen: HashMap<String, u32>,
    /// Attempts taken so far.
    attempts: u32,
    /// Max times the *same* signature may recur before giving up.
    max_repeats: u32,
    /// Absolute cap on attempts regardless of signatures.
    max_attempts: u32,
}

impl ReflectionEngine {
    /// Build an engine with the given loop-detection thresholds.
    pub fn new(max_repeats: u32, max_attempts: u32) -> Self {
        Self {
            seen: HashMap::new(),
            attempts: 0,
            max_repeats,
            max_attempts,
        }
    }

    /// Sensible defaults: a signature may repeat twice; at most 5 attempts.
    pub fn with_defaults() -> Self {
        Self::new(2, 5)
    }

    /// Number of attempts observed so far.
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Reflect on a verification report, advancing the attempt counter and
    /// updating loop-detection state.
    pub fn reflect(&mut self, report: &VerificationReport) -> Reflection {
        self.attempts += 1;

        if report.passed {
            return Reflection {
                action: NextAction::Proceed,
                diagnosis: "All verification steps passed.".to_string(),
                failure_kind: None,
                attempt: self.attempts,
            };
        }

        // Absolute attempt cap.
        if self.attempts >= self.max_attempts {
            return Reflection {
                action: NextAction::GiveUp,
                diagnosis: format!(
                    "Reached the maximum of {} verification attempts without success.",
                    self.max_attempts
                ),
                failure_kind: report.first_failure().and_then(|f| f.failure_kind),
                attempt: self.attempts,
            };
        }

        let first = report.first_failure();
        let kind = first.and_then(|f| f.failure_kind);

        // Loop detection: has this exact failure recurred too often?
        if let Some(sig) = failure_signature(report) {
            let count = self.seen.entry(sig).or_insert(0);
            *count += 1;
            if *count > self.max_repeats {
                return Reflection {
                    action: NextAction::GiveUp,
                    diagnosis: format!(
                        "The same {} failure has recurred {} times; stopping to avoid an \
                         infinite retry loop. Manual intervention is likely needed.",
                        kind.map(|k| format!("{k:?}"))
                            .unwrap_or_else(|| "unknown".into()),
                        count
                    ),
                    failure_kind: kind,
                    attempt: self.attempts,
                };
            }
        }

        Reflection {
            action: NextAction::Retry,
            diagnosis: suggest_fix(first),
            failure_kind: kind,
            attempt: self.attempts,
        }
    }
}

/// Produce a fix suggestion tailored to the failure kind.
fn suggest_fix(failure: Option<&crate::verifier::StepResult>) -> String {
    let Some(f) = failure else {
        return "Verification failed but no failing step was identified.".to_string();
    };
    let head: String = f.detail.lines().take(3).collect::<Vec<_>>().join(" ");
    match f.failure_kind {
        Some(FailureKind::Build) => format!(
            "Build failed. Inspect the compiler error and fix the offending code, then re-run. \
             First lines: {head}"
        ),
        Some(FailureKind::Type) => {
            format!("Type error. Reconcile the expected vs. actual types. First lines: {head}")
        }
        Some(FailureKind::Test) => format!(
            "A test failed. Read the assertion, decide whether the code or the test is wrong, \
             and fix it. First lines: {head}"
        ),
        Some(FailureKind::Lint) => format!(
            "Lint violation. Apply the suggested fix or adjust the code to satisfy the linter. \
             First lines: {head}"
        ),
        Some(FailureKind::Infrastructure) => format!(
            "The verification command could not run (missing tool / wrong path). Fix the \
             environment before retrying. First lines: {head}"
        ),
        _ => format!("Verification failed: {head}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verifier::{StepResult, VerificationReport};

    fn failing_report(detail: &str) -> VerificationReport {
        VerificationReport {
            steps: vec![StepResult {
                name: "build".into(),
                passed: false,
                failure_kind: Some(FailureKind::Build),
                detail: detail.into(),
            }],
            passed: false,
        }
    }

    fn passing_report() -> VerificationReport {
        VerificationReport {
            steps: vec![StepResult {
                name: "build".into(),
                passed: true,
                failure_kind: None,
                detail: String::new(),
            }],
            passed: true,
        }
    }

    #[test]
    fn passing_report_proceeds() {
        let mut engine = ReflectionEngine::with_defaults();
        let r = engine.reflect(&passing_report());
        assert_eq!(r.action, NextAction::Proceed);
        assert_eq!(r.attempt, 1);
    }

    #[test]
    fn first_failure_suggests_retry() {
        let mut engine = ReflectionEngine::with_defaults();
        let r = engine.reflect(&failing_report("error[E0277]: trait bound not satisfied"));
        assert_eq!(r.action, NextAction::Retry);
        assert_eq!(r.failure_kind, Some(FailureKind::Build));
        assert!(r.diagnosis.contains("Build failed"));
    }

    #[test]
    fn repeated_identical_failure_triggers_giveup() {
        let mut engine = ReflectionEngine::new(2, 100);
        // Same failure signature each time (digits stripped, so line numbers
        // don't matter).
        let r1 = engine.reflect(&failing_report("error at line 10: bad"));
        let r2 = engine.reflect(&failing_report("error at line 20: bad"));
        let r3 = engine.reflect(&failing_report("error at line 30: bad"));
        assert_eq!(r1.action, NextAction::Retry);
        assert_eq!(r2.action, NextAction::Retry);
        // Third occurrence exceeds max_repeats=2 -> give up.
        assert_eq!(r3.action, NextAction::GiveUp);
        assert!(r3.diagnosis.contains("infinite retry loop"));
    }

    #[test]
    fn absolute_attempt_cap_triggers_giveup() {
        let mut engine = ReflectionEngine::new(100, 3);
        // Different signatures each time so repeat-detection doesn't fire.
        engine.reflect(&failing_report("alpha"));
        engine.reflect(&failing_report("beta"));
        let r3 = engine.reflect(&failing_report("gamma"));
        assert_eq!(r3.action, NextAction::GiveUp);
        assert!(r3.diagnosis.contains("maximum"));
    }

    #[test]
    fn different_failures_keep_retrying() {
        let mut engine = ReflectionEngine::new(2, 100);
        let r1 = engine.reflect(&failing_report("first kind of problem"));
        let r2 = engine.reflect(&failing_report("totally different issue"));
        assert_eq!(r1.action, NextAction::Retry);
        assert_eq!(r2.action, NextAction::Retry);
    }
}
