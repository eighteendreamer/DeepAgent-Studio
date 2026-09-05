use async_trait::async_trait;
use deepagent_mobile_core::{MobileError, MobileResult};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Result of running an ADB command.
#[derive(Debug, Clone)]
pub struct AdbCommandOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Trait for executing ADB commands. Abstracted for testability.
///
/// Implementations must:
/// - Use argv arrays (no shell interpolation)
/// - Respect cancellation and timeout
/// - Capture stdout/stderr separately
/// - Return structured output
#[async_trait]
pub trait AdbCommandRunner: Send + Sync {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput>;
}

/// Real system ADB command runner. Uses `tokio::process::Command` with argv
/// arrays, no shell. Follows the established pattern from
/// `deepagent-verification/src/runner.rs` with added timeout and cancellation.
pub struct SystemAdbRunner;

impl SystemAdbRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemAdbRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AdbCommandRunner for SystemAdbRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput> {
        if cancel.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: "adb-command".into(),
            });
        }

        let mut cmd = std::process::Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = tokio::process::Command::from(cmd)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| MobileError::ToolExecutionFailed {
                tool_name: program.to_string(),
                exit_code: -1,
                stderr: format!("failed to spawn: {e}"),
            })?;

        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        let stdout_task = stdout_handle.map(|s| {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let mut reader = tokio::io::BufReader::new(s);
                let _ = reader.read_to_string(&mut buf).await;
                buf
            })
        });

        let stderr_task = stderr_handle.map(|s| {
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                let mut reader = tokio::io::BufReader::new(s);
                let _ = reader.read_to_string(&mut buf).await;
                buf
            })
        });

        let result = tokio::select! {
            status = child.wait() => {
                let exit_code = status.ok().and_then(|s| s.code());
                let stdout = match stdout_task {
                    Some(t) => t.await.unwrap_or_default(),
                    None => String::new(),
                };
                let stderr = match stderr_task {
                    Some(t) => t.await.unwrap_or_default(),
                    None => String::new(),
                };
                Ok(AdbCommandOutput { exit_code, stdout, stderr })
            }
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                Err(MobileError::Cancelled {
                    operation_id: "adb-command".into(),
                })
            }
            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                Err(MobileError::Timeout {
                    operation_id: "adb-command".into(),
                    elapsed_ms: timeout.as_millis() as u64,
                })
            }
        };

        result
    }
}

/// Fake ADB command runner for testing. Returns pre-configured outputs keyed
/// by the first argument (e.g. "devices", "screencap").
pub struct FakeAdbRunner {
    responses: Arc<tokio::sync::Mutex<std::collections::HashMap<String, AdbCommandOutput>>>,
}

impl FakeAdbRunner {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn set_output(&self, key: &str, output: AdbCommandOutput) {
        self.responses.lock().await.insert(key.to_string(), output);
    }
}

impl Default for FakeAdbRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AdbCommandRunner for FakeAdbRunner {
    async fn run(
        &self,
        _program: &str,
        args: &[&str],
        _timeout: Duration,
        cancel: &CancellationToken,
    ) -> MobileResult<AdbCommandOutput> {
        if cancel.is_cancelled() {
            return Err(MobileError::Cancelled {
                operation_id: "adb-command".into(),
            });
        }

        let key = args.first().copied().unwrap_or("unknown");
        let responses = self.responses.lock().await;
        Ok(responses.get(key).cloned().unwrap_or(AdbCommandOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_runner_returns_configured_output() {
        let runner = FakeAdbRunner::new();
        runner
            .set_output(
                "devices",
                AdbCommandOutput {
                    exit_code: Some(0),
                    stdout: "List of devices attached\nABC123\tdevice\n".into(),
                    stderr: String::new(),
                },
            )
            .await;

        let cancel = CancellationToken::new();
        let result = runner
            .run("adb", &["devices", "-l"], Duration::from_secs(5), &cancel)
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("ABC123"));
    }

    #[tokio::test]
    async fn fake_runner_cancelled() {
        let runner = FakeAdbRunner::new();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = runner
            .run("adb", &["devices"], Duration::from_secs(5), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, MobileError::Cancelled { .. }));
    }

    #[tokio::test]
    async fn fake_runner_default_output() {
        let runner = FakeAdbRunner::new();
        let cancel = CancellationToken::new();
        let result = runner
            .run("adb", &["unknown-cmd"], Duration::from_secs(5), &cancel)
            .await
            .unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.is_empty());
    }
}
