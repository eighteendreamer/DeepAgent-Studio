//! # deepagent-security
//!
//! Zero-trust security for the agent runtime (开发计划.md Phase 9; 开发提示词.md
//! §16).
//!
//! - [`secrets`] — a dependency-free [`secrets::SecretScanner`] that detects
//!   hard-coded credentials (AWS keys, private keys, bearer/Slack tokens,
//!   high-entropy secret assignments) before they are committed, logged, or sent
//!   to a model. Findings are masked so the scanner never leaks values.
//! - [`gate`] — a [`gate::CiGate`] that aggregates security/quality checks into
//!   a single pass/block decision (blocking vs. advisory), the automated CI
//!   gate that halts a pipeline on real problems.
//!
//! External scanners (Semgrep, `cargo audit`, dependency/secret scanners) plug
//! in by contributing [`gate::CheckResult`]s; the secret scanner here is the
//! built-in, always-available baseline.

pub mod gate;
pub mod secrets;

pub use gate::{CheckResult, CiGate, GateReport};
pub use secrets::{SecretFinding, SecretScanner, Severity};
