//! Sandboxie-Plus integration for the Windows software sandbox.
//!
//! This is intentionally a runner layer, not a replacement for the existing
//! permission hooks. The app still decides what a tool may do; Sandboxie adds an
//! OS-level containment layer around local shell/git commands.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_builtins::bash_tool::{
    CommandExecutor, CommandOutcome, CommandShell, SystemExecutor,
};
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

use crate::settings::SandboxMode;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const DEFAULT_BOX_NAME: &str = "DeepAgentStudio";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxieStatusDto {
    pub supported: bool,
    pub ready: bool,
    pub box_name: String,
    pub install_dir: Option<String>,
    pub start_exe: Option<String>,
    pub sbie_ini_exe: Option<String>,
    pub bundled_installer: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
struct SandboxieTools {
    install_dir: PathBuf,
    start_exe: PathBuf,
    sbie_ini_exe: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SandboxieService {
    box_name: String,
    bundled_installer: Option<PathBuf>,
}

impl SandboxieService {
    pub fn new(bundled_installer: Option<PathBuf>) -> Self {
        Self {
            box_name: DEFAULT_BOX_NAME.to_string(),
            bundled_installer,
        }
    }

    pub fn box_name(&self) -> &str {
        &self.box_name
    }

    pub fn status(&self) -> SandboxieStatusDto {
        if !cfg!(windows) {
            return SandboxieStatusDto {
                supported: false,
                ready: false,
                box_name: self.box_name.clone(),
                install_dir: None,
                start_exe: None,
                sbie_ini_exe: None,
                bundled_installer: self.display_installer(),
                message: "Sandboxie-Plus is only supported on Windows.".to_string(),
            };
        }

        match self.locate_tools() {
            Some(tools) => SandboxieStatusDto {
                supported: true,
                ready: true,
                box_name: self.box_name.clone(),
                install_dir: Some(display_path(&tools.install_dir)),
                start_exe: Some(display_path(&tools.start_exe)),
                sbie_ini_exe: Some(display_path(&tools.sbie_ini_exe)),
                bundled_installer: self.display_installer(),
                message: "Sandboxie-Plus is installed and ready.".to_string(),
            },
            None => SandboxieStatusDto {
                supported: true,
                ready: false,
                box_name: self.box_name.clone(),
                install_dir: None,
                start_exe: None,
                sbie_ini_exe: None,
                bundled_installer: self.display_installer(),
                message: if self.bundled_installer_exists() {
                    "Sandboxie-Plus is bundled but not installed yet.".to_string()
                } else {
                    "Sandboxie-Plus is not installed and no bundled installer was found."
                        .to_string()
                },
            },
        }
    }

    /// Install the bundled Sandboxie-Plus setup in silent mode, then return
    /// the refreshed status snapshot.
    pub fn install_bundled(&self) -> Result<SandboxieStatusDto> {
        if !cfg!(windows) {
            return Ok(self.status());
        }
        if self.locate_tools().is_some() {
            return Ok(self.status());
        }
        self.install_bundled_silent()?;
        Ok(self.status())
    }

    /// Create/update the app sandbox. This is best-effort: missing Sandboxie
    /// does not block login or normal app startup.
    pub fn initialize_for_project(
        &self,
        project_root: impl AsRef<Path>,
    ) -> Result<SandboxieStatusDto> {
        self.initialize_for_project_with_mode(project_root, SandboxMode::WorkspaceWrite)
    }

    /// Create/update the app sandbox with mode-aware file access configuration.
    ///
    /// - **ReadOnly**: project directory is added to `ClosedFilePath` (denies
    ///   writes at the kernel level); reads still work because Sandboxie's
    ///   default allows reading.
    /// - **WorkspaceWrite**: project directory is added to `OpenFilePath`
    ///   (allows reads and writes within the project).
    /// - **FullAccess**: should not reach here (caller uses `SystemExecutor`
    ///   directly), but if it does, behaves like WorkspaceWrite.
    pub fn initialize_for_project_with_mode(
        &self,
        project_root: impl AsRef<Path>,
        mode: SandboxMode,
    ) -> Result<SandboxieStatusDto> {
        let Some(tools) = self.ensure_tools_available()? else {
            return Ok(self.status());
        };
        let project_root = project_root.as_ref();

        self.run_sbie_ini(&tools, ["set", self.box_name.as_str(), "Enabled", "y"])?;
        self.run_sbie_ini(&tools, ["set", self.box_name.as_str(), "ConfigLevel", "10"])?;
        self.run_sbie_ini(&tools, ["set", self.box_name.as_str(), "AutoRecover", "n"])?;
        if !project_root.as_os_str().is_empty() {
            let project = display_path(project_root);
            match mode {
                SandboxMode::ReadOnly => {
                    self.run_sbie_ini(
                        &tools,
                        [
                            "append",
                            self.box_name.as_str(),
                            "ClosedFilePath",
                            project.as_str(),
                        ],
                    )?;
                }
                SandboxMode::WorkspaceWrite | SandboxMode::FullAccess => {
                    self.run_sbie_ini(
                        &tools,
                        [
                            "append",
                            self.box_name.as_str(),
                            "OpenFilePath",
                            project.as_str(),
                        ],
                    )?;
                }
            }
        }

        Ok(SandboxieStatusDto {
            supported: true,
            ready: true,
            box_name: self.box_name.clone(),
            install_dir: Some(display_path(&tools.install_dir)),
            start_exe: Some(display_path(&tools.start_exe)),
            sbie_ini_exe: Some(display_path(&tools.sbie_ini_exe)),
            bundled_installer: self.display_installer(),
            message: "Sandboxie-Plus sandbox initialized.".to_string(),
        })
    }

    fn display_installer(&self) -> Option<String> {
        self.bundled_installer
            .as_ref()
            .filter(|p| p.is_file())
            .map(|p| display_path(p))
    }

    fn bundled_installer_exists(&self) -> bool {
        self.bundled_installer
            .as_ref()
            .map(|p| p.is_file())
            .unwrap_or(false)
    }

    fn ensure_tools_available(&self) -> Result<Option<SandboxieTools>> {
        if let Some(tools) = self.locate_tools() {
            return Ok(Some(tools));
        }
        if !cfg!(windows) || !self.bundled_installer_exists() {
            return Ok(None);
        }

        self.install_bundled_silent()?;
        let tools = self.locate_tools().ok_or_else(|| {
            CoreError::other(
                "Sandboxie-Plus installer completed, but Start.exe / SbieIni.exe were not found.",
            )
        })?;
        Ok(Some(tools))
    }

    fn locate_tools(&self) -> Option<SandboxieTools> {
        if !cfg!(windows) {
            return None;
        }

        for dir in sandboxie_candidate_dirs() {
            let start_exe = dir.join("Start.exe");
            let sbie_ini_exe = dir.join("SbieIni.exe");
            if start_exe.is_file() && sbie_ini_exe.is_file() {
                return Some(SandboxieTools {
                    install_dir: dir,
                    start_exe,
                    sbie_ini_exe,
                });
            }
        }
        None
    }

    fn run_sbie_ini<const N: usize>(&self, tools: &SandboxieTools, args: [&str; N]) -> Result<()> {
        let mut cmd = Command::new(&tools.sbie_ini_exe);
        cmd.args(args);
        configure_hidden_process(&mut cmd);
        let output = cmd
            .output()
            .map_err(|e| CoreError::other(format!("failed to run SbieIni.exe: {e}")))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(CoreError::other(format!(
            "SbieIni.exe failed: {}{}",
            stderr,
            if stdout.is_empty() {
                String::new()
            } else {
                format!("; stdout: {stdout}")
            }
        )))
    }

    fn run_command_in_box(
        &self,
        command: &str,
        cwd: &str,
        mode: SandboxMode,
        shell: CommandShell,
        environment: &[(String, String)],
    ) -> Result<CommandOutcome> {
        let Some(tools) = self.ensure_tools_available()? else {
            return Err(CoreError::not_found(
                "Sandboxie-Plus Start.exe was not found",
            ));
        };
        let _ = self.initialize_for_project_with_mode(cwd, mode);

        // Sandboxie's Start.exe never relays the boxed program's stdout or
        // stderr to its own handles, so capturing Start.exe output alone
        // leaves the model blind to every command result ("exit 1, empty
        // output" wall-banging, manual acceptance 2026-07-28). Wrap the
        // command so the boxed shell writes both streams to capture files in
        // the working directory (an OpenFilePath in WorkspaceWrite mode),
        // then read them back host-side. In ReadOnly mode the write is
        // denied and we degrade to the old behaviour.
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        let out_name = format!(".da-shell-{nonce}.out");
        let err_name = format!(".da-shell-{nonce}.err");
        let wrapped = wrap_command_with_capture(command, shell, &out_name, &err_name);

        let box_arg = format!("/box:{}", self.box_name);
        let output = std::thread::scope(|_| {
            let mut cmd = Command::new(&tools.start_exe);
            cmd.args([box_arg.as_str(), "/wait", "/hide_window"]);
            configure_sandboxed_shell_args(&mut cmd, shell, &wrapped);
            cmd.current_dir(cwd);
            cmd.envs(environment.iter().map(|(key, value)| (key, value)));
            configure_hidden_process(&mut cmd);
            cmd.output()
        })
        .map_err(|e| CoreError::other(format!("failed to run Sandboxie Start.exe: {e}")))?;

        let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if let Some(captured) = collect_capture_file(cwd, &out_name, &self.box_name) {
            if !captured.is_empty() {
                if !stdout.is_empty() && !stdout.ends_with('\n') {
                    stdout.push('\n');
                }
                stdout.push_str(&captured);
            }
        }
        if let Some(captured) = collect_capture_file(cwd, &err_name, &self.box_name) {
            if !captured.is_empty() {
                if !stderr.is_empty() && !stderr.ends_with('\n') {
                    stderr.push('\n');
                }
                stderr.push_str(&captured);
            }
        }

        Ok(CommandOutcome {
            exit_code: output.status.code(),
            stdout,
            stderr,
        })
    }

    /// Terminate every process running in the app sandbox.
    ///
    /// Official Sandboxie semantics (StartCommandLine docs, "Stop Programs"):
    /// `Start.exe /box:<name> /terminate` transmits the request to the
    /// SbieSvc service, which kills all boxed processes. This is the ONLY
    /// reliable way to stop a sandboxed command tree — killing our Start.exe
    /// child does not touch the programs inside the box.
    pub fn terminate_box_processes(&self) -> Result<()> {
        let Some(tools) = self.locate_tools() else {
            return Ok(());
        };
        let box_arg = format!("/box:{}", self.box_name);
        let mut cmd = Command::new(&tools.start_exe);
        cmd.args([box_arg.as_str(), "/terminate"]);
        configure_hidden_process(&mut cmd);
        let output = cmd
            .output()
            .map_err(|e| CoreError::other(format!("failed to run Start.exe /terminate: {e}")))?;
        if !output.status.success() {
            return Err(CoreError::other(format!(
                "Start.exe /terminate exited with {:?}",
                output.status.code()
            )));
        }
        Ok(())
    }

    fn install_bundled_silent(&self) -> Result<()> {
        let installer = self
            .bundled_installer
            .as_ref()
            .filter(|path| path.is_file())
            .ok_or_else(|| CoreError::not_found("bundled Sandboxie-Plus installer was not found"))?
            .clone();

        let script = format!(
            "$p = Start-Process -FilePath '{}' -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-' -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
            escape_powershell_single_quoted(&display_path(&installer))
        );
        let mut cmd = Command::new("powershell.exe");
        cmd.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ]);
        configure_hidden_process(&mut cmd);
        let output = cmd
            .output()
            .map_err(|e| CoreError::other(format!("failed to launch Sandboxie installer: {e}")))?;
        let exit_code = output.status.code().unwrap_or(-1);
        if output.status.success() || exit_code == 3010 {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Err(CoreError::other(format!(
            "Sandboxie-Plus silent install failed with exit code {exit_code}: {}{}",
            stderr,
            if stdout.is_empty() {
                String::new()
            } else {
                format!("; stdout: {stdout}")
            }
        )))
    }
}

#[derive(Clone)]
pub struct SandboxieExecutor {
    service: Arc<SandboxieService>,
    fallback: SystemExecutor,
    /// Atomic sandbox mode: 0=ReadOnly, 1=WorkspaceWrite, 2=FullAccess.
    /// Updated per-run so concurrent sessions use the correct confinement.
    sandbox_mode: Arc<AtomicU8>,
}

impl SandboxieExecutor {
    pub fn new(service: Arc<SandboxieService>) -> Self {
        Self {
            service,
            fallback: SystemExecutor,
            sandbox_mode: Arc::new(AtomicU8::new(1)),
        }
    }

    /// Set the sandbox mode for OS-level file access configuration (builder).
    pub fn with_sandbox_mode(self, mode: SandboxMode) -> Self {
        self.set_sandbox_mode(mode);
        self
    }

    /// Dynamically update the sandbox mode (called per-chat-run).
    pub fn set_sandbox_mode(&self, mode: SandboxMode) {
        let val = match mode {
            SandboxMode::ReadOnly => 0,
            SandboxMode::WorkspaceWrite => 1,
            SandboxMode::FullAccess => 2,
        };
        self.sandbox_mode.store(val, Ordering::Relaxed);
    }

    /// Whether the underlying Sandboxie tools are available.
    ///
    /// The legacy executor may still retain its compatibility fallback to
    /// direct execution; the Harness backend reports this separately so a
    /// caller can make an explicit safety decision.
    pub fn is_available(&self) -> bool {
        self.service.locate_tools().is_some()
    }

    fn current_mode(&self) -> SandboxMode {
        match self.sandbox_mode.load(Ordering::Relaxed) {
            0 => SandboxMode::ReadOnly,
            2 => SandboxMode::FullAccess,
            _ => SandboxMode::WorkspaceWrite,
        }
    }
}

#[async_trait]
impl CommandExecutor for SandboxieExecutor {
    async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
        self.run_with_options(command, cwd, CommandShell::Auto)
            .await
    }

    async fn run_with_options(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
    ) -> Result<CommandOutcome> {
        self.run_with_environment(command, cwd, shell, &[]).await
    }

    async fn run_with_environment(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
        environment: &[(String, String)],
    ) -> Result<CommandOutcome> {
        let command = command.to_string();
        let cwd = cwd.to_string();
        let environment = environment.to_vec();
        let service = self.service.clone();
        let sandbox_command = command.clone();
        let sandbox_cwd = cwd.clone();
        let sandbox_environment = environment.clone();
        let mode = self.current_mode();
        match tokio::task::spawn_blocking(move || {
            service.run_command_in_box(
                &sandbox_command,
                &sandbox_cwd,
                mode,
                shell,
                &sandbox_environment,
            )
        })
        .await
        {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(_)) | Err(_) => {
                self.fallback
                    .run_with_environment(command.as_str(), cwd.as_str(), shell, &environment)
                    .await
            }
        }
    }

    /// Kernel-owned cancellation/deadline for sandboxed commands (M-06).
    ///
    /// The boxed process tree lives OUTSIDE our Job Object, so the
    /// SystemExecutor kill path cannot reach it. Supervise the blocking
    /// Start.exe run and, on cancel/timeout, ask SbieSvc to terminate every
    /// process in the box (`Start.exe /box:<name> /terminate` — official
    /// Sandboxie "Stop Programs" command line).
    async fn run_controlled(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
    ) -> Result<CommandOutcome> {
        self.run_controlled_with_environment(command, cwd, shell, cancel, timeout, &[])
            .await
    }

    async fn run_controlled_with_environment(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
        timeout: std::time::Duration,
        environment: &[(String, String)],
    ) -> Result<CommandOutcome> {
        use std::sync::atomic::Ordering;
        if cancel.load(Ordering::Acquire) {
            return Err(CoreError::other("command cancelled before start"));
        }
        // Fall back to the system executor (with its own full kill support)
        // when Sandboxie is not installed, instead of losing control flags.
        if self.service.locate_tools().is_none() {
            return self
                .fallback
                .run_controlled_with_environment(command, cwd, shell, cancel, timeout, environment)
                .await;
        }

        let service = self.service.clone();
        let sandbox_command = command.to_string();
        let sandbox_cwd = cwd.to_string();
        let sandbox_environment = environment.to_vec();
        let mode = self.current_mode();
        let mut boxed_run = tokio::task::spawn_blocking(move || {
            service.run_command_in_box(
                &sandbox_command,
                &sandbox_cwd,
                mode,
                shell,
                &sandbox_environment,
            )
        });

        let cancel_watch = cancel.clone();
        let outcome = tokio::select! {
            joined = &mut boxed_run => Some(joined),
            _ = async {
                while !cancel_watch.load(Ordering::Acquire) {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            } => None,
            _ = tokio::time::sleep(timeout) => None,
        };

        match outcome {
            Some(Ok(Ok(out))) => Ok(out),
            Some(Ok(Err(_))) | Some(Err(_)) => {
                self.fallback
                    .run_controlled_with_environment(
                        command,
                        cwd,
                        shell,
                        cancel,
                        timeout,
                        environment,
                    )
                    .await
            }
            None => {
                // Cancelled or timed out: kill everything in the box, then
                // let the (now unblocked) Start.exe wait finish in background.
                let reason = if cancel.load(Ordering::Acquire) {
                    "command cancelled"
                } else {
                    "command timed out"
                };
                if let Err(error) = self.service.terminate_box_processes() {
                    tracing::warn!(%error, "Start.exe /terminate failed after {reason}");
                } else {
                    tracing::warn!(reason, "terminated sandboxed command via SbieSvc");
                }
                boxed_run.abort();
                Err(CoreError::other(format!(
                    "{reason}: sandboxed process tree terminated via Sandboxie /terminate"
                )))
            }
        }
    }
}

fn configure_sandboxed_shell_args(cmd: &mut Command, shell: CommandShell, command: &str) {
    match shell {
        CommandShell::Powershell => {
            let exe = if command_available("pwsh.exe") {
                "pwsh.exe"
            } else {
                "powershell.exe"
            };
            cmd.args([
                exe,
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &encode_powershell_command(command),
            ]);
        }
        CommandShell::Wsl => {
            cmd.args(["wsl.exe", "--", "bash", "-lc", command]);
        }
        CommandShell::Bash => {
            cmd.args(["bash.exe", "-lc", command]);
        }
        CommandShell::Zsh => {
            cmd.args(["zsh.exe", "-lc", command]);
        }
        CommandShell::Sh => {
            cmd.args(["sh.exe", "-c", command]);
        }
        CommandShell::Auto | CommandShell::Cmd => {
            let command = format!("chcp 65001 >NUL && {command}");
            cmd.args(["cmd.exe", "/D", "/S", "/C", command.as_str()]);
        }
    }
}

/// Wrap `command` so the boxed shell writes stdout/stderr into capture files
/// (relative names, resolved against the working directory that Start.exe
/// inherits). Relative paths sidestep quoting and WSL path-translation issues.
fn wrap_command_with_capture(
    command: &str,
    shell: CommandShell,
    out_name: &str,
    err_name: &str,
) -> String {
    match shell {
        CommandShell::Powershell => {
            format!("& {{ {command} }} 1> './{out_name}' 2> './{err_name}'")
        }
        CommandShell::Auto | CommandShell::Cmd => {
            format!("( {command} ) 1> \".\\{out_name}\" 2> \".\\{err_name}\"")
        }
        CommandShell::Wsl | CommandShell::Bash | CommandShell::Zsh | CommandShell::Sh => {
            format!("{{ {command}\n}} 1> './{out_name}' 2> './{err_name}'")
        }
    }
}

/// Read a capture file back on the host and delete it. Looks in the real
/// working directory first (direct write via OpenFilePath), then in the
/// Sandboxie overlay copies for paths that were virtualized into the box.
fn collect_capture_file(cwd: &str, name: &str, box_name: &str) -> Option<String> {
    let host = Path::new(cwd).join(name);
    let content = read_capture_text(&host);
    let _ = std::fs::remove_file(&host);
    if content.is_some() {
        return content;
    }
    for overlay in box_overlay_dirs(cwd, box_name) {
        let candidate = overlay.join(name);
        let content = read_capture_text(&candidate);
        let _ = std::fs::remove_file(&candidate);
        if content.is_some() {
            return content;
        }
    }
    None
}

/// Decode capture-file bytes. Windows PowerShell 5 writes `>` redirection as
/// UTF-16LE with a BOM; PowerShell 7 / cmd / POSIX shells write UTF-8.
fn read_capture_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Some(String::from_utf16_lossy(&units))
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Host-side directories where Sandboxie may have virtualized writes to
/// `cwd` (default box root `C:\Sandbox\<user>\<box>`): the user-profile
/// mapping (`user\current\...`) and the generic drive mapping
/// (`drive\<letter>\...`).
fn box_overlay_dirs(cwd: &str, box_name: &str) -> Vec<PathBuf> {
    let Ok(user) = std::env::var("USERNAME") else {
        return Vec::new();
    };
    let root = PathBuf::from(r"C:\Sandbox").join(user).join(box_name);
    let normalized = cwd.replace('/', "\\");
    let bytes = normalized.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' {
        return Vec::new();
    }
    let mut dirs = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let profile = profile.replace('/', "\\");
        if !profile.is_empty()
            && normalized
                .to_ascii_lowercase()
                .starts_with(&profile.to_ascii_lowercase())
        {
            let rest = normalized[profile.len()..].trim_start_matches('\\');
            dirs.push(root.join("user").join("current").join(rest));
        }
    }
    let drive = (bytes[0] as char).to_ascii_uppercase().to_string();
    let rest = normalized[2..].trim_start_matches('\\');
    dirs.push(root.join("drive").join(drive).join(rest));
    dirs
}

fn command_available(bin: &str) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    configure_hidden_process(&mut cmd);
    cmd.output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn encode_powershell_command(command: &str) -> String {
    let command = wrap_powershell_command(command);
    let mut utf16le = Vec::with_capacity(command.len() * 2);
    for unit in command.encode_utf16() {
        utf16le.extend_from_slice(&unit.to_le_bytes());
    }
    base64_encode(&utf16le)
}

fn wrap_powershell_command(command: &str) -> String {
    format!(
        "{}; if ($null -ne $global:LASTEXITCODE) {{ exit $global:LASTEXITCODE }} elseif ($?) {{ exit 0 }} else {{ exit 1 }}",
        command
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn sandboxie_candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = std::env::var("DEEPAGENT_SANDBOXIE_HOME") {
        if !home.trim().is_empty() {
            dirs.push(PathBuf::from(home));
        }
    }
    for env in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(env) {
            dirs.push(PathBuf::from(&base).join("Sandboxie-Plus"));
            dirs.push(PathBuf::from(&base).join("Sandboxie"));
        }
    }
    dirs
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn escape_powershell_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

fn configure_hidden_process(_cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        _cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_wrapping_redirects_both_streams_per_shell() {
        let ps = wrap_command_with_capture("node x.js", CommandShell::Powershell, "o.out", "e.err");
        assert_eq!(ps, "& { node x.js } 1> './o.out' 2> './e.err'");
        let cmd = wrap_command_with_capture("node x.js", CommandShell::Cmd, "o.out", "e.err");
        assert_eq!(cmd, "( node x.js ) 1> \".\\o.out\" 2> \".\\e.err\"");
        let auto = wrap_command_with_capture("a && b", CommandShell::Auto, "o.out", "e.err");
        assert_eq!(auto, "( a && b ) 1> \".\\o.out\" 2> \".\\e.err\"");
        // POSIX shells use a newline before `}` so trailing comments or `&`
        // in the user command cannot swallow the closing brace.
        let bash = wrap_command_with_capture("make # build", CommandShell::Bash, "o", "e");
        assert_eq!(bash, "{ make # build\n} 1> './o' 2> './e'");
    }

    #[test]
    fn capture_reader_decodes_utf16le_and_utf8() {
        let dir = std::env::temp_dir().join(format!("da-capture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // PS5-style UTF-16LE with BOM.
        let utf16 = dir.join("u16.out");
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "Cannot find module 'docx'".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        std::fs::write(&utf16, &bytes).unwrap();
        assert_eq!(
            read_capture_text(&utf16).unwrap(),
            "Cannot find module 'docx'"
        );
        // Plain UTF-8.
        let utf8 = dir.join("u8.out");
        std::fs::write(&utf8, "错误: 模块缺失").unwrap();
        assert_eq!(read_capture_text(&utf8).unwrap(), "错误: 模块缺失");
        // Missing file → None (degrades to old behaviour).
        assert!(read_capture_text(&dir.join("absent")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_reads_and_deletes_from_the_working_directory() {
        let dir = std::env::temp_dir().join(format!("da-collect-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c.out"), "exit status detail").unwrap();
        let cwd = dir.to_string_lossy().to_string();
        assert_eq!(
            collect_capture_file(&cwd, "c.out", "DeepAgentBox").unwrap(),
            "exit status detail"
        );
        // The capture file must not survive (no litter in the workspace).
        assert!(!dir.join("c.out").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlay_candidates_map_drive_and_profile_paths() {
        if std::env::var("USERNAME").is_err() {
            return; // non-Windows CI
        }
        let dirs = box_overlay_dirs("G:\\managed-test", "DeepAgentBox");
        assert!(dirs.iter().any(|p| {
            let s = p.to_string_lossy().to_string();
            s.contains("\\DeepAgentBox\\drive\\G\\managed-test")
        }));
        if let Ok(profile) = std::env::var("USERPROFILE") {
            let cwd = format!("{profile}\\proj");
            let dirs = box_overlay_dirs(&cwd, "DeepAgentBox");
            assert!(dirs.iter().any(|p| {
                let s = p.to_string_lossy().to_string();
                s.contains("\\DeepAgentBox\\user\\current\\proj")
            }));
        }
    }
}
