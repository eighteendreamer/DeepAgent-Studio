//! Post-edit verification dispatcher (Phase 4A of coding-amplifier).
//!
//! After the model writes / edits a file, the runtime can route the file
//! through a language-specific syntax / parse check. Outcomes are exposed via
//! [`VerificationOutcome`] so the runtime decorator (Phase 4B) can decide how
//! to surface them — typically as a `<system-reminder>` block.
//!
//! ## Verifiers shipped here
//!
//! - [`RustVerifier`] — runs `cargo check --offline --manifest-path …` against
//!   the nearest ancestor `Cargo.toml`.
//! - [`TypeScriptVerifier`] — runs `tsc --noEmit -p …` against the nearest
//!   ancestor `tsconfig.json`.
//! - [`PythonVerifier`] — runs `python -m py_compile <file>`, then optionally
//!   `ruff check <file>` when `ruff` is on PATH.
//! - [`JsonVerifier`] — strict JSON / JSONC syntax, or the actual TypeScript consumer
//!   via the same command executor for identified TypeScript configurations.
//!
//! The TOML and YAML verifiers from the spec are deferred: the workspace has
//! no `toml` / `serde_yaml` dependency yet and the project rule says "no new
//! deps". They will return [`VerificationOutcome::Skipped`] via the
//! "no verifier handles this path" branch.
//!
//! ## Timeouts and truncation
//!
//! - [`VerificationDispatcher::verify_file`] races each verifier against
//!   [`VERIFICATION_DEFAULT_TIMEOUT`] (5 s by default; override with
//!   [`VerificationDispatcher::with_timeout`]).
//! - Detail buffers larger than [`MAX_DETAIL_BYTES`] are truncated to the
//!   first error block plus a "(more errors omitted; run \`<tool>\` manually)"
//!   suffix; the [`VerificationOutcome::Failed::truncated`] flag flips so
//!   downstream UI can mark the message accordingly.
//!
//! ## Per-session deduplicated "missing toolchain" hints
//!
//! When `cargo` / `tsc` / `python` is not on PATH the verifier returns
//! `Skipped { reason: "<tool> toolchain not found …" }`. The dispatcher tracks
//! which toolchain names it has already announced via
//! [`VerificationDispatcher::announce_missing_toolchain`] so that the Phase
//! 4B decorator emits at most one such reminder per session per toolchain.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use crate::verification_commands::quote_argument;
use deepagent_builtins::{CommandExecutor, CommandOutcome, SystemExecutor};

mod json;
pub use json::JsonVerifier;

#[cfg(test)]
#[path = "verification_dispatcher/json_tests.rs"]
mod json_dialect_tests;

/// Default per-file verification budget. Verifiers that exceed this duration
/// yield [`VerificationOutcome::TimedOut`].
pub const VERIFICATION_DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum bytes of `Failed { detail }` shown to the model. Output longer
/// than this is truncated and `truncated: true` is set.
pub const MAX_DETAIL_BYTES: usize = 4096;

/// Result of a single-file verification pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationOutcome {
    /// No verifier handled the file (unsupported language, no toolchain
    /// installed, deferred-by-design extension like `.toml` / `.yaml`).
    Skipped {
        /// Human-readable reason. Meant for the system-reminder body.
        reason: String,
    },
    /// Verifier ran and reported no problems.
    Passed,
    /// Verifier ran and reported problems. `detail` is the raw error block
    /// (already truncated to [`MAX_DETAIL_BYTES`]); `truncated` says whether
    /// truncation occurred.
    Failed {
        /// Error detail, ready to embed inside a `<system-reminder>` block.
        detail: String,
        /// Whether the original output exceeded [`MAX_DETAIL_BYTES`].
        truncated: bool,
    },
    /// Verifier exceeded the dispatcher's timeout.
    TimedOut,
}

/// A single language-level file verifier.
#[async_trait]
pub trait Verifier: Send + Sync + std::fmt::Debug {
    /// Short identifier used in logs / reminder text (`"rust"`, `"json"`, …).
    fn name(&self) -> &str;
    /// Whether this verifier wants to handle `path` (typically extension-based).
    fn handles(&self, path: &Path) -> bool;
    /// Run the verification. Implementations should NOT enforce their own
    /// timeout — the dispatcher races them against
    /// [`VerificationDispatcher::timeout`].
    async fn verify(&self, path: &Path) -> VerificationOutcome;

    /// Supply the existing tool workspace for local configuration-role discovery.
    /// Other verifiers keep their existing execution boundary unchanged.
    async fn verify_in_workspace(
        &self,
        path: &Path,
        _workspace_root: Option<&Path>,
    ) -> VerificationOutcome {
        self.verify(path).await
    }
}

/// Top-level verification entry point. Holds the registered verifiers and the
/// per-toolchain "already announced as missing" set for Phase 4B's
/// reminder-deduplication contract.
#[derive(Debug, Clone)]
pub struct VerificationDispatcher {
    verifiers: Vec<Arc<dyn Verifier>>,
    timeout: Duration,
    announced_missing: Arc<Mutex<HashSet<String>>>,
}

impl VerificationDispatcher {
    /// Build a dispatcher with the given verifier registry. Verifiers are
    /// tried in order; the first whose `handles()` returns `true` wins.
    pub fn new(verifiers: Vec<Arc<dyn Verifier>>) -> Self {
        Self {
            verifiers,
            timeout: VERIFICATION_DEFAULT_TIMEOUT,
            announced_missing: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// The standard 4-verifier setup: Rust, TypeScript, Python, JSON.
    /// Pulls the system-default subprocess executor.
    pub fn standard() -> Self {
        let exec: Arc<dyn CommandExecutor> = Arc::new(SystemExecutor);
        Self::standard_with_executor(exec)
    }

    /// Build the standard set with the caller's execution boundary.
    pub fn standard_with_executor(executor: Arc<dyn CommandExecutor>) -> Self {
        Self::new(vec![
            Arc::new(RustVerifier::new(executor.clone())),
            Arc::new(TypeScriptVerifier::new(executor.clone())),
            Arc::new(PythonVerifier::new(executor.clone())),
            Arc::new(JsonVerifier::new(executor)),
        ])
    }

    /// Override the per-file timeout (default
    /// [`VERIFICATION_DEFAULT_TIMEOUT`]).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Configured timeout (read-only).
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Number of verifiers registered.
    pub fn len(&self) -> usize {
        self.verifiers.len()
    }

    /// Whether no verifiers are registered.
    pub fn is_empty(&self) -> bool {
        self.verifiers.is_empty()
    }

    /// Return whether `toolchain` (e.g. `"cargo"`, `"tsc"`, `"python"`) has
    /// already been announced as missing in this session. Returns `true` the
    /// first time; subsequent calls with the same name return `false`. The
    /// runtime decorator uses this to surface the "X is not installed" hint
    /// once.
    pub fn announce_missing_toolchain(&self, toolchain: &str) -> bool {
        let mut set = self
            .announced_missing
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        set.insert(toolchain.to_string())
    }

    /// Verify a single file against the first matching verifier. Returns
    /// [`VerificationOutcome::Skipped`] when no verifier handles the file.
    pub async fn verify_file(&self, path: &Path) -> VerificationOutcome {
        self.verify_file_in_workspace(path, None).await
    }

    /// Verify with the tool's workspace boundary, without a second registry or timeout.
    pub async fn verify_file_in_workspace(
        &self,
        path: &Path,
        workspace_root: Option<&Path>,
    ) -> VerificationOutcome {
        let Some(verifier) = self.verifiers.iter().find(|v| v.handles(path)) else {
            return VerificationOutcome::Skipped {
                reason: format!(
                    "no verifier registered for {}",
                    path.extension()
                        .and_then(|s| s.to_str())
                        .map(|s| format!(".{s}"))
                        .unwrap_or_else(|| "this file".to_string())
                ),
            };
        };
        let fut = verifier.verify_in_workspace(path, workspace_root);
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(outcome) => match outcome {
                VerificationOutcome::Failed { detail, .. } => {
                    let (clipped, truncated) = truncate_detail(&detail, verifier.name());
                    VerificationOutcome::Failed {
                        detail: clipped,
                        truncated,
                    }
                }
                other => other,
            },
            Err(_) => VerificationOutcome::TimedOut,
        }
    }
}

/// Walk up from `start` looking for a sibling file named `name`, returning
/// the first match. Returns `None` if `start` has no ancestor that contains
/// `name`.
fn find_nearest_ancestor(start: &Path, name: &str) -> Option<PathBuf> {
    let mut cur = if start.is_file() {
        start.parent().map(Path::to_path_buf)
    } else {
        Some(start.to_path_buf())
    };
    while let Some(dir) = cur {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// Truncate `detail` to [`MAX_DETAIL_BYTES`], appending a "more errors
/// omitted" suffix that points the user at running the verifier directly.
/// Returns `(clipped_detail, was_truncated)`.
fn truncate_detail(detail: &str, verifier_name: &str) -> (String, bool) {
    if detail.len() <= MAX_DETAIL_BYTES {
        return (detail.to_string(), false);
    }
    // Char-aware truncation: don't slice through a multi-byte UTF-8 boundary.
    let mut end = MAX_DETAIL_BYTES;
    while end > 0 && !detail.is_char_boundary(end) {
        end -= 1;
    }
    let mut clipped = detail[..end].to_string();
    clipped.push_str(&format!(
        "\n…(more output omitted; run the {verifier_name} verifier manually for the full list)"
    ));
    (clipped, true)
}

/// Detect whether `executor` could not find `command` (e.g. cargo not on
/// PATH). Returns the toolchain hint string when so.
fn missing_toolchain_message(toolchain: &str, err: &dyn std::fmt::Display) -> String {
    format!(
        "{toolchain} toolchain not found ({err}); install it or add it to PATH to enable {toolchain} verification"
    )
}

// ---- Rust ----

/// Runs `cargo check --offline --manifest-path <Cargo.toml>` against the
/// nearest ancestor `Cargo.toml`.
#[derive(Clone)]
pub struct RustVerifier {
    executor: Arc<dyn CommandExecutor>,
}

impl std::fmt::Debug for RustVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RustVerifier").finish()
    }
}

impl RustVerifier {
    /// Build over the given subprocess executor.
    pub fn new(executor: Arc<dyn CommandExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl Verifier for RustVerifier {
    fn name(&self) -> &str {
        "rust"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("rs")
    }

    async fn verify(&self, path: &Path) -> VerificationOutcome {
        let Some(manifest) = find_nearest_ancestor(path, "Cargo.toml") else {
            return VerificationOutcome::Skipped {
                reason: format!("no Cargo.toml found ancestrally from {}", path.display()),
            };
        };
        let manifest_str = manifest.to_string_lossy().into_owned();
        let cwd = manifest
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        // Quote the path so spaces / Unicode are preserved through the shell.
        let cmd = format!(
            "cargo check --offline --manifest-path {}",
            quote_argument(&manifest_str)
        );
        match self.executor.run(&cmd, &cwd).await {
            Ok(outcome) => command_outcome_to_verification(outcome),
            Err(e) => VerificationOutcome::Skipped {
                reason: missing_toolchain_message("cargo", &e),
            },
        }
    }
}

// ---- TypeScript / JavaScript ----

/// Runs `tsc --noEmit -p <tsconfig_dir>`. JS / JSX files are accepted too;
/// TypeScript projects with `allowJs` will type-check them, otherwise tsc
/// returns clean and we treat that as Passed.
#[derive(Clone)]
pub struct TypeScriptVerifier {
    executor: Arc<dyn CommandExecutor>,
}

impl std::fmt::Debug for TypeScriptVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypeScriptVerifier").finish()
    }
}

impl TypeScriptVerifier {
    /// Build over the given subprocess executor.
    pub fn new(executor: Arc<dyn CommandExecutor>) -> Self {
        Self { executor }
    }

    /// Check an explicitly identified configuration with its actual consumer.
    /// --noEmit checks the actual consumer. Redirect incremental state to a
    /// disposable host-owned directory, never the user's project. --showConfig
    /// is not a validation verdict (TS 5.9 can exit 0 for malformed JSON).
    /// The availability marker is emitted only by our host-generated shell,
    /// not inferred from localized compiler stderr or arbitrary exit codes.
    async fn verify_config(&self, path: &Path, root: Option<&Path>) -> VerificationOutcome {
        const MISSING: &str = "__DEEPAGENT_TSC_UNAVAILABLE_V1__";
        let Some(dir) = path.parent() else {
            return VerificationOutcome::Skipped {
                reason: "tsc toolchain not found: configuration has no parent; not verified".into(),
            };
        };
        let scratch = match tempfile::Builder::new()
            .prefix("deepagent-tsconfig-check-")
            .tempdir()
        {
            Ok(directory) => directory,
            Err(error) => {
                return VerificationOutcome::Failed {
                    detail: format!("prepare TypeScript verification scratch directory: {error}"),
                    truncated: false,
                }
            }
        };
        let mut local = None;
        for ancestor in dir
            .ancestors()
            .take_while(|dir| root.is_none_or(|root| dir.starts_with(root)))
        {
            let candidate = ancestor.join(if cfg!(windows) {
                "node_modules/.bin/tsc.cmd"
            } else {
                "node_modules/.bin/tsc"
            });
            if candidate.is_file() {
                local = Some(candidate);
                break;
            }
            if root.is_none() {
                break;
            }
        }
        let args = format!(
            "--noEmit --pretty false --tsBuildInfoFile {} --project {}",
            quote_argument(
                &scratch
                    .path()
                    .join("verification.tsbuildinfo")
                    .to_string_lossy()
            ),
            quote_argument(&path.to_string_lossy())
        );
        let command = match (cfg!(windows), local) {
            (true, Some(local)) => format!("$ErrorActionPreference = 'Stop'; & {} {args}; exit $LASTEXITCODE", quote_argument(&local.to_string_lossy())),
            (false, Some(local)) => format!("{} {args}", quote_argument(&local.to_string_lossy())),
            (true, None) => format!("$ErrorActionPreference = 'Stop'; $compiler = Get-Command -Name tsc -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1; if ($null -eq $compiler) {{ Write-Output '{MISSING}'; exit 0 }}; & $compiler.Source {args}; exit $LASTEXITCODE"),
            (false, None) => format!("if command -v tsc >/dev/null 2>&1; then tsc {args}; else printf '%s\\n' '{MISSING}'; fi"),
        };
        match self.executor.run(&command, &dir.to_string_lossy()).await {
            Ok(outcome) if outcome.exit_code == Some(0) && outcome.stdout.trim() == MISSING && outcome.stderr.trim().is_empty() => VerificationOutcome::Skipped {
                reason: "tsc toolchain not found; TypeScript configuration was not verified (未完成验证); install TypeScript to enable this check".into(),
            },
            Ok(outcome) if outcome.exit_code == Some(0) => VerificationOutcome::Passed,
            Ok(outcome) => {
                // tsc writes diagnostics to stdout. A shell warning on stderr
                // must not hide the actual compiler error.
                let detail = [outcome.stdout.trim(), outcome.stderr.trim()]
                    .into_iter().filter(|text| !text.is_empty()).collect::<Vec<_>>().join("\n");
                VerificationOutcome::Failed {
                    detail: format!("TypeScript verification failed:\n{}", if detail.is_empty() { pick_error_block(&outcome) } else { detail }),
                    truncated: false,
                }
            },
            Err(error) => VerificationOutcome::Failed { detail: format!("TypeScript verification execution failed: {error}"), truncated: false },
        }
    }
}

#[async_trait]
impl Verifier for TypeScriptVerifier {
    fn name(&self) -> &str {
        "tsc"
    }

    fn handles(&self, path: &Path) -> bool {
        matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts")
        )
    }

    async fn verify(&self, path: &Path) -> VerificationOutcome {
        let Some(tsconfig) = find_nearest_ancestor(path, "tsconfig.json") else {
            return VerificationOutcome::Skipped {
                reason: format!(
                    "no tsconfig.json found ancestrally from {} (skipping tsc check)",
                    path.display()
                ),
            };
        };
        let dir = tsconfig
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        let cmd = format!("tsc --noEmit -p {}", quote_argument(&dir));
        match self.executor.run(&cmd, &dir).await {
            Ok(outcome) => command_outcome_to_verification(outcome),
            Err(e) => VerificationOutcome::Skipped {
                reason: missing_toolchain_message("tsc", &e),
            },
        }
    }
}

// ---- Python ----

/// Runs `python -m py_compile <file>`, then `ruff check <file>` when ruff
/// is available.
#[derive(Clone)]
pub struct PythonVerifier {
    executor: Arc<dyn CommandExecutor>,
}

impl std::fmt::Debug for PythonVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PythonVerifier").finish()
    }
}

impl PythonVerifier {
    /// Build over the given subprocess executor.
    pub fn new(executor: Arc<dyn CommandExecutor>) -> Self {
        Self { executor }
    }

    fn ruff_is_missing(outcome: &CommandOutcome) -> bool {
        Self::ruff_is_missing_for_shell(outcome, cfg!(windows))
    }

    fn ruff_is_missing_for_shell(outcome: &CommandOutcome, windows: bool) -> bool {
        let expected_exit_code = if windows { 1 } else { 127 };
        if outcome.exit_code != Some(expected_exit_code) {
            return false;
        }
        let stderr = outcome.stderr.trim_start();
        if windows {
            stderr.starts_with("#< CLIXML") && stderr.contains("<S S=\"Error\">ruff:")
        } else {
            stderr.lines().any(|line| {
                let line = line.trim();
                line == "ruff: command not found"
                    || (line.starts_with("bash: line ")
                        && line.contains(": ruff: command not found"))
                    || (line.starts_with("sh: ") && line.contains(": ruff: not found"))
                    || line == "zsh: command not found: ruff"
            })
        }
    }
}

#[async_trait]
impl Verifier for PythonVerifier {
    fn name(&self) -> &str {
        "python"
    }

    fn handles(&self, path: &Path) -> bool {
        path.extension().and_then(|s| s.to_str()) == Some("py")
    }

    async fn verify(&self, path: &Path) -> VerificationOutcome {
        let path_str = path.to_string_lossy().into_owned();
        let cwd = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| ".".into());
        let compile_cmd = format!("python -m py_compile {}", quote_argument(&path_str));
        let compile = match self.executor.run(&compile_cmd, &cwd).await {
            Ok(o) => o,
            Err(e) => {
                return VerificationOutcome::Skipped {
                    reason: missing_toolchain_message("python", &e),
                };
            }
        };
        if compile.exit_code != Some(0) {
            return VerificationOutcome::Failed {
                detail: pick_error_block(&compile),
                truncated: false,
            };
        }
        // Keep the existing single command inside the normal execution gate;
        // only its platform-native command-not-found signature is optional.
        let ruff_cmd = format!("ruff check {}", quote_argument(&path_str));
        match self.executor.run(&ruff_cmd, &cwd).await {
            Ok(o) if Self::ruff_is_missing(&o) => {
                VerificationOutcome::Skipped {
                    reason: "ruff optional lint unavailable; Python syntax passed, optional lint was not run".into(),
                }
            }
            Ok(o) if o.exit_code == Some(0) => VerificationOutcome::Passed,
            Ok(o) => VerificationOutcome::Failed {
                detail: pick_error_block(&o),
                truncated: false,
            },
            Err(error) => VerificationOutcome::Failed {
                detail: format!("ruff execution failed after Python syntax passed: {error}"),
                truncated: false,
            },
        }
    }
}

/// Convert a subprocess outcome into a verification outcome.
fn command_outcome_to_verification(outcome: CommandOutcome) -> VerificationOutcome {
    if outcome.exit_code == Some(0) {
        VerificationOutcome::Passed
    } else {
        VerificationOutcome::Failed {
            detail: pick_error_block(&outcome),
            truncated: false,
        }
    }
}

/// Heuristic for picking which channel actually contains the human-readable
/// error. Prefers stderr when it has content (cargo / tsc / py_compile all
/// write errors there); falls back to stdout; finally produces a synthetic
/// message when both are empty.
fn pick_error_block(outcome: &CommandOutcome) -> String {
    let stderr = outcome.stderr.trim();
    let stdout = outcome.stdout.trim();
    if !stderr.is_empty() {
        stderr.to_string()
    } else if !stdout.is_empty() {
        stdout.to_string()
    } else {
        format!(
            "verifier exited with code {}",
            outcome
                .exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".into())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::error::CoreError;
    use std::sync::Mutex as StdMutex;

    /// Test executor that returns canned outcomes (or an error simulating
    /// "command not found") keyed by the prefix of the command line.
    #[derive(Default)]
    pub(super) struct MockExecutor {
        responses: StdMutex<Vec<MockEntry>>,
    }

    struct MockEntry {
        prefix: String,
        result: Result<CommandOutcome, String>,
    }

    impl MockExecutor {
        pub(super) fn with(self, prefix: &str, result: Result<CommandOutcome, String>) -> Self {
            self.responses.lock().unwrap().push(MockEntry {
                prefix: prefix.to_string(),
                result,
            });
            self
        }
    }

    #[async_trait]
    impl CommandExecutor for MockExecutor {
        async fn run(
            &self,
            command: &str,
            _cwd: &str,
        ) -> deepagent_core::error::Result<CommandOutcome> {
            let resp = self.responses.lock().unwrap();
            for e in resp.iter() {
                if command.starts_with(&e.prefix) {
                    return e
                        .result
                        .clone()
                        .map_err(|m| CoreError::other(format!("not found: {m}")));
                }
            }
            Err(CoreError::other(format!("no mock for: {command}")))
        }
    }

    pub(super) fn ok_exit(stdout: &str, stderr: &str) -> CommandOutcome {
        CommandOutcome {
            exit_code: Some(0),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    pub(super) fn fail_exit(code: i32, stdout: &str, stderr: &str) -> CommandOutcome {
        CommandOutcome {
            exit_code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn missing_ruff_exit() -> CommandOutcome {
        let stderr = if cfg!(windows) {
            "#< CLIXML\r\n<Objs Version=\"1.1.0.1\"><S S=\"Error\">ruff: command is unavailable in this locale</S></Objs>"
        } else {
            "sh: 1: ruff: not found"
        };
        fail_exit(if cfg!(windows) { 1 } else { 127 }, "", stderr)
    }

    // ----- find_nearest_ancestor -----

    #[test]
    fn find_nearest_ancestor_walks_upward() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        let leaf = nested.join("file.rs");
        std::fs::write(&leaf, "fn main() {}").unwrap();
        let found = find_nearest_ancestor(&leaf, "Cargo.toml").unwrap();
        assert_eq!(found, dir.path().join("Cargo.toml"));
    }

    #[test]
    fn find_nearest_ancestor_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("nope.rs");
        std::fs::write(&leaf, "").unwrap();
        assert!(find_nearest_ancestor(&leaf, "Cargo.toml").is_none());
    }

    // ----- truncate_detail -----

    #[test]
    fn truncate_detail_keeps_short_outputs_intact() {
        let (out, was) = truncate_detail("error[E0001]: oops", "rust");
        assert!(!was);
        assert_eq!(out, "error[E0001]: oops");
    }

    #[test]
    fn truncate_detail_clips_long_output_with_hint() {
        let big = "x".repeat(MAX_DETAIL_BYTES + 100);
        let (out, was) = truncate_detail(&big, "rust");
        assert!(was);
        assert!(out.len() < big.len() + 200);
        assert!(out.contains("more output omitted"));
        assert!(out.contains("rust"));
    }

    // ----- JsonVerifier -----

    #[tokio::test]
    async fn json_verifier_passes_valid() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, r#"{"k":1,"v":[true,null]}"#).unwrap();
        let outcome = JsonVerifier::default().verify(&p).await;
        assert_eq!(outcome, VerificationOutcome::Passed);
    }

    #[tokio::test]
    async fn json_verifier_fails_on_syntax_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, "{ bad }").unwrap();
        let outcome = JsonVerifier::default().verify(&p).await;
        match outcome {
            VerificationOutcome::Failed { detail, .. } => {
                assert!(detail.contains("JSON parse error"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn json_verifier_handles_only_json_extension() {
        let v = JsonVerifier::default();
        assert!(v.handles(Path::new("foo.json")));
        assert!(!v.handles(Path::new("foo.rs")));
        assert!(!v.handles(Path::new("foo")));
    }

    // ----- RustVerifier (with mocks) -----

    #[tokio::test]
    async fn rust_verifier_passes_when_cargo_check_exits_zero() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let leaf = dir.path().join("src").join("lib.rs");
        std::fs::create_dir_all(leaf.parent().unwrap()).unwrap();
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(MockExecutor::default().with("cargo check", Ok(ok_exit("", ""))));
        let v = RustVerifier::new(mock);
        let outcome = v.verify(&leaf).await;
        assert_eq!(outcome, VerificationOutcome::Passed);
    }

    #[tokio::test]
    async fn rust_verifier_fails_with_stderr_detail() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"").unwrap();
        let leaf = dir.path().join("src").join("lib.rs");
        std::fs::create_dir_all(leaf.parent().unwrap()).unwrap();
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(
            MockExecutor::default().with("cargo check", Ok(fail_exit(1, "", "error[E0001]: oops"))),
        );
        let v = RustVerifier::new(mock);
        let outcome = v.verify(&leaf).await;
        match outcome {
            VerificationOutcome::Failed { detail, .. } => {
                assert!(detail.contains("error[E0001]"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rust_verifier_skipped_when_cargo_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        let leaf = dir.path().join("a.rs");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(
            MockExecutor::default().with("cargo check", Err("cargo: command not found".into())),
        );
        let v = RustVerifier::new(mock);
        let outcome = v.verify(&leaf).await;
        match outcome {
            VerificationOutcome::Skipped { reason } => {
                assert!(reason.contains("cargo toolchain not found"));
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rust_verifier_skipped_when_no_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.rs");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(MockExecutor::default());
        let v = RustVerifier::new(mock);
        let outcome = v.verify(&leaf).await;
        match outcome {
            VerificationOutcome::Skipped { reason } => {
                assert!(reason.contains("no Cargo.toml"));
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    // ----- TypeScriptVerifier (with mocks) -----

    #[tokio::test]
    async fn ts_verifier_handles_relevant_extensions() {
        let v = TypeScriptVerifier::new(Arc::new(MockExecutor::default()));
        assert!(v.handles(Path::new("a.ts")));
        assert!(v.handles(Path::new("a.tsx")));
        assert!(v.handles(Path::new("a.js")));
        assert!(v.handles(Path::new("a.jsx")));
        assert!(v.handles(Path::new("a.mts")));
        assert!(!v.handles(Path::new("a.rs")));
    }

    #[tokio::test]
    async fn ts_verifier_passes_with_clean_tsc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        let leaf = dir.path().join("a.ts");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(MockExecutor::default().with("tsc --noEmit", Ok(ok_exit("", ""))));
        let outcome = TypeScriptVerifier::new(mock).verify(&leaf).await;
        assert_eq!(outcome, VerificationOutcome::Passed);
    }

    #[tokio::test]
    async fn ts_verifier_fails_on_tsc_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), "{}").unwrap();
        let leaf = dir.path().join("a.ts");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(
            MockExecutor::default().with("tsc --noEmit", Ok(fail_exit(2, "TS2304: oops", ""))),
        );
        let outcome = TypeScriptVerifier::new(mock).verify(&leaf).await;
        match outcome {
            VerificationOutcome::Failed { detail, .. } => assert!(detail.contains("TS2304")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ts_verifier_skipped_without_tsconfig() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.ts");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(MockExecutor::default());
        let outcome = TypeScriptVerifier::new(mock).verify(&leaf).await;
        match outcome {
            VerificationOutcome::Skipped { reason } => assert!(reason.contains("tsconfig.json")),
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    // ----- PythonVerifier (with mocks) -----

    #[tokio::test]
    async fn python_verifier_passes_when_compile_clean_and_no_ruff() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.py");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(
            MockExecutor::default()
                .with("python -m py_compile", Ok(ok_exit("", "")))
                .with("ruff check", Ok(missing_ruff_exit())),
        );
        let outcome = PythonVerifier::new(mock).verify(&leaf).await;
        assert!(
            matches!(outcome, VerificationOutcome::Skipped { reason } if reason.contains("lint was not run"))
        );
    }

    #[tokio::test]
    async fn python_verifier_classifies_only_its_missing_ruff_marker_as_optional() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.py");
        std::fs::write(&leaf, "").unwrap();

        let missing = Arc::new(
            MockExecutor::default()
                .with("python -m py_compile", Ok(ok_exit("", "")))
                .with("ruff check", Ok(missing_ruff_exit())),
        );
        assert!(matches!(
            PythonVerifier::new(missing).verify(&leaf).await,
            VerificationOutcome::Skipped { reason } if reason.contains("lint was not run")
        ));

        let infrastructure_failure = Arc::new(
            MockExecutor::default()
                .with("python -m py_compile", Ok(ok_exit("", "")))
                .with(
                    "ruff check",
                    Ok(fail_exit(1, "", "permission denied by shell policy")),
                ),
        );
        assert!(matches!(
            PythonVerifier::new(infrastructure_failure).verify(&leaf).await,
            VerificationOutcome::Failed { detail, .. } if detail.contains("permission denied")
        ));

        let unavailable_executor = Arc::new(
            MockExecutor::default()
                .with("python -m py_compile", Ok(ok_exit("", "")))
                .with("ruff check", Err("executor connection lost".into())),
        );
        assert!(matches!(
            PythonVerifier::new(unavailable_executor).verify(&leaf).await,
            VerificationOutcome::Failed { detail, .. } if detail.contains("executor connection lost")
        ));
    }

    #[test]
    fn python_verifier_missing_ruff_signature_is_shell_specific() {
        let windows_missing = fail_exit(
            1,
            "",
            "#< CLIXML\r\n<Objs Version=\"1.1.0.1\"><S S=\"Error\">ruff: command is unavailable in this locale</S></Objs>",
        );
        let posix_missing = fail_exit(127, "", "sh: 1: ruff: not found");
        assert!(PythonVerifier::ruff_is_missing_for_shell(
            &windows_missing,
            true
        ));
        assert!(PythonVerifier::ruff_is_missing_for_shell(
            &posix_missing,
            false
        ));
        assert!(!PythonVerifier::ruff_is_missing_for_shell(
            &windows_missing,
            false
        ));
        assert!(!PythonVerifier::ruff_is_missing_for_shell(
            &posix_missing,
            true
        ));
        assert!(!PythonVerifier::ruff_is_missing_for_shell(
            &fail_exit(1, "", "sh: 1: ruff: not found"),
            false
        ));
        assert!(!PythonVerifier::ruff_is_missing_for_shell(
            &fail_exit(127, "", "#< CLIXML\r\n<S S=\"Error\">ruff: not found</S>"),
            true
        ));
        assert!(!PythonVerifier::ruff_is_missing(&fail_exit(
            1,
            "",
            "ruff F401: imported but unused"
        )));
        assert!(!PythonVerifier::ruff_is_missing(&fail_exit(
            1,
            "",
            "permission denied by shell policy"
        )));
        assert!(!PythonVerifier::ruff_is_missing(&fail_exit(
            2,
            "",
            &windows_missing.stderr
        )));
    }

    #[tokio::test]
    async fn python_verifier_fails_on_compile_error() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.py");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(MockExecutor::default().with(
            "python -m py_compile",
            Ok(fail_exit(1, "", "SyntaxError: invalid syntax")),
        ));
        let outcome = PythonVerifier::new(mock).verify(&leaf).await;
        match outcome {
            VerificationOutcome::Failed { detail, .. } => {
                assert!(detail.contains("SyntaxError"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn python_verifier_runs_ruff_when_available_and_passes() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.py");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(
            MockExecutor::default()
                .with("python -m py_compile", Ok(ok_exit("", "")))
                .with("ruff check", Ok(ok_exit("", ""))),
        );
        let outcome = PythonVerifier::new(mock).verify(&leaf).await;
        assert_eq!(outcome, VerificationOutcome::Passed);
    }

    #[tokio::test]
    async fn python_verifier_fails_when_ruff_complains() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.py");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(
            MockExecutor::default()
                .with("python -m py_compile", Ok(ok_exit("", "")))
                .with(
                    "ruff check",
                    Ok(fail_exit(1, "F401 imported but unused", "")),
                ),
        );
        let outcome = PythonVerifier::new(mock).verify(&leaf).await;
        match outcome {
            VerificationOutcome::Failed { detail, .. } => assert!(detail.contains("F401")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn python_verifier_skipped_when_python_missing() {
        let dir = tempfile::tempdir().unwrap();
        let leaf = dir.path().join("a.py");
        std::fs::write(&leaf, "").unwrap();
        let mock = Arc::new(MockExecutor::default().with(
            "python -m py_compile",
            Err("python: command not found".into()),
        ));
        let outcome = PythonVerifier::new(mock).verify(&leaf).await;
        match outcome {
            VerificationOutcome::Skipped { reason } => {
                assert!(reason.contains("python toolchain not found"));
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    // ----- VerificationDispatcher routing + timeout + dedup -----

    #[tokio::test]
    async fn dispatcher_routes_by_extension() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, r#"{"x":1}"#).unwrap();
        let dispatcher =
            VerificationDispatcher::standard_with_executor(Arc::new(MockExecutor::default()));
        let outcome = dispatcher.verify_file(&p).await;
        assert_eq!(outcome, VerificationOutcome::Passed);
    }

    #[tokio::test]
    async fn dispatcher_skips_unknown_extension() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.weird");
        std::fs::write(&p, "x").unwrap();
        let dispatcher =
            VerificationDispatcher::standard_with_executor(Arc::new(MockExecutor::default()));
        let outcome = dispatcher.verify_file(&p).await;
        match outcome {
            VerificationOutcome::Skipped { reason } => assert!(reason.contains(".weird")),
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    /// A verifier that sleeps longer than the dispatcher timeout — used to
    /// drive the TimedOut branch deterministically.
    #[derive(Debug)]
    struct SlowVerifier;
    #[async_trait]
    impl Verifier for SlowVerifier {
        fn name(&self) -> &str {
            "slow"
        }
        fn handles(&self, path: &Path) -> bool {
            path.extension().and_then(|s| s.to_str()) == Some("slow")
        }
        async fn verify(&self, _path: &Path) -> VerificationOutcome {
            tokio::time::sleep(Duration::from_millis(500)).await;
            VerificationOutcome::Passed
        }
    }

    #[tokio::test]
    async fn dispatcher_returns_timed_out_when_verifier_exceeds_budget() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.slow");
        std::fs::write(&p, "").unwrap();
        let dispatcher = VerificationDispatcher::new(vec![Arc::new(SlowVerifier)])
            .with_timeout(Duration::from_millis(50));
        let outcome = dispatcher.verify_file(&p).await;
        assert_eq!(outcome, VerificationOutcome::TimedOut);
    }

    #[test]
    fn dispatcher_tracks_announced_missing_toolchains() {
        let dispatcher = VerificationDispatcher::new(vec![]);
        // First call → fresh, returns true (caller is allowed to surface).
        assert!(dispatcher.announce_missing_toolchain("cargo"));
        // Second call with same toolchain → already announced, returns false.
        assert!(!dispatcher.announce_missing_toolchain("cargo"));
        // Different toolchain → fresh again.
        assert!(dispatcher.announce_missing_toolchain("tsc"));
        assert!(!dispatcher.announce_missing_toolchain("tsc"));
    }

    #[tokio::test]
    async fn dispatcher_truncates_oversized_failure_detail() {
        // A failing verifier that emits a 6 KB error blob → dispatcher must
        // clip and set truncated:true.
        #[derive(Debug)]
        struct GiantFailVerifier;
        #[async_trait]
        impl Verifier for GiantFailVerifier {
            fn name(&self) -> &str {
                "giant"
            }
            fn handles(&self, path: &Path) -> bool {
                path.extension().and_then(|s| s.to_str()) == Some("giant")
            }
            async fn verify(&self, _path: &Path) -> VerificationOutcome {
                VerificationOutcome::Failed {
                    detail: "x".repeat(MAX_DETAIL_BYTES + 2000),
                    truncated: false,
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.giant");
        std::fs::write(&p, "").unwrap();
        let dispatcher = VerificationDispatcher::new(vec![Arc::new(GiantFailVerifier)]);
        match dispatcher.verify_file(&p).await {
            VerificationOutcome::Failed { detail, truncated } => {
                assert!(truncated);
                assert!(detail.len() < MAX_DETAIL_BYTES + 500);
                assert!(detail.contains("more output omitted"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn dispatcher_default_timeout_is_5_seconds() {
        let dispatcher = VerificationDispatcher::standard();
        assert_eq!(dispatcher.timeout(), VERIFICATION_DEFAULT_TIMEOUT);
        assert!(!dispatcher.is_empty());
        assert_eq!(dispatcher.len(), 4);
    }
}
