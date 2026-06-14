//! Post-edit verification decorator (Phase 4B of coding-amplifier).
//!
//! Runs the [`crate::verification_dispatcher::VerificationDispatcher`] against
//! a file written or edited by the runtime's write/edit tools, and attaches a
//! `<system-reminder>` describing the outcome to the tool result. The model
//! sees the reminder on its next THINK step and self-corrects when
//! verification fails.
//!
//! ## Outcomes mapped
//!
//! - `Passed`   → `<system-reminder>verified: <path></system-reminder>`
//! - `Failed`   → `<system-reminder>Verification of <path> failed:\n<detail>\nFix this before continuing.</system-reminder>`
//! - `TimedOut` → `<system-reminder>Verification of <path> timed out (>5s) — verify manually if needed.</system-reminder>`
//! - `Skipped`  → emitted ONCE per toolchain via the dispatcher's
//!   [`crate::verification_dispatcher::VerificationDispatcher::announce_missing_toolchain`]
//!   dedupe — for "no Cargo.toml found" / unknown extension we stay silent.
//!
//! `ok` is **never** flipped here. Strict mode (Phase 4C) flips `ok` at a
//! different layer; Phase 4B's job is purely to surface the outcome.
//!
//! ## Trigger gating
//!
//! - Only runs for tool names: `write_file`, `edit_file`, `multi_edit`.
//! - Only runs when `output.ok == true` (failed writes have no file to verify).
//! - Resolves the file path from the tool's value JSON (`path` field, set by
//!   the file_tools implementations) against an optional workspace root.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use deepagent_runtime::ToolResultDecorator;
use deepagent_tools::ToolOutput;

use crate::settings::VerificationPolicy;
use crate::system_reminder::{append_to_tool_result, wrap};
use crate::verification_dispatcher::{VerificationDispatcher, VerificationOutcome};

/// Tool names that should trigger post-edit verification. All produce or
/// modify files in place.
const VERIFIED_TOOLS: &[&str] = &["write_file", "edit_file", "multi_edit"];

/// Decorator that runs the verification dispatcher on a freshly-written file
/// and attaches a `<system-reminder>` describing the outcome.
#[derive(Debug, Clone)]
pub struct VerificationDecorator {
    dispatcher: Arc<VerificationDispatcher>,
    /// Workspace root used to resolve relative paths from the tool result.
    /// `None` means "trust whatever the tool returned (it's already absolute
    /// or already exists relative to cwd)".
    workspace_root: Option<PathBuf>,
    /// Active verification policy. `Disabled` short-circuits the decorator
    /// (zero overhead). `Strict` flips `ok = false` on confirmed `Failed`
    /// outcomes so the runtime's reflection / recovery path triggers.
    policy: VerificationPolicy,
}

impl VerificationDecorator {
    /// Build over a shared dispatcher and an optional workspace root, with
    /// the default [`VerificationPolicy::Enabled`] policy.
    pub fn new(dispatcher: Arc<VerificationDispatcher>, workspace_root: Option<PathBuf>) -> Self {
        Self::with_policy(dispatcher, workspace_root, VerificationPolicy::default())
    }

    /// Build over a shared dispatcher, an optional workspace root, and an
    /// explicit policy.
    pub fn with_policy(
        dispatcher: Arc<VerificationDispatcher>,
        workspace_root: Option<PathBuf>,
        policy: VerificationPolicy,
    ) -> Self {
        Self {
            dispatcher,
            workspace_root,
            policy,
        }
    }

    /// Erase to a runtime-friendly trait object.
    pub fn into_arc(self) -> Arc<dyn ToolResultDecorator> {
        Arc::new(self)
    }

    /// Resolve the path the tool actually wrote. Returns `None` if the tool
    /// didn't include one (e.g. an aborted edit) or the path can't be made
    /// to point at an existing file.
    fn resolve_target(&self, output: &ToolOutput) -> Option<PathBuf> {
        let raw = output.value.get("path").and_then(|v| v.as_str())?;
        let path = Path::new(raw);
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(root) = self.workspace_root.as_ref() {
            root.join(path)
        } else {
            path.to_path_buf()
        };
        candidate.exists().then_some(candidate)
    }
}

#[async_trait]
impl ToolResultDecorator for VerificationDecorator {
    async fn decorate(&self, tool_name: &str, output: &mut ToolOutput) {
        if !self.policy.is_enabled() {
            return;
        }
        if !VERIFIED_TOOLS.contains(&tool_name) {
            return;
        }
        if !output.ok {
            return;
        }
        let Some(path) = self.resolve_target(output) else {
            return;
        };
        let display = path
            .strip_prefix(self.workspace_root.as_deref().unwrap_or(&path))
            .unwrap_or(&path)
            .display()
            .to_string();
        let outcome = self.dispatcher.verify_file(&path).await;
        match outcome {
            VerificationOutcome::Passed => {
                append_to_tool_result(&mut output.value, &wrap(&format!("verified: {display}")));
            }
            VerificationOutcome::Failed { detail, truncated } => {
                let suffix = if truncated {
                    "\n(detail truncated; run the verifier manually for the full output)"
                } else {
                    ""
                };
                let body = format!(
                    "Verification of {display} failed:\n{detail}{suffix}\nFix this before continuing."
                );
                append_to_tool_result(&mut output.value, &wrap(&body));
                // Phase 4C Strict mode: flip ok so the runtime's reflection /
                // recovery path forces the next THINK step to address the
                // failure instead of just hoping the model notices the
                // reminder. TimedOut and Skipped are NOT escalated — only
                // confirmed Failed outcomes flip ok.
                if self.policy.flips_ok_on_failure() {
                    output.ok = false;
                    if let serde_json::Value::Object(map) = &mut output.value {
                        map.insert(
                            "error".to_string(),
                            serde_json::Value::String(format!(
                                "post-edit verification of {display} failed; see _system_reminder for detail"
                            )),
                        );
                    }
                }
            }
            VerificationOutcome::TimedOut => {
                let body = format!(
                    "Verification of {display} timed out (>{}s) — verify manually if needed.",
                    self.dispatcher.timeout().as_secs()
                );
                append_to_tool_result(&mut output.value, &wrap(&body));
            }
            VerificationOutcome::Skipped { reason } => {
                // Only surface the FIRST per-toolchain "missing" hint. Others
                // (no Cargo.toml / unknown extension) stay silent so we don't
                // spam every tool result.
                let toolchain = toolchain_from_skip_reason(&reason);
                if let Some(name) = toolchain {
                    if self.dispatcher.announce_missing_toolchain(name) {
                        append_to_tool_result(&mut output.value, &wrap(&reason));
                    }
                }
            }
        }
    }
}

/// Heuristic: extract the toolchain name from a "<X> toolchain not found"
/// reason produced by the verifiers. Returns `None` for skip reasons that
/// don't represent a missing toolchain (e.g. "no Cargo.toml found").
fn toolchain_from_skip_reason(reason: &str) -> Option<&'static str> {
    if reason.starts_with("cargo toolchain not found") {
        Some("cargo")
    } else if reason.starts_with("tsc toolchain not found") {
        Some("tsc")
    } else if reason.starts_with("python toolchain not found") {
        Some("python")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verification_dispatcher::Verifier;
    use serde_json::json;
    use std::sync::Mutex as StdMutex;

    /// Test-only verifier that returns canned outcomes.
    #[derive(Debug)]
    struct CannedVerifier {
        outcomes: StdMutex<Vec<VerificationOutcome>>,
        ext: &'static str,
    }

    impl CannedVerifier {
        fn new(ext: &'static str, outcomes: Vec<VerificationOutcome>) -> Self {
            Self {
                outcomes: StdMutex::new(outcomes),
                ext,
            }
        }
    }

    #[async_trait]
    impl Verifier for CannedVerifier {
        fn name(&self) -> &str {
            "canned"
        }
        fn handles(&self, path: &Path) -> bool {
            path.extension().and_then(|s| s.to_str()) == Some(self.ext)
        }
        async fn verify(&self, _path: &Path) -> VerificationOutcome {
            let mut q = self.outcomes.lock().unwrap();
            if q.is_empty() {
                VerificationOutcome::Skipped {
                    reason: "queue empty".into(),
                }
            } else {
                q.remove(0)
            }
        }
    }

    fn ok_write(path: &Path) -> ToolOutput {
        ToolOutput {
            ok: true,
            value: json!({
                "path": path.to_string_lossy(),
                "bytes": 4,
            }),
            truncated: false,
        }
    }

    #[tokio::test]
    async fn skips_non_verified_tools() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, r#"{"k":1}"#).unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new("json", vec![VerificationOutcome::Passed]),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));
        let mut out = ok_write(&p);
        // read_file is not in VERIFIED_TOOLS — verifier must NOT run.
        dec.decorate("read_file", &mut out).await;
        assert!(out.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn skips_failed_writes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, r#"{"k":1}"#).unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new("json", vec![VerificationOutcome::Passed]),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));
        // Failed write: ok = false → no verification.
        let mut out = ToolOutput {
            ok: false,
            value: json!({"path": p.to_string_lossy(), "error": "denied"}),
            truncated: false,
        };
        dec.decorate("write_file", &mut out).await;
        assert!(out.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn passed_outcome_appends_verified_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, r#"{"k":1}"#).unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new("json", vec![VerificationOutcome::Passed]),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));
        let mut out = ok_write(&p);
        dec.decorate("write_file", &mut out).await;
        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.contains("verified:"));
        assert!(reminder.contains("a.json"));
        assert!(out.ok);
    }

    #[tokio::test]
    async fn failed_outcome_includes_detail_and_fix_hint() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{ bad }").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![VerificationOutcome::Failed {
                    detail: "JSON parse error: expected key at line 1".into(),
                    truncated: false,
                }],
            ),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));
        let mut out = ok_write(&p);
        dec.decorate("edit_file", &mut out).await;
        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.contains("Verification of"));
        assert!(reminder.contains("failed"));
        assert!(reminder.contains("JSON parse error"));
        assert!(reminder.contains("Fix this before continuing"));
        // ok stays true (Phase 4B never flips ok; that's Strict / Phase 4C).
        assert!(out.ok);
    }

    #[tokio::test]
    async fn timed_out_outcome_appends_timeout_reminder() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new("json", vec![VerificationOutcome::TimedOut]),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));
        let mut out = ok_write(&p);
        dec.decorate("multi_edit", &mut out).await;
        let reminder = out.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.contains("timed out"));
    }

    #[tokio::test]
    async fn missing_toolchain_skip_announces_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![
                    VerificationOutcome::Skipped {
                        reason: "cargo toolchain not found (cargo: not found); install it".into(),
                    },
                    VerificationOutcome::Skipped {
                        reason: "cargo toolchain not found (cargo: not found); install it".into(),
                    },
                ],
            ),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));

        let mut first = ok_write(&p);
        dec.decorate("write_file", &mut first).await;
        // First call surfaces the hint exactly once.
        let r = first.value["_system_reminder"].as_str().unwrap();
        assert!(r.contains("cargo toolchain not found"));

        let mut second = ok_write(&p);
        dec.decorate("write_file", &mut second).await;
        // Second call: dispatcher's announced-set already contains "cargo",
        // so the reminder is suppressed.
        assert!(second.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn benign_skip_reasons_stay_silent() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![VerificationOutcome::Skipped {
                    reason: "no Cargo.toml found ancestrally from /tmp/x.rs".into(),
                }],
            ),
        )]));
        let dec = VerificationDecorator::new(dispatcher, Some(dir.path().to_path_buf()));
        let mut out = ok_write(&p);
        dec.decorate("write_file", &mut out).await;
        // No toolchain mentioned → stays silent.
        assert!(out.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn no_path_field_means_no_verification() {
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![]));
        let dec = VerificationDecorator::new(dispatcher, None);
        let mut out = ToolOutput {
            ok: true,
            value: json!({"replacements": 1}), // missing "path"
            truncated: false,
        };
        dec.decorate("edit_file", &mut out).await;
        assert!(out.value.get("_system_reminder").is_none());
    }

    // ----- Phase 4C: VerificationPolicy modes -----

    #[tokio::test]
    async fn disabled_policy_short_circuits_decorator() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{ bad }").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![VerificationOutcome::Failed {
                    detail: "broken".into(),
                    truncated: false,
                }],
            ),
        )]));
        let dec = VerificationDecorator::with_policy(
            dispatcher,
            Some(dir.path().to_path_buf()),
            VerificationPolicy::Disabled,
        );
        let mut out = ok_write(&p);
        dec.decorate("write_file", &mut out).await;
        // Disabled → no reminder, no flip, no verifier call.
        assert!(out.value.get("_system_reminder").is_none());
        assert!(out.ok);
    }

    #[tokio::test]
    async fn enabled_policy_keeps_ok_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{ bad }").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![VerificationOutcome::Failed {
                    detail: "JSON parse error".into(),
                    truncated: false,
                }],
            ),
        )]));
        let dec = VerificationDecorator::with_policy(
            dispatcher,
            Some(dir.path().to_path_buf()),
            VerificationPolicy::Enabled,
        );
        let mut out = ok_write(&p);
        dec.decorate("write_file", &mut out).await;
        // Reminder present.
        assert!(out.value["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("failed"));
        // Default Enabled keeps ok=true (Phase 4B semantic).
        assert!(out.ok);
    }

    #[tokio::test]
    async fn strict_policy_flips_ok_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{ bad }").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![VerificationOutcome::Failed {
                    detail: "JSON parse error".into(),
                    truncated: false,
                }],
            ),
        )]));
        let dec = VerificationDecorator::with_policy(
            dispatcher,
            Some(dir.path().to_path_buf()),
            VerificationPolicy::Strict,
        );
        let mut out = ok_write(&p);
        dec.decorate("edit_file", &mut out).await;
        // Reminder present.
        assert!(out.value["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("failed"));
        // Strict flips ok.
        assert!(!out.ok);
        // Original "error" field is set to a stable summary so the
        // reflection engine can see it without parsing the reminder.
        let err = out.value["error"].as_str().unwrap();
        assert!(err.contains("post-edit verification"));
    }

    #[tokio::test]
    async fn strict_policy_does_not_flip_ok_on_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new("json", vec![VerificationOutcome::TimedOut]),
        )]));
        let dec = VerificationDecorator::with_policy(
            dispatcher,
            Some(dir.path().to_path_buf()),
            VerificationPolicy::Strict,
        );
        let mut out = ok_write(&p);
        dec.decorate("write_file", &mut out).await;
        assert!(out.value["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("timed out"));
        // TimedOut is not a confirmed failure → ok stays true even in Strict.
        assert!(out.ok);
    }

    #[tokio::test]
    async fn strict_policy_does_not_flip_ok_on_skipped_toolchain() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new(
                "json",
                vec![VerificationOutcome::Skipped {
                    reason: "cargo toolchain not found (cargo: missing)".into(),
                }],
            ),
        )]));
        let dec = VerificationDecorator::with_policy(
            dispatcher,
            Some(dir.path().to_path_buf()),
            VerificationPolicy::Strict,
        );
        let mut out = ok_write(&p);
        dec.decorate("multi_edit", &mut out).await;
        // Skipped is not a failure — ok stays true even in Strict.
        assert!(out.ok);
    }

    #[tokio::test]
    async fn strict_policy_keeps_ok_when_passed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{}").unwrap();
        let dispatcher = Arc::new(VerificationDispatcher::new(vec![Arc::new(
            CannedVerifier::new("json", vec![VerificationOutcome::Passed]),
        )]));
        let dec = VerificationDecorator::with_policy(
            dispatcher,
            Some(dir.path().to_path_buf()),
            VerificationPolicy::Strict,
        );
        let mut out = ok_write(&p);
        dec.decorate("write_file", &mut out).await;
        assert!(out.ok);
        assert!(out.value["_system_reminder"]
            .as_str()
            .unwrap()
            .contains("verified:"));
    }
}
