use std::path::Path;
use std::process::Command;

fn repository(root: &Path) {
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap()
        .success());
}

fn changed_files(root: &Path) -> Vec<String> {
    deepagent_workspace::cached_git_context(root)
        .unwrap()
        .changed_files
}

#[test]
fn atomic_workspace_writes_invalidate_the_cached_git_context() {
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path());
    assert!(changed_files(dir.path()).is_empty());
    crate::atomic_file::write_file_atomically(&dir.path().join("state.json"), b"{}").unwrap();
    assert_eq!(changed_files(dir.path()), ["state.json"]);
}

#[test]
fn office_generation_invalidates_the_cached_git_context() {
    let (office, dir) = crate::office_service::tests::office();
    repository(dir.path());
    assert!(changed_files(dir.path()).is_empty());
    office
        .create_docx(
            &crate::office_service::DocSpec::default(),
            &dir.path().join("out.docx").display().to_string(),
        )
        .unwrap();
    assert_eq!(changed_files(dir.path()), ["out.docx"]);
    office
        .create_xlsx(
            &[("Sheet1".into(), vec![vec!["cell".into()]])],
            &dir.path().join("out.xlsx").display().to_string(),
        )
        .unwrap();
    assert_eq!(changed_files(dir.path()), ["out.docx", "out.xlsx"]);
}

#[tokio::test]
async fn workspace_command_hooks_invalidate_the_cached_git_context() {
    use deepagent_hooks::{HookCommandRunner, HookCommandShell};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    repository(dir.path());
    assert!(changed_files(dir.path()).is_empty());
    struct Noop;
    impl deepagent_runtime::RuntimeEventSink for Noop {
        fn emit(&self, _: deepagent_runtime::RuntimeEvent) {}
    }
    let events = Arc::new(Noop);
    let runner = crate::hook_runtime::ObservableHookRunner::new_in_dir(
        events,
        dir.path().to_path_buf(),
        BTreeMap::new(),
    );
    let (shell, command) = if cfg!(windows) {
        (
            HookCommandShell::Powershell,
            "[System.IO.File]::WriteAllText((Join-Path (Get-Location) 'hook.txt'), 'fixture')",
        )
    } else {
        (HookCommandShell::Sh, "printf fixture > hook.txt")
    };
    let result = runner
        .run(
            command,
            "{}",
            shell,
            &BTreeMap::new(),
            std::time::Duration::from_secs(10),
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(changed_files(dir.path()), ["hook.txt"]);
}
