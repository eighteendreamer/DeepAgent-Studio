//! The CI gate (开发计划.md Phase 9: CI Gate — 自动阻断).
//!
//! A [`CiGate`] aggregates the results of independent security/quality
//! [`Check`]s into a single pass/block decision. A check can be *blocking*
//! (failure halts the pipeline) or *advisory* (failure is reported but does not
//! block). This mirrors a zero-trust gate: by default, secret findings and
//! known vulnerabilities block; style nits warn.

use serde::{Deserialize, Serialize};

/// Outcome of one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Check name (e.g. "secret_scan", "cargo_audit").
    pub name: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Whether a failure blocks the pipeline.
    pub blocking: bool,
    /// Number of issues found.
    pub issues: u32,
    /// Human-readable summary.
    pub summary: String,
}

impl CheckResult {
    /// A passing check.
    pub fn pass(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: true,
            blocking: true,
            issues: 0,
            summary: "ok".to_string(),
        }
    }

    /// A failing, blocking check with `issues` problems.
    pub fn fail(name: impl Into<String>, issues: u32, summary: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: false,
            blocking: true,
            issues,
            summary: summary.into(),
        }
    }

    /// Mark this check advisory (failures warn but do not block).
    pub fn advisory(mut self) -> Self {
        self.blocking = false;
        self
    }
}

/// The aggregated gate decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateReport {
    /// All check results.
    pub checks: Vec<CheckResult>,
    /// Whether the pipeline is allowed to proceed.
    pub allowed: bool,
}

impl GateReport {
    /// Checks that failed and are blocking.
    pub fn blocking_failures(&self) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| !c.passed && c.blocking)
            .collect()
    }

    /// Advisory (non-blocking) failures.
    pub fn warnings(&self) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| !c.passed && !c.blocking)
            .collect()
    }

    /// Render a concise multi-line report.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for c in &self.checks {
            let status = if c.passed {
                "PASS"
            } else if c.blocking {
                "BLOCK"
            } else {
                "WARN"
            };
            out.push_str(&format!("[{status}] {} — {}\n", c.name, c.summary));
        }
        out.push_str(if self.allowed {
            "\nGate: ALLOWED"
        } else {
            "\nGate: BLOCKED"
        });
        out
    }
}

/// Aggregates checks into a gate decision.
#[derive(Default)]
pub struct CiGate {
    checks: Vec<CheckResult>,
}

impl CiGate {
    /// New empty gate.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a check result (builder).
    pub fn with(mut self, result: CheckResult) -> Self {
        self.checks.push(result);
        self
    }

    /// Add a check result in place.
    pub fn add(&mut self, result: CheckResult) -> &mut Self {
        self.checks.push(result);
        self
    }

    /// Evaluate the gate: blocked iff any blocking check failed.
    pub fn evaluate(self) -> GateReport {
        let allowed = !self.checks.iter().any(|c| !c.passed && c.blocking);
        GateReport {
            checks: self.checks,
            allowed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_passing_is_allowed() {
        let report = CiGate::new()
            .with(CheckResult::pass("secret_scan"))
            .with(CheckResult::pass("cargo_audit"))
            .evaluate();
        assert!(report.allowed);
        assert!(report.blocking_failures().is_empty());
    }

    #[test]
    fn blocking_failure_blocks() {
        let report = CiGate::new()
            .with(CheckResult::pass("fmt"))
            .with(CheckResult::fail("secret_scan", 2, "2 secrets found"))
            .evaluate();
        assert!(!report.allowed);
        assert_eq!(report.blocking_failures().len(), 1);
    }

    #[test]
    fn advisory_failure_does_not_block() {
        let report = CiGate::new()
            .with(CheckResult::pass("secret_scan"))
            .with(CheckResult::fail("lint", 3, "style nits").advisory())
            .evaluate();
        assert!(report.allowed);
        assert_eq!(report.warnings().len(), 1);
    }

    #[test]
    fn render_marks_statuses() {
        let report = CiGate::new()
            .with(CheckResult::pass("a"))
            .with(CheckResult::fail("b", 1, "bad"))
            .with(CheckResult::fail("c", 1, "meh").advisory())
            .evaluate();
        let text = report.render();
        assert!(text.contains("[PASS] a"));
        assert!(text.contains("[BLOCK] b"));
        assert!(text.contains("[WARN] c"));
        assert!(text.contains("Gate: BLOCKED"));
    }
}
