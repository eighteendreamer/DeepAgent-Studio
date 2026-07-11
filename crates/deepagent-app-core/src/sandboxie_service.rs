//! Sandboxie-Plus integration for the Windows software sandbox.
//!
//! This is intentionally a runner layer, not a replacement for the existing
//! permission hooks. The app still decides what a tool may do; Sandboxie adds an
//! OS-level containment layer around local shell/git commands.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use deepagent_builtins::bash_tool::{CommandExecutor, CommandOutcome, SystemExecutor};
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

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
        let Some(tools) = self.ensure_tools_available()? else {
            return Ok(self.status());
        };
        let project_root = project_root.as_ref();

        self.run_sbie_ini(&tools, ["set", self.box_name.as_str(), "Enabled", "y"])?;
        self.run_sbie_ini(&tools, ["set", self.box_name.as_str(), "ConfigLevel", "10"])?;
        self.run_sbie_ini(&tools, ["set", self.box_name.as_str(), "AutoRecover", "n"])?;
        if !project_root.as_os_str().is_empty() {
            let project = display_path(project_root);
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

    fn run_command_in_box(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
        let Some(tools) = self.ensure_tools_available()? else {
            return Err(CoreError::not_found(
                "Sandboxie-Plus Start.exe was not found",
            ));
        };
        let _ = self.initialize_for_project(cwd);

        let box_arg = format!("/box:{}", self.box_name);
        let output = std::thread::scope(|_| {
            let mut cmd = Command::new(&tools.start_exe);
            cmd.args([
                box_arg.as_str(),
                "/wait",
                "/hide_window",
                "cmd.exe",
                "/C",
                command,
            ]);
            cmd.current_dir(cwd);
            configure_hidden_process(&mut cmd);
            cmd.output()
        })
        .map_err(|e| CoreError::other(format!("failed to run Sandboxie Start.exe: {e}")))?;

        Ok(CommandOutcome {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
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
}

impl SandboxieExecutor {
    pub fn new(service: Arc<SandboxieService>) -> Self {
        Self {
            service,
            fallback: SystemExecutor,
        }
    }
}

#[async_trait]
impl CommandExecutor for SandboxieExecutor {
    async fn run(&self, command: &str, cwd: &str) -> Result<CommandOutcome> {
        let command = command.to_string();
        let cwd = cwd.to_string();
        let service = self.service.clone();
        let sandbox_command = command.clone();
        let sandbox_cwd = cwd.clone();
        match tokio::task::spawn_blocking(move || {
            service.run_command_in_box(&sandbox_command, &sandbox_cwd)
        })
        .await
        {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(_)) | Err(_) => self.fallback.run(command.as_str(), cwd.as_str()).await,
        }
    }
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
