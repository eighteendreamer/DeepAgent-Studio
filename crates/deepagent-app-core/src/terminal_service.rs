//! Interactive terminal for the desktop Terminal panel (Phase C).
//!
//! Runs one-shot shell commands in the **active project** directory and returns
//! captured output. Two safety properties mirror the agent's `bash` tool:
//! - **Dangerous commands are refused** (`rm -rf`, `curl | sh`, `sudo`, …) and
//!   reported as `blocked` rather than executed — these need explicit approval.
//! - Commands run with the active project folder as the working directory, so
//!   the panel operates within the same project the agent does.
//!
//! Unlike the agent's `bash` tool this panel is **not** allow-list gated: it is
//! a user-driven terminal, so any non-dangerous command the user types runs.
//! The dangerous-command refusal remains as a guardrail.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use deepagent_builtins::{is_dangerous, CommandExecutor, SystemExecutor};
use deepagent_core::error::{CoreError, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::dto::TerminalResultDto;
use crate::project_service::ProjectService;
use crate::runtime_service::RuntimeBroker;
use crate::settings::TerminalShell;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalPtyHandle {
    pub pty_id: String,
    pub cols: u16,
    pub rows: u16,
}

struct LocalPtyState {
    writer: Mutex<Box<dyn Write + Send>>,
    reader: Mutex<mpsc::Receiver<Vec<u8>>>,
    master: std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    child: std::sync::Mutex<Box<dyn portable_pty::Child + Send>>,
}

/// Runs interactive terminal commands in the active project directory.
pub struct TerminalService {
    projects: Arc<ProjectService>,
    /// Fallback working directory when no project is active.
    default_cwd: String,
    executor: Arc<dyn CommandExecutor>,
    runtime: Option<Arc<RuntimeBroker>>,
    ptys: RwLock<HashMap<String, Arc<LocalPtyState>>>,
    next_pty_id: AtomicU64,
}

impl TerminalService {
    /// Build over the project registry, with a default cwd used when no project
    /// is active. Uses the real [`SystemExecutor`].
    pub fn new(
        projects: Arc<ProjectService>,
        default_cwd: impl Into<String>,
        runtime: Arc<RuntimeBroker>,
    ) -> Self {
        Self {
            projects,
            default_cwd: default_cwd.into(),
            executor: Arc::new(SystemExecutor),
            runtime: Some(runtime),
            ptys: RwLock::new(HashMap::new()),
            next_pty_id: AtomicU64::new(1),
        }
    }

    /// Build with a custom executor (for tests).
    pub fn with_executor(
        projects: Arc<ProjectService>,
        default_cwd: impl Into<String>,
        executor: Arc<dyn CommandExecutor>,
    ) -> Self {
        Self {
            projects,
            default_cwd: default_cwd.into(),
            executor,
            runtime: None,
            ptys: RwLock::new(HashMap::new()),
            next_pty_id: AtomicU64::new(1),
        }
    }

    /// The working directory for terminal commands: the active project, else
    /// the default cwd.
    fn cwd(&self) -> String {
        self.projects
            .active()
            .ok()
            .flatten()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| self.default_cwd.clone())
    }

    /// Run `command` in the active project directory. Dangerous commands are
    /// refused (reported as `blocked`) instead of being executed.
    pub async fn run(&self, command: &str) -> Result<TerminalResultDto> {
        let command = command.trim();
        let cwd = self.cwd();

        if command.is_empty() {
            return Ok(TerminalResultDto {
                command: command.to_string(),
                cwd,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                blocked: false,
            });
        }

        if is_dangerous(command) {
            return Ok(TerminalResultDto {
                command: command.to_string(),
                cwd,
                exit_code: None,
                stdout: String::new(),
                stderr: format!("blocked: '{command}' is high-risk and requires explicit approval"),
                blocked: true,
            });
        }

        let outcome = self.executor.run(command, &cwd).await?;
        Ok(TerminalResultDto {
            command: command.to_string(),
            cwd,
            exit_code: outcome.exit_code,
            stdout: outcome.stdout,
            stderr: outcome.stderr,
            blocked: false,
        })
    }

    /// The current working directory (for the prompt display).
    pub fn current_dir(&self) -> String {
        self.cwd()
    }

    /// Spawn a local PTY-backed interactive shell rooted at the active project.
    pub async fn pty_spawn(
        &self,
        shell: TerminalShell,
        cols: u16,
        rows: u16,
    ) -> Result<LocalPtyHandle> {
        let cwd = self.cwd();
        let environment = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.build_process_environment(Some(std::path::Path::new(&cwd))))
            .unwrap_or_default();
        let state = spawn_local_pty(shell, &cwd, cols, rows, &environment)?;
        let id = format!(
            "local-pty-{}",
            self.next_pty_id.fetch_add(1, Ordering::Relaxed)
        );
        self.ptys.write().await.insert(id.clone(), Arc::new(state));
        Ok(LocalPtyHandle {
            pty_id: id,
            cols,
            rows,
        })
    }

    pub async fn pty_write(&self, handle: &LocalPtyHandle, data: &[u8]) -> Result<()> {
        let state = self
            .ptys
            .read()
            .await
            .get(&handle.pty_id)
            .cloned()
            .ok_or_else(|| CoreError::not_found(format!("pty {} not found", handle.pty_id)))?;
        let payload = data.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut writer = state.writer.blocking_lock();
            writer
                .write_all(&payload)
                .map_err(|e| CoreError::other(format!("pty write failed: {e}")))?;
            writer
                .flush()
                .map_err(|e| CoreError::other(format!("pty flush failed: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::other(format!("pty write join failed: {e}")))??;
        Ok(())
    }

    pub async fn pty_read(&self, handle: &LocalPtyHandle) -> Result<Vec<u8>> {
        let state = self
            .ptys
            .read()
            .await
            .get(&handle.pty_id)
            .cloned()
            .ok_or_else(|| CoreError::not_found(format!("pty {} not found", handle.pty_id)))?;
        let mut rx = state.reader.lock().await;
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.extend_from_slice(&chunk);
            if out.len() >= 64 * 1024 {
                break;
            }
        }
        Ok(out)
    }

    pub async fn pty_resize(&self, handle: &LocalPtyHandle, cols: u16, rows: u16) -> Result<()> {
        let state = self
            .ptys
            .read()
            .await
            .get(&handle.pty_id)
            .cloned()
            .ok_or_else(|| CoreError::not_found(format!("pty {} not found", handle.pty_id)))?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let master = state
                .master
                .lock()
                .map_err(|_| CoreError::other("pty master lock poisoned"))?;
            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|e| CoreError::other(format!("pty resize failed: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::other(format!("pty resize join failed: {e}")))??;
        Ok(())
    }

    pub async fn pty_close(&self, handle: &LocalPtyHandle) -> Result<()> {
        let state = self
            .ptys
            .write()
            .await
            .remove(&handle.pty_id)
            .ok_or_else(|| CoreError::not_found(format!("pty {} not found", handle.pty_id)))?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut child = state
                .child
                .lock()
                .map_err(|_| CoreError::other("pty child lock poisoned"))?;
            let _ = child.kill();
            let _ = child.wait();
            Ok(())
        })
        .await
        .map_err(|e| CoreError::other(format!("pty close join failed: {e}")))??;
        Ok(())
    }

    /// Launch the user's system terminal in the current working directory,
    /// using the preferred shell from settings.
    pub fn open_system(&self, shell: TerminalShell) -> Result<String> {
        let cwd = self.cwd();
        let environment = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.build_process_environment(Some(std::path::Path::new(&cwd))))
            .unwrap_or_default();
        spawn_system_terminal(shell, &cwd, &environment)?;
        Ok(cwd)
    }
}

fn spawn_local_pty(
    shell: TerminalShell,
    cwd: &str,
    cols: u16,
    rows: u16,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<LocalPtyState> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| CoreError::other(format!("open pty failed: {e}")))?;

    let cmd = terminal_command(shell, cwd, environment)?;
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| CoreError::other(format!("spawn shell failed: {e}")))?;

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| CoreError::other(format!("clone pty reader failed: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| CoreError::other(format!("take pty writer failed: {e}")))?;

    let (stdout_tx, stdout_rx) = mpsc::channel::<Vec<u8>>(256);
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if stdout_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(LocalPtyState {
        writer: Mutex::new(writer),
        reader: Mutex::new(stdout_rx),
        master: std::sync::Mutex::new(pair.master),
        child: std::sync::Mutex::new(child),
    })
}

fn terminal_command(
    shell: TerminalShell,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<CommandBuilder> {
    let mut cmd = match shell {
        TerminalShell::PowerShell => {
            let mut cmd = CommandBuilder::new("powershell.exe");
            cmd.arg("-NoLogo");
            cmd
        }
        TerminalShell::CommandPrompt => CommandBuilder::new("cmd.exe"),
        TerminalShell::GitBash => {
            let bash = find_git_bash_exe()
                .ok_or_else(|| CoreError::not_found("Git Bash was not found on this computer"))?;
            let mut cmd = CommandBuilder::new(bash);
            cmd.arg("--login");
            cmd.arg("-i");
            cmd
        }
        TerminalShell::Wsl => {
            let mut cmd = CommandBuilder::new("wsl.exe");
            if let Some(wsl_cwd) = to_wsl_path(cwd) {
                cmd.arg("--cd");
                cmd.arg(wsl_cwd);
            }
            cmd
        }
    };
    cmd.cwd(cwd);
    for (key, value) in environment {
        cmd.env(key, value);
    }
    Ok(cmd)
}

#[cfg(target_os = "windows")]
fn spawn_system_terminal(
    shell: TerminalShell,
    cwd: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    use std::process::Command;

    match shell {
        TerminalShell::PowerShell => {
            Command::new("powershell.exe")
                .args([
                    "-NoExit",
                    "-Command",
                    &format!("Set-Location -LiteralPath {}", powershell_quote(cwd)),
                ])
                .envs(environment)
                .spawn()
                .map_err(|e| CoreError::other(format!("failed to open PowerShell: {e}")))?;
        }
        TerminalShell::CommandPrompt => {
            Command::new("cmd.exe")
                .args(["/K", &format!("cd /d {}", cmd_quote(cwd))])
                .envs(environment)
                .spawn()
                .map_err(|e| CoreError::other(format!("failed to open Command Prompt: {e}")))?;
        }
        TerminalShell::GitBash => {
            let bash = find_git_bash_exe()
                .ok_or_else(|| CoreError::not_found("Git Bash was not found on this computer"))?;
            Command::new(bash)
                .args([
                    "--login",
                    "-i",
                    "-c",
                    &format!("cd {}; exec bash -i", bash_quote(cwd)),
                ])
                .envs(environment)
                .spawn()
                .map_err(|e| CoreError::other(format!("failed to open Git Bash: {e}")))?;
        }
        TerminalShell::Wsl => {
            let mut cmd = Command::new("wsl.exe");
            if let Some(wsl_cwd) = to_wsl_path(cwd) {
                cmd.args(["--cd", &wsl_cwd]);
            }
            cmd.envs(environment);
            cmd.spawn()
                .map_err(|e| CoreError::other(format!("failed to open WSL: {e}")))?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn spawn_system_terminal(
    _shell: TerminalShell,
    _cwd: &str,
    _environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    Err(CoreError::other(
        "opening a system terminal is currently supported on Windows only",
    ))
}

#[cfg(target_os = "windows")]
fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn cmd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(target_os = "windows")]
fn bash_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "windows")]
fn find_git_bash_exe() -> Option<String> {
    let mut candidates = Vec::new();
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        candidates.push(format!(r"{program_files}\Git\bin\bash.exe"));
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        candidates.push(format!(r"{program_files_x86}\Git\bin\bash.exe"));
    }
    if let Ok(local_app_data) = std::env::var("LocalAppData") {
        candidates.push(format!(r"{local_app_data}\Programs\Git\bin\bash.exe"));
    }
    candidates.push("bash.exe".to_string());
    candidates
        .into_iter()
        .find(|path| path.eq_ignore_ascii_case("bash.exe") || std::path::Path::new(path).exists())
}

#[cfg(not(target_os = "windows"))]
fn find_git_bash_exe() -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn to_wsl_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = trimmed[3..].replace('\\', "/");
        return Some(format!("/mnt/{drive}/{rest}"));
    }
    Some(trimmed.replace('\\', "/"))
}

#[cfg(not(target_os = "windows"))]
fn to_wsl_path(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use deepagent_builtins::CommandOutcome;
    use deepagent_persistence::Database;
    use std::sync::Mutex;

    struct RecordingExecutor {
        ran: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl CommandExecutor for RecordingExecutor {
        async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
            self.ran
                .lock()
                .unwrap()
                .push((command.to_string(), cwd.to_string()));
            Ok(CommandOutcome {
                exit_code: Some(0),
                stdout: format!("ran: {command}"),
                stderr: String::new(),
            })
        }
    }

    fn service() -> (TerminalService, Arc<RecordingExecutor>, Arc<ProjectService>) {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let projects = Arc::new(ProjectService::new(db));
        let exec = Arc::new(RecordingExecutor {
            ran: Mutex::new(Vec::new()),
        });
        let svc = TerminalService::with_executor(projects.clone(), "/default", exec.clone());
        (svc, exec, projects)
    }

    #[tokio::test]
    async fn runs_in_active_project_dir() {
        let (svc, exec, projects) = service();
        projects.add_project("/work/proj").unwrap();
        let res = svc.run("echo hi").await.unwrap();
        assert_eq!(res.cwd, "/work/proj");
        assert_eq!(res.exit_code, Some(0));
        assert!(!res.blocked);
        assert_eq!(exec.ran.lock().unwrap()[0].1, "/work/proj");
    }

    #[tokio::test]
    async fn falls_back_to_default_cwd() {
        let (svc, _exec, _projects) = service();
        let res = svc.run("echo hi").await.unwrap();
        assert_eq!(res.cwd, "/default");
    }

    #[tokio::test]
    async fn dangerous_command_is_blocked_not_run() {
        let (svc, exec, _projects) = service();
        let res = svc.run("rm -rf /").await.unwrap();
        assert!(res.blocked);
        assert!(res.stderr.contains("high-risk"));
        // The executor was never invoked.
        assert!(exec.ran.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_command_is_noop() {
        let (svc, exec, _projects) = service();
        let res = svc.run("   ").await.unwrap();
        assert!(!res.blocked);
        assert!(exec.ran.lock().unwrap().is_empty());
    }
}
