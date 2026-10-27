use super::*;
use deepagent_builtins::{BashTool, CommandShell, SystemExecutor};
use deepagent_hooks::{Hook, HookContext, HookOutcome, HookPoint};
use deepagent_runtime::{AutoApproveGate, AutoDenyGate, NullEventSink};
use deepagent_tools::PermissionSet;
use std::sync::{atomic::AtomicBool, Mutex};
use std::time::Duration;

struct PolicyHook(HookOutcome);
#[async_trait]
impl Hook for PolicyHook {
    fn name(&self) -> &str {
        "verification-policy-fixture"
    }
    async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
struct RecordingExecutor(Mutex<Vec<(String, Duration, Arc<AtomicBool>)>>);
#[async_trait]
impl CommandExecutor for RecordingExecutor {
    async fn run(&self, _: &str, _: &str) -> Result<CommandOutcome> {
        panic!("verification must use bash's controlled execution path")
    }
    async fn run_controlled(
        &self,
        command: &str,
        _: &str,
        _: CommandShell,
        cancel: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Result<CommandOutcome> {
        self.0
            .lock()
            .unwrap()
            .push((command.to_string(), timeout, cancel));
        Ok(CommandOutcome {
            exit_code: Some(0),
            stdout: "diagnostic".repeat(10_000),
            stderr: String::new(),
        })
    }
}

fn fixture(
    root: &Path,
    executor: Arc<dyn CommandExecutor>,
    outcome: HookOutcome,
) -> VerificationCommands {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(
            BashTool::new(executor, root.to_string_lossy(), Vec::<String>::new())
                .with_external_safety_gate(true),
        ))
        .unwrap();
    let mut hooks = HookRegistry::new();
    hooks.register(HookPoint::BeforeToolUse, Arc::new(PolicyHook(outcome)));
    let commands = VerificationCommands::new(
        VerificationCommandContext {
            registry: Arc::new(registry),
            session_id: SessionId::new(),
            hooks: Arc::new(hooks),
            approvals: Arc::new(AutoDenyGate),
            cancellation: CancellationTree::new(),
        },
        root,
        &RuntimeConfig::default(),
    );
    commands.bind_events(Arc::new(NullEventSink)).unwrap();
    commands
}

#[tokio::test]
async fn hook_approval_and_permissions_apply_to_automatic_commands() {
    let temp = tempfile::tempdir().unwrap();
    let exec = Arc::new(RecordingExecutor::default());
    for (outcome, permissions) in [
        (
            HookOutcome::deny("fixture denied"),
            PermissionSet::developer(),
        ),
        (
            HookOutcome::ask("fixture approval"),
            PermissionSet::developer(),
        ),
        (HookOutcome::Continue, PermissionSet::read_only()),
    ] {
        let mut commands = fixture(temp.path(), exec.clone(), outcome);
        commands.config.permissions = permissions;
        let out = CommandExecutor::run(&commands, "cargo check", temp.path().to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(out.exit_code, Some(1));
        assert!(!out.stderr.is_empty(), "denial must retain its diagnostic");
        assert!(
            exec.0.lock().unwrap().is_empty(),
            "denial must not reach the executor"
        );
    }
    let mut commands = fixture(
        temp.path(),
        exec.clone(),
        HookOutcome::ask("fixture approval"),
    );
    commands.context.approvals = Arc::new(AutoApproveGate);
    commands.config.tool_timeout = Duration::from_millis(123);
    commands.config.tool_result_budget.max_tokens = 4;
    commands.config.tool_result_budget.output_dir = temp.path().join("results");
    let out = CommandRunner::run(&commands, &Command::parse("cargo check"))
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    assert_eq!(
        out.stdout,
        "diagnostic".repeat(10_000),
        "budgeted UI output must not erase verifier diagnostics/status"
    );
    let calls = exec.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, Duration::from_millis(123));
    assert!(Arc::ptr_eq(
        &calls[0].2,
        &commands.context.cancellation.legacy_flag()
    ));
}

#[tokio::test]
async fn cancelled_checks_and_outside_cwd_never_enter_executor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let exec = Arc::new(RecordingExecutor::default());
    let commands = fixture(&root, exec.clone(), HookOutcome::Continue);
    let out = CommandExecutor::run(&commands, "cargo check", temp.path().to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(1));
    assert!(out.stderr.contains("outside the run workspace"));
    commands.context.cancellation.cancel();
    let out = CommandExecutor::run(&commands, "cargo check", root.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(1));
    assert!(out.stderr.contains("cancelled"));
    assert!(exec.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn generated_argv_and_nested_cwd_preserve_literal_paths_in_real_shell() {
    let temp = tempfile::tempdir().unwrap();
    let nested = temp.path().join("nested 空格 $literal 'quote");
    std::fs::create_dir(&nested).unwrap();
    let commands = fixture(temp.path(), Arc::new(SystemExecutor), HookOutcome::Continue);
    // Canonical Windows paths can use the verbatim prefix; ProviderPath is the
    // OS cwd. The fixture explicitly emits UTF-8: SystemExecutor decodes UTF-8
    // while PowerShell can inherit a legacy console code page on Windows.
    let line = if cfg!(windows) {
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $PWD.ProviderPath"
    } else {
        "pwd"
    };
    let out = CommandExecutor::run(&commands, line, nested.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0), "{out:?}");
    assert_eq!(
        Path::new(out.stdout.trim())
            .canonicalize()
            .unwrap_or_else(|error| panic!("cwd output {out:?}: {error}")),
        nested.canonicalize().unwrap()
    );
    let value = "空格 $literal 'quote ; unchanged";
    let script = temp.path().join("argv 空格 $literal 'quote.ps1");
    let command = if cfg!(windows) {
        std::fs::write(
            &script,
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)\nWrite-Output $args[0]\n",
        )
        .unwrap();
        Command::new(script.to_string_lossy().into_owned(), [value.to_string()])
    } else {
        std::fs::write(&script, "printf '%s\\n' \"$1\"\n").unwrap();
        Command::new(
            "sh",
            [script.to_string_lossy().into_owned(), value.to_string()],
        )
    };
    let out = CommandRunner::run(&commands, &command).await.unwrap();
    assert_eq!(out.exit_code, Some(0), "{out:?}");
    assert_eq!(out.stdout.trim(), value);
}
