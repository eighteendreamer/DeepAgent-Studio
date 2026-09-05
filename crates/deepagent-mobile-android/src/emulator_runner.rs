//! Emulator command execution.
//!
//! Wraps the `emulator` binary from the Android SDK to manage AVD lifecycle.
//! Uses the same argv-only, no-shell pattern as `AdbCommandRunner`.

use async_trait::async_trait;
use deepagent_mobile_core::{MobileError, MobileResult};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::adb_runner::AdbCommandOutput;

/// Trait for executing emulator commands.
///
/// Implementations wrap the `emulator` binary. Tests use `FakeEmulatorRunner`.
#[async_trait]
pub trait EmulatorCommandRunner: Send + Sync {
    /// Run an emulator command and return captured output.
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput>;
}

/// Fake emulator runner for testing.
///
/// Returns pre-configured output keyed by the first argument.
#[derive(Clone, Default)]
pub struct FakeEmulatorRunner {
    outputs: Arc<std::collections::HashMap<String, AdbCommandOutput>>,
}

impl FakeEmulatorRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the output for commands starting with the given key.
    pub fn set_output(&mut self, key: impl Into<String>, output: AdbCommandOutput) {
        Arc::make_mut(&mut self.outputs).insert(key.into(), output);
    }
}

#[async_trait]
impl EmulatorCommandRunner for FakeEmulatorRunner {
    async fn run(
        &self,
        _program: &str,
        args: &[&str],
        _timeout: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput> {
        if cancel.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: "emulator".into(),
            });
        }

        let key = args.first().map(|s| s.to_string()).unwrap_or_default();
        self.outputs
            .get(&key)
            .cloned()
            .ok_or_else(|| MobileError::ToolNotFound {
                tool_name: format!("emulator {}", args.join(" ")),
            })
    }
}

/// System emulator runner that executes the real `emulator` binary.
///
/// Delegates to the same process execution infrastructure as `SystemAdbRunner`.
pub struct SystemEmulatorRunner {
    adb_runner: Arc<dyn crate::adb_runner::AdbCommandRunner>,
}

impl SystemEmulatorRunner {
    pub fn new(adb_runner: Arc<dyn crate::adb_runner::AdbCommandRunner>) -> Self {
        Self { adb_runner }
    }
}

#[async_trait]
impl EmulatorCommandRunner for SystemEmulatorRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput> {
        self.adb_runner.run(program, args, timeout, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_runner_returns_configured_output() {
        let mut runner = FakeEmulatorRunner::new();
        runner.set_output(
            "-list-avds",
            AdbCommandOutput {
                exit_code: Some(0),
                stdout: "Pixel_4\nPixel_6\n".to_string(),
                stderr: String::new(),
            },
        );

        let cancel = CancellationToken::new();
        let output = runner
            .run(
                "emulator",
                &["-list-avds"],
                Duration::from_secs(10),
                &cancel,
            )
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.contains("Pixel_4"));
    }

    #[tokio::test]
    async fn fake_runner_returns_error_for_unknown_command() {
        let runner = FakeEmulatorRunner::new();
        let cancel = CancellationToken::new();
        let result = runner
            .run("emulator", &["-unknown"], Duration::from_secs(10), &cancel)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fake_runner_respects_cancellation() {
        let runner = FakeEmulatorRunner::new();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = runner
            .run(
                "emulator",
                &["-list-avds"],
                Duration::from_secs(10),
                &cancel,
            )
            .await;
        assert!(matches!(result, Err(MobileError::Cancelled { .. })));
    }
}
