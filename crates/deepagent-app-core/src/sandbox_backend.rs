//! Unified sandbox backend boundary for one-shot command execution.
//!
//! Existing tools still consume [`deepagent_builtins::bash_tool::CommandExecutor`].
//! This layer makes the sandbox choice explicit for Harness/CLI/server code and
//! adapts back to the existing command-executor contract instead of introducing
//! another tool runtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use deepagent_builtins::bash_tool::{
    CommandExecutor, CommandOutcome, CommandShell, SystemExecutor,
};
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

use crate::settings::SandboxMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxBackendKind {
    Direct,
    Sandboxie,
    WindowsSandbox,
}

impl SandboxBackendKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Sandboxie => "sandboxie",
            Self::WindowsSandbox => "windows_sandbox",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "direct" | "system" => Some(Self::Direct),
            "sandboxie" | "sandboxie_preferred" => Some(Self::Sandboxie),
            "windows_sandbox" | "windows-sandbox" | "wsb" => Some(Self::WindowsSandbox),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxNetworkPolicy {
    Enabled,
    Disabled,
}

impl SandboxNetworkPolicy {
    const fn wsb_value(self) -> &'static str {
        match self {
            Self::Enabled => "Enable",
            Self::Disabled => "Disable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxCapabilities {
    pub kind: SandboxBackendKind,
    pub available: bool,
    pub supports_one_shot: bool,
    pub supports_interactive_pty: bool,
    pub supports_network_toggle: bool,
    pub supports_readonly_mapping: bool,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct SandboxExecutionRequest {
    pub command: String,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub shell: CommandShell,
    pub timeout: Duration,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    pub environment: Vec<(String, String)>,
    pub sandbox_mode: SandboxMode,
    pub network: SandboxNetworkPolicy,
    pub allow_writable_host_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxExecutionResult {
    pub outcome: CommandOutcome,
    pub backend: SandboxBackendKind,
    pub artifacts: Vec<PathBuf>,
}

#[async_trait]
pub trait SandboxBackend: Send + Sync {
    fn capabilities(&self) -> SandboxCapabilities;

    async fn execute(&self, request: SandboxExecutionRequest) -> Result<SandboxExecutionResult>;
}

#[derive(Debug, Clone, Default)]
pub struct DirectSandboxBackend {
    executor: SystemExecutor,
}

impl DirectSandboxBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SandboxBackend for DirectSandboxBackend {
    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            kind: SandboxBackendKind::Direct,
            available: true,
            supports_one_shot: true,
            supports_interactive_pty: true,
            supports_network_toggle: false,
            supports_readonly_mapping: false,
            message: "direct host execution".to_string(),
        }
    }

    async fn execute(&self, request: SandboxExecutionRequest) -> Result<SandboxExecutionResult> {
        let cwd = request.cwd.to_string_lossy().to_string();
        let outcome = self
            .executor
            .run_controlled_with_environment(
                &request.command,
                &cwd,
                request.shell,
                request.cancel,
                request.timeout,
                &request.environment,
            )
            .await?;
        Ok(SandboxExecutionResult {
            outcome,
            backend: SandboxBackendKind::Direct,
            artifacts: Vec::new(),
        })
    }
}

#[derive(Clone)]
pub struct SandboxieBackend {
    executor: Arc<crate::sandboxie_service::SandboxieExecutor>,
}

impl SandboxieBackend {
    pub fn new(executor: Arc<crate::sandboxie_service::SandboxieExecutor>) -> Self {
        Self { executor }
    }
}

#[async_trait]
impl SandboxBackend for SandboxieBackend {
    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            kind: SandboxBackendKind::Sandboxie,
            available: self.executor.is_available(),
            supports_one_shot: true,
            supports_interactive_pty: false,
            supports_network_toggle: false,
            supports_readonly_mapping: true,
            message: if self.executor.is_available() {
                "Sandboxie command execution".to_string()
            } else {
                "Sandboxie tools are unavailable; legacy executor fallback is not represented by this backend"
                    .to_string()
            },
        }
    }

    async fn execute(&self, request: SandboxExecutionRequest) -> Result<SandboxExecutionResult> {
        self.executor.set_sandbox_mode(request.sandbox_mode);
        let cwd = request.cwd.to_string_lossy().to_string();
        let outcome = self
            .executor
            .run_controlled_with_environment(
                &request.command,
                &cwd,
                request.shell,
                request.cancel,
                request.timeout,
                &request.environment,
            )
            .await?;
        Ok(SandboxExecutionResult {
            outcome,
            backend: SandboxBackendKind::Sandboxie,
            artifacts: Vec::new(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct WindowsSandboxBackend {
    availability: WindowsSandboxAvailability,
    task_root: PathBuf,
}

#[derive(Debug, Clone)]
enum WindowsSandboxAvailability {
    Auto,
    Unavailable(String),
}

impl WindowsSandboxBackend {
    pub fn new(task_root: impl Into<PathBuf>) -> Self {
        Self {
            availability: WindowsSandboxAvailability::Auto,
            task_root: task_root.into(),
        }
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            availability: WindowsSandboxAvailability::Unavailable(message.into()),
            task_root: std::env::temp_dir().join("deepagent-windows-sandbox"),
        }
    }

    fn availability_message(&self) -> Option<String> {
        match &self.availability {
            WindowsSandboxAvailability::Unavailable(message) => Some(message.clone()),
            WindowsSandboxAvailability::Auto => detect_windows_sandbox_unavailable_reason(),
        }
    }
}

#[async_trait]
impl SandboxBackend for WindowsSandboxBackend {
    fn capabilities(&self) -> SandboxCapabilities {
        let unavailable = self.availability_message();
        SandboxCapabilities {
            kind: SandboxBackendKind::WindowsSandbox,
            available: unavailable.is_none(),
            supports_one_shot: true,
            supports_interactive_pty: false,
            supports_network_toggle: true,
            supports_readonly_mapping: true,
            message: unavailable.unwrap_or_else(|| "Windows Sandbox is available".to_string()),
        }
    }

    async fn execute(&self, request: SandboxExecutionRequest) -> Result<SandboxExecutionResult> {
        if let Some(reason) = self.availability_message() {
            return Err(CoreError::other(format!(
                "Windows Sandbox backend unavailable: {reason}"
            )));
        }

        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let task_dir = self.task_root.join(&nonce).join("task");
        let staging_dir = self.task_root.join(&nonce).join("staging");
        let output_dir = self.task_root.join(&nonce).join("output");
        std::fs::create_dir_all(&task_dir).map_err(|error| {
            CoreError::other(format!("create Windows Sandbox task dir: {error}"))
        })?;
        std::fs::create_dir_all(&staging_dir).map_err(|error| {
            CoreError::other(format!("create Windows Sandbox staging dir: {error}"))
        })?;
        std::fs::create_dir_all(&output_dir).map_err(|error| {
            CoreError::other(format!("create Windows Sandbox output dir: {error}"))
        })?;

        let plan = WindowsSandboxTaskPlan::new(WindowsSandboxTaskPlanRequest {
            command: request.command,
            cwd: request.cwd,
            workspace_root: request.workspace_root,
            task_dir: task_dir.clone(),
            staging_dir: staging_dir.clone(),
            output_dir: output_dir.clone(),
            sandbox_mode: request.sandbox_mode,
            network: request.network,
            allow_writable_host_workspace: request.allow_writable_host_workspace,
            shell: request.shell,
            environment: request.environment,
        })?;
        std::fs::write(task_dir.join("run.ps1"), plan.task_script()).map_err(|error| {
            CoreError::other(format!("write Windows Sandbox task script: {error}"))
        })?;
        std::fs::write(task_dir.join("task.wsb"), plan.wsb_xml())
            .map_err(|error| CoreError::other(format!("write Windows Sandbox config: {error}")))?;

        let outcome = launch_windows_sandbox(
            &task_dir.join("task.wsb"),
            &output_dir,
            request.cancel,
            request.timeout,
        )
        .await?;
        let artifacts_dir = self.task_root.join(&nonce).join("artifacts");
        let artifacts = collect_artifacts(&output_dir, &artifacts_dir)?;
        Ok(SandboxExecutionResult {
            outcome,
            backend: SandboxBackendKind::WindowsSandbox,
            artifacts,
        })
    }
}

pub struct WindowsSandboxTaskPlanRequest {
    pub command: String,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub task_dir: PathBuf,
    pub staging_dir: PathBuf,
    pub output_dir: PathBuf,
    pub sandbox_mode: SandboxMode,
    pub network: SandboxNetworkPolicy,
    pub allow_writable_host_workspace: bool,
    pub shell: CommandShell,
    pub environment: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct WindowsSandboxTaskPlan {
    command: String,
    sandbox_cwd: String,
    task_dir: PathBuf,
    staging_dir: PathBuf,
    output_dir: PathBuf,
    workspace_root: PathBuf,
    workspace_read_only: bool,
    network: SandboxNetworkPolicy,
    shell: CommandShell,
    environment: Vec<(String, String)>,
}

impl WindowsSandboxTaskPlan {
    pub fn new(request: WindowsSandboxTaskPlanRequest) -> Result<Self> {
        let workspace_root = canonical_existing_dir(&request.workspace_root, "workspace root")?;
        let cwd = canonical_existing_dir(&request.cwd, "cwd")?;
        if !cwd.starts_with(&workspace_root) {
            return Err(CoreError::invalid(format!(
                "Windows Sandbox cwd is outside workspace: {}",
                cwd.display()
            )));
        }
        if matches!(request.sandbox_mode, SandboxMode::FullAccess)
            && !request.allow_writable_host_workspace
        {
            return Err(CoreError::invalid(
                "Windows Sandbox full-access writable host mapping requires approval",
            ));
        }
        let task_dir = ensure_host_dir(&request.task_dir, "task dir")?;
        let staging_dir = ensure_host_dir(&request.staging_dir, "staging dir")?;
        let output_dir = ensure_host_dir(&request.output_dir, "output dir")?;
        let workspace_read_only = !matches!(request.sandbox_mode, SandboxMode::FullAccess);
        let sandbox_cwd = sandbox_workspace_path(&workspace_root, &cwd)?;
        Ok(Self {
            command: request.command,
            sandbox_cwd,
            task_dir,
            staging_dir,
            output_dir,
            workspace_root,
            workspace_read_only,
            network: request.network,
            shell: request.shell,
            environment: request.environment,
        })
    }

    pub fn wsb_xml(&self) -> String {
        format!(
            concat!(
                "<Configuration>\n",
                "  <Networking>{network}</Networking>\n",
                "  <MappedFolders>\n",
                "{workspace}",
                "{task}",
                "{staging}",
                "{output}",
                "  </MappedFolders>\n",
                "  <LogonCommand>\n",
                "    <Command>powershell.exe -ExecutionPolicy Bypass -File C:\\DeepAgent\\task\\run.ps1</Command>\n",
                "  </LogonCommand>\n",
                "</Configuration>\n"
            ),
            network = self.network.wsb_value(),
            workspace = mapped_folder(
                &self.workspace_root,
                "C:\\DeepAgent\\workspace",
                self.workspace_read_only
            ),
            task = mapped_folder(&self.task_dir, "C:\\DeepAgent\\task", true),
            staging = mapped_folder(&self.staging_dir, "C:\\DeepAgent\\staging", false),
            output = mapped_folder(&self.output_dir, "C:\\DeepAgent\\output", false),
        )
    }

    pub fn task_script(&self) -> String {
        let env = self
            .environment
            .iter()
            .map(|(key, value)| {
                format!("$env:{} = {}\n", powershell_env_name(key), ps_quote(value))
            })
            .collect::<String>();
        let command = shell_command_for_guest(self.shell, &self.command);
        format!(
            concat!(
                "$ErrorActionPreference = 'Continue'\n",
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n",
                "{env}",
                "Set-Location -LiteralPath {cwd}\n",
                "$stdout = 'C:\\DeepAgent\\output\\stdout.txt'\n",
                "$stderr = 'C:\\DeepAgent\\output\\stderr.txt'\n",
                "{command} 1> $stdout 2> $stderr\n",
                "$code = if ($LASTEXITCODE -ne $null) {{ $LASTEXITCODE }} else {{ 0 }}\n",
                "Set-Content -LiteralPath 'C:\\DeepAgent\\output\\exit_code.txt' -Value $code -Encoding UTF8\n",
                "Set-Content -LiteralPath 'C:\\DeepAgent\\output\\done.txt' -Value 'done' -Encoding UTF8\n",
                "shutdown.exe /s /t 0 /f\n"
            ),
            env = env,
            cwd = ps_quote(&self.sandbox_cwd),
            command = command,
        )
    }
}

#[derive(Clone)]
pub struct SandboxBackendCommandExecutor {
    backend: Arc<dyn SandboxBackend>,
    workspace_root: PathBuf,
    sandbox_mode: SandboxMode,
    network: SandboxNetworkPolicy,
    allow_writable_host_workspace: bool,
}

impl SandboxBackendCommandExecutor {
    pub fn new(
        backend: Arc<dyn SandboxBackend>,
        workspace_root: impl Into<PathBuf>,
        sandbox_mode: SandboxMode,
    ) -> Self {
        Self {
            backend,
            workspace_root: workspace_root.into(),
            sandbox_mode,
            network: SandboxNetworkPolicy::Disabled,
            allow_writable_host_workspace: false,
        }
    }

    pub fn with_network(mut self, network: SandboxNetworkPolicy) -> Self {
        self.network = network;
        self
    }

    pub fn with_writable_host_workspace(mut self, allowed: bool) -> Self {
        self.allow_writable_host_workspace = allowed;
        self
    }
}

#[async_trait]
impl CommandExecutor for SandboxBackendCommandExecutor {
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
        self.run_controlled_with_environment(
            command,
            cwd,
            shell,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Duration::from_secs(120),
            &[],
        )
        .await
    }

    async fn run_with_environment(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
        environment: &[(String, String)],
    ) -> Result<CommandOutcome> {
        self.run_controlled_with_environment(
            command,
            cwd,
            shell,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            Duration::from_secs(120),
            environment,
        )
        .await
    }

    async fn run_controlled(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: Duration,
    ) -> Result<CommandOutcome> {
        self.run_controlled_with_environment(command, cwd, shell, cancel, timeout, &[])
            .await
    }

    async fn run_controlled_with_environment(
        &self,
        command: &str,
        cwd: &str,
        shell: CommandShell,
        cancel: Arc<std::sync::atomic::AtomicBool>,
        timeout: Duration,
        environment: &[(String, String)],
    ) -> Result<CommandOutcome> {
        let result = self
            .backend
            .execute(SandboxExecutionRequest {
                command: command.to_string(),
                cwd: PathBuf::from(cwd),
                workspace_root: self.workspace_root.clone(),
                shell,
                timeout,
                cancel,
                environment: environment.to_vec(),
                sandbox_mode: self.sandbox_mode,
                network: self.network,
                allow_writable_host_workspace: self.allow_writable_host_workspace,
            })
            .await?;
        Ok(result.outcome)
    }
}

fn canonical_existing_dir(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = path.canonicalize().map_err(|error| {
        CoreError::invalid(format!("{label} is not an existing directory: {error}"))
    })?;
    if !canonical.is_dir() {
        return Err(CoreError::invalid(format!(
            "{label} is not a directory: {}",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn ensure_host_dir(path: &Path, label: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(path)
        .map_err(|error| CoreError::other(format!("create {label}: {error}")))?;
    canonical_existing_dir(path, label)
}

fn sandbox_workspace_path(workspace_root: &Path, cwd: &Path) -> Result<String> {
    let relative = cwd
        .strip_prefix(workspace_root)
        .map_err(|_| CoreError::invalid(format!("cwd is outside workspace: {}", cwd.display())))?;
    if relative.as_os_str().is_empty() {
        return Ok("C:\\DeepAgent\\workspace".to_string());
    }
    Ok(format!(
        "C:\\DeepAgent\\workspace\\{}",
        relative.to_string_lossy().replace('/', "\\")
    ))
}

fn mapped_folder(host: &Path, sandbox: &str, read_only: bool) -> String {
    format!(
        concat!(
            "    <MappedFolder>\n",
            "      <HostFolder>{}</HostFolder>\n",
            "      <SandboxFolder>{}</SandboxFolder>\n",
            "      <ReadOnly>{}</ReadOnly>\n",
            "    </MappedFolder>\n"
        ),
        xml_escape(&host.to_string_lossy()),
        xml_escape(sandbox),
        if read_only { "true" } else { "false" }
    )
}

fn shell_command_for_guest(shell: CommandShell, command: &str) -> String {
    match shell {
        CommandShell::Cmd => format!("cmd.exe /D /S /C {}", ps_quote(command)),
        CommandShell::Bash | CommandShell::Sh | CommandShell::Zsh | CommandShell::Wsl => {
            format!("bash.exe -lc {}", ps_quote(command))
        }
        CommandShell::Auto | CommandShell::Powershell => {
            format!(
                "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command {}",
                ps_quote(command)
            )
        }
    }
}

fn powershell_env_name(key: &str) -> String {
    key.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>()
}

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn collect_artifacts(output_dir: &Path, artifacts_dir: &Path) -> Result<Vec<PathBuf>> {
    if !output_dir.exists() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(artifacts_dir)
        .map_err(|error| CoreError::other(format!("create artifacts dir: {error}")))?;
    let mut copied = Vec::new();
    collect_artifacts_inner(output_dir, output_dir, artifacts_dir, &mut copied)?;
    Ok(copied)
}

fn collect_artifacts_inner(
    root: &Path,
    current: &Path,
    artifacts_dir: &Path,
    copied: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in std::fs::read_dir(current)
        .map_err(|error| CoreError::other(format!("read artifacts dir: {error}")))?
    {
        let entry =
            entry.map_err(|error| CoreError::other(format!("read artifact entry: {error}")))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|error| CoreError::other(format!("artifact path escape: {error}")))?;
        let target = artifacts_dir.join(relative);
        if path.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| CoreError::other(format!("create artifact subdir: {error}")))?;
            collect_artifacts_inner(root, &path, artifacts_dir, copied)?;
        } else if path.is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    CoreError::other(format!("create artifact parent: {error}"))
                })?;
            }
            std::fs::copy(&path, &target)
                .map_err(|error| CoreError::other(format!("copy artifact: {error}")))?;
            copied.push(target);
        }
    }
    Ok(())
}

fn detect_windows_sandbox_unavailable_reason() -> Option<String> {
    if !cfg!(windows) {
        return Some("Windows Sandbox is supported on Windows only".to_string());
    }
    if std::env::var_os("DEEPAGENT_ENABLE_WINDOWS_SANDBOX").is_none() {
        return Some("set DEEPAGENT_ENABLE_WINDOWS_SANDBOX=1 to opt in".to_string());
    }
    None
}

async fn launch_windows_sandbox(
    wsb_path: &Path,
    output_dir: &Path,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    timeout: Duration,
) -> Result<CommandOutcome> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return Err(CoreError::other("command cancelled before start"));
    }
    let command = format!(
        "Start-Process -FilePath {}",
        ps_quote(&wsb_path.to_string_lossy())
    );
    SystemExecutor
        .run_controlled(
            &command,
            ".",
            CommandShell::Powershell,
            cancel.clone(),
            timeout,
        )
        .await?;

    let done = output_dir.join("done.txt");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cancel.load(std::sync::atomic::Ordering::Acquire) {
            return Err(CoreError::other(
                "Windows Sandbox command cancelled before completion",
            ));
        }
        if done.is_file() {
            let stdout = std::fs::read_to_string(output_dir.join("stdout.txt")).unwrap_or_default();
            let stderr = std::fs::read_to_string(output_dir.join("stderr.txt")).unwrap_or_default();
            let exit_code = std::fs::read_to_string(output_dir.join("exit_code.txt"))
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok());
            return Ok(CommandOutcome {
                exit_code,
                stdout,
                stderr,
            });
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CoreError::other(
                "Windows Sandbox command timed out waiting for completion marker",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_builtins::bash_tool::CommandShell;

    #[test]
    fn backend_profile_parses_stable_labels() {
        assert_eq!(
            SandboxBackendKind::parse("direct"),
            Some(SandboxBackendKind::Direct)
        );
        assert_eq!(
            SandboxBackendKind::parse("sandboxie"),
            Some(SandboxBackendKind::Sandboxie)
        );
        assert_eq!(
            SandboxBackendKind::parse("windows_sandbox"),
            Some(SandboxBackendKind::WindowsSandbox)
        );
        assert_eq!(SandboxBackendKind::parse("unknown"), None);
    }

    #[test]
    fn windows_sandbox_wsb_maps_workspace_read_only_and_outputs_writable() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let task = temp.path().join("task");
        let staging = temp.path().join("staging");
        let output = temp.path().join("output");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&output).unwrap();

        let plan = WindowsSandboxTaskPlan::new(WindowsSandboxTaskPlanRequest {
            command: "cargo test".into(),
            cwd: workspace.clone(),
            workspace_root: workspace.clone(),
            task_dir: task.clone(),
            staging_dir: staging.clone(),
            output_dir: output.clone(),
            sandbox_mode: crate::settings::SandboxMode::WorkspaceWrite,
            network: SandboxNetworkPolicy::Disabled,
            allow_writable_host_workspace: false,
            shell: CommandShell::Powershell,
            environment: Vec::new(),
        })
        .unwrap();

        let wsb = plan.wsb_xml();
        assert!(wsb.contains("<Networking>Disable</Networking>"));
        assert!(wsb.contains("<ReadOnly>true</ReadOnly>"));
        assert!(wsb.contains("<SandboxFolder>C:\\DeepAgent\\workspace</SandboxFolder>"));
        assert!(wsb.contains("<SandboxFolder>C:\\DeepAgent\\staging</SandboxFolder>"));
        assert!(wsb.contains("<SandboxFolder>C:\\DeepAgent\\output</SandboxFolder>"));
        assert!(wsb.contains("<Command>powershell.exe -ExecutionPolicy Bypass -File C:\\DeepAgent\\task\\run.ps1</Command>"));
        assert!(plan.task_script().contains("cargo test"));
    }

    #[test]
    fn windows_sandbox_network_policy_is_explicit() {
        let plan = minimal_windows_plan(SandboxNetworkPolicy::Enabled).unwrap();
        assert!(plan.wsb_xml().contains("<Networking>Enable</Networking>"));
        let plan = minimal_windows_plan(SandboxNetworkPolicy::Disabled).unwrap();
        assert!(plan.wsb_xml().contains("<Networking>Disable</Networking>"));
    }

    #[test]
    fn windows_sandbox_full_access_requires_writable_host_approval() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let task = temp.path().join("task");
        let staging = temp.path().join("staging");
        let output = temp.path().join("output");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&output).unwrap();

        let denied = WindowsSandboxTaskPlan::new(WindowsSandboxTaskPlanRequest {
            command: "echo hi".into(),
            cwd: workspace.clone(),
            workspace_root: workspace.clone(),
            task_dir: task.clone(),
            staging_dir: staging.clone(),
            output_dir: output.clone(),
            sandbox_mode: crate::settings::SandboxMode::FullAccess,
            network: SandboxNetworkPolicy::Disabled,
            allow_writable_host_workspace: false,
            shell: CommandShell::Powershell,
            environment: Vec::new(),
        });
        assert!(denied.unwrap_err().to_string().contains("approval"));

        let approved = WindowsSandboxTaskPlan::new(WindowsSandboxTaskPlanRequest {
            command: "echo hi".into(),
            cwd: workspace.clone(),
            workspace_root: workspace,
            task_dir: task,
            staging_dir: staging,
            output_dir: output,
            sandbox_mode: crate::settings::SandboxMode::FullAccess,
            network: SandboxNetworkPolicy::Disabled,
            allow_writable_host_workspace: true,
            shell: CommandShell::Powershell,
            environment: Vec::new(),
        })
        .unwrap();
        assert!(approved.wsb_xml().contains("<ReadOnly>false</ReadOnly>"));
    }

    #[test]
    fn windows_sandbox_rejects_cwd_outside_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        let task = temp.path().join("task");
        let staging = temp.path().join("staging");
        let output = temp.path().join("output");
        for dir in [&workspace, &outside, &task, &staging, &output] {
            std::fs::create_dir_all(dir).unwrap();
        }

        let err = WindowsSandboxTaskPlan::new(WindowsSandboxTaskPlanRequest {
            command: "echo hi".into(),
            cwd: outside,
            workspace_root: workspace,
            task_dir: task,
            staging_dir: staging,
            output_dir: output,
            sandbox_mode: crate::settings::SandboxMode::WorkspaceWrite,
            network: SandboxNetworkPolicy::Disabled,
            allow_writable_host_workspace: false,
            shell: CommandShell::Powershell,
            environment: Vec::new(),
        })
        .unwrap_err();
        assert!(err.to_string().contains("outside workspace"));
    }

    #[test]
    fn collects_windows_sandbox_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("output");
        let artifacts = temp.path().join("artifacts");
        std::fs::create_dir_all(output.join("nested")).unwrap();
        std::fs::write(output.join("stdout.txt"), "hello").unwrap();
        std::fs::write(output.join("nested").join("report.txt"), "ok").unwrap();

        let copied = collect_artifacts(&output, &artifacts).unwrap();
        assert_eq!(copied.len(), 2);
        assert_eq!(
            std::fs::read_to_string(artifacts.join("nested").join("report.txt")).unwrap(),
            "ok"
        );
    }

    #[tokio::test]
    async fn windows_sandbox_unavailable_does_not_fallback_to_direct() {
        let backend = WindowsSandboxBackend::unavailable("Windows Sandbox is not installed");
        let request = SandboxExecutionRequest {
            command: "echo should-not-run".into(),
            cwd: std::env::current_dir().unwrap(),
            workspace_root: std::env::current_dir().unwrap(),
            shell: CommandShell::Powershell,
            timeout: std::time::Duration::from_secs(1),
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            environment: Vec::new(),
            sandbox_mode: crate::settings::SandboxMode::WorkspaceWrite,
            network: SandboxNetworkPolicy::Disabled,
            allow_writable_host_workspace: false,
        };

        let err = backend.execute(request).await.unwrap_err();
        assert!(err.to_string().contains("unavailable"));
    }

    fn minimal_windows_plan(
        network: SandboxNetworkPolicy,
    ) -> deepagent_core::error::Result<WindowsSandboxTaskPlan> {
        let root = tempfile::tempdir().unwrap().keep();
        let workspace = root.join("workspace");
        let task = root.join("task");
        let staging = root.join("staging");
        let output = root.join("output");
        for dir in [&workspace, &task, &staging, &output] {
            std::fs::create_dir_all(dir).unwrap();
        }
        WindowsSandboxTaskPlan::new(WindowsSandboxTaskPlanRequest {
            command: "echo hi".into(),
            cwd: workspace.clone(),
            workspace_root: workspace,
            task_dir: task,
            staging_dir: staging,
            output_dir: output,
            sandbox_mode: crate::settings::SandboxMode::WorkspaceWrite,
            network,
            allow_writable_host_workspace: false,
            shell: CommandShell::Powershell,
            environment: Vec::new(),
        })
    }
}
