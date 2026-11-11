use super::*;
use crate::settings::VerificationPolicy;
use crate::verification_decorator::VerificationDecorator;
use deepagent_runtime::ToolResultDecorator;
use deepagent_tools::ToolOutput;

#[derive(Debug)]
struct Compiler {
    accepted: bool,
    commands: Mutex<Vec<String>>,
}

#[async_trait]
impl CommandExecutor for Compiler {
    async fn run(&self, command: &str, _: &str) -> deepagent_core::error::Result<CommandOutcome> {
        self.commands.lock().unwrap().push(command.to_string());
        assert!(
            command.contains("--noEmit --pretty false --tsBuildInfoFile")
                && command.contains("--project")
        );
        Ok(CommandOutcome {
            exit_code: Some(if self.accepted { 0 } else { 2 }),
            stdout: if self.accepted {
                ""
            } else {
                "error TS1005: '}' expected."
            }
            .into(),
            stderr: String::new(),
        })
    }
}

async fn check(root: &Path, name: &str, content: &str, accepted: bool) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, content).unwrap();
    let compiler = Arc::new(Compiler {
        accepted,
        commands: Mutex::new(Vec::new()),
    });
    let decorator = VerificationDecorator::with_policy(
        Arc::new(VerificationDispatcher::new(vec![Arc::new(
            JsonVerifier::new(compiler.clone()),
        )])),
        Some(root.to_path_buf()),
        VerificationPolicy::Strict,
    );
    let mut output = ToolOutput::success(serde_json::json!({"path": name}));
    decorator.decorate("write_file", &mut output).await;
    assert_eq!(output.ok, accepted, "{name}: {content:?}: {output:?}");
    let reminder = output.value["_system_reminder"].as_str().unwrap_or("");
    assert!(
        reminder.contains(if accepted { "verified:" } else { "failed:" }),
        "must actually validate, not skip: {name}: {output:?}"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    let needs_compiler = name.starts_with("tsconfig.")
        || name.contains("/tsconfig.")
        || name == "jsconfig.json"
        || name.ends_with("/base.json")
        || name.ends_with("/custom.json");
    assert_eq!(
        compiler.commands.lock().unwrap().len(),
        usize::from(needs_compiler)
    );
}

#[tokio::test]
async fn h230_known_configuration_roles_accept_comments_and_trailing_commas() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "tsconfig.json",
        "project/tsconfig.build.json",
        "jsconfig.json",
        "config.jsonc",
        "example.code-workspace",
        ".vscode/settings.json",
        ".vscode/tasks.json",
        ".vscode/launch.json",
        ".vscode/extensions.json",
    ] {
        for content in [
            "{ // why\n\"a\":1}",
            "{/* why */\"a\":[1,],}",
            "\u{feff}{\"a\":true,}",
            r#"{"url":"https://example.invalid/a/*b*/,","escaped":"\"//",}"#,
        ] {
            check(dir.path(), name, content, true).await;
        }
    }
}

#[tokio::test]
async fn h230_strict_data_and_real_syntax_errors_remain_failures() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "package.json",
        "data.json",
        "tsconfig-data.json",
        ".vscode/data.json",
    ] {
        check(dir.path(), name, "{\"a\":1}", true).await;
        check(
            dir.path(),
            name,
            "{/* why */\"compilerOptions\":{},}",
            false,
        )
        .await;
    }
    for name in ["config.jsonc", "package.json"] {
        for content in [
            "{\"a\":",
            "/* open",
            "{a:1}",
            "{'a':1}",
            "{\"a\":1 \"b\":2}",
            "{\"a\":[1,,2]}",
            "{\"a\":0x12}",
            "{} {}",
            "{\"a\":\"\\x41\"}",
            "{\"a\":\"line\nbreak\"}",
        ] {
            check(dir.path(), name, content, false).await;
        }
    }
}

#[tokio::test]
async fn h230_compiler_errors_missing_marker_and_policy_failure_are_distinct() {
    use super::tests::{fail_exit, ok_exit, MockExecutor};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tsconfig.json");
    std::fs::write(&path, "{\"a\":0b10}").unwrap();
    let cases = [
        (Ok(ok_exit("", "")), "passed"),
        (
            Ok(ok_exit("__DEEPAGENT_TSC_UNAVAILABLE_V1__\n", "")),
            "missing",
        ),
        (
            Ok(fail_exit(1, "", "permission denied by fixture policy")),
            "failed",
        ),
        (Ok(fail_exit(2, "error TS1005", "")), "failed"),
        (Err("fixture executor failure".into()), "failed"),
    ];
    for (result, expected) in cases {
        let verifier = JsonVerifier::new(Arc::new(MockExecutor::default().with("", result)));
        let outcome = verifier.verify(&path).await;
        match expected {
            "passed" => assert_eq!(outcome, VerificationOutcome::Passed),
            "missing" => assert!(
                matches!(outcome, VerificationOutcome::Skipped {reason} if reason.contains("not verified"))
            ),
            _ => assert!(matches!(outcome, VerificationOutcome::Failed { .. })),
        }
    }
}

#[tokio::test]
async fn h230_role_boundary_and_unreadable_dialect_are_not_false_source_diagnostics() {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let target = root.join("base.json");
    std::fs::write(&target, "{/* note */\"a\":1,}").unwrap();
    std::fs::write(
        outer.path().join("tsconfig.json"),
        "{\"extends\":\"./workspace/base\"}",
    )
    .unwrap();
    let verifier = JsonVerifier::default();
    assert!(
        matches!(
            verifier.verify_in_workspace(&target, Some(&root)).await,
            VerificationOutcome::Failed { .. }
        ),
        "outside workspace must not grant a configuration role"
    );
    // A TS parent whose metadata exceeds the JSONC discovery grammar makes
    // the association unknown, not evidence that the edited file is broken.
    std::fs::write(
        root.join("tsconfig.json"),
        "{\"extends\":\"./base\",\"compilerOptions\":{\"maxNodeModuleJsDepth\":0b10}}",
    )
    .unwrap();
    let decorator = VerificationDecorator::with_policy(
        Arc::new(VerificationDispatcher::new(vec![Arc::new(verifier)])),
        Some(root.clone()),
        VerificationPolicy::Strict,
    );
    let mut output = ToolOutput::success(serde_json::json!({"path":"base.json"}));
    decorator.decorate("edit_file", &mut output).await;
    assert!(output.ok);
    let reminder = output.value["_system_reminder"].as_str().unwrap();
    assert!(reminder.contains("configuration verification incomplete:"));
    assert!(!reminder.contains("parse error") && !reminder.contains("verified:"));
    check(
        &root,
        "package.json",
        "{/* note */\"name\":\"fixture\",}",
        false,
    )
    .await;
    std::fs::write(
        root.join("tsconfig.json"),
        "{\"extends\":\"./absent.json\",\"references\":[{\"path\":\"./ref\"}]}",
    )
    .unwrap();
    check(
        &root,
        "absent.json.json",
        "{/* unrelated */\"a\":1,}",
        false,
    )
    .await;
    check(&root, "ref.json", "{/* unrelated */\"a\":1,}", false).await;
}

#[tokio::test]
async fn h230_config_consumer_keeps_dispatcher_timeout() {
    #[derive(Debug)]
    struct Slow;
    #[async_trait]
    impl CommandExecutor for Slow {
        async fn run(&self, _: &str, _: &str) -> deepagent_core::error::Result<CommandOutcome> {
            std::future::pending().await
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tsconfig.json");
    std::fs::write(&path, "{}").unwrap();
    let dispatcher = VerificationDispatcher::new(vec![Arc::new(JsonVerifier::new(Arc::new(Slow)))])
        .with_timeout(Duration::from_millis(10));
    assert_eq!(
        dispatcher
            .verify_file_in_workspace(&path, Some(dir.path()))
            .await,
        VerificationOutcome::TimedOut
    );
}

#[tokio::test]
#[ignore = "acceptance: real platform shell in isolated child PATH without TypeScript"]
async fn h230_real_missing_tsc_reports_unverified_not_syntax_failure() {
    const TEST: &str = "verification_dispatcher::json_dialect_tests::h230_real_missing_tsc_reports_unverified_not_syntax_failure";
    if std::env::var_os("H230_NO_TSC_CHILD").is_none() {
        let paths = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|dir| {
                !["tsc", "tsc.cmd", "tsc.exe", "tsc.ps1"]
                    .iter()
                    .any(|name| dir.join(name).is_file())
            })
            .collect::<Vec<_>>();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([TEST, "--ignored", "--exact", "--nocapture"])
            .env("H230_NO_TSC_CHILD", "1")
            .env("PATH", std::env::join_paths(paths).unwrap())
            .kill_on_drop(true)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tsconfig.json");
    std::fs::write(&path, "{ // comment\n\"compilerOptions\":{},}").unwrap();
    let outcome = JsonVerifier::default().verify(&path).await;
    assert!(
        matches!(outcome, VerificationOutcome::Skipped { reason } if reason.starts_with("tsc toolchain not found") && reason.contains("未完成验证"))
    );
}

#[tokio::test]
async fn h230_local_config_references_propagate_roles_not_object_shape() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("configs")).unwrap();
    std::fs::write(dir.path().join("tsconfig.json"),
        "{/* root */\"extends\":[\"./configs/middle\"],\"references\":[{\"path\":\"./configs/custom.json\"}]}").unwrap();
    std::fs::write(
        dir.path().join("configs/middle.json"),
        "{\"extends\":\"./base.json\",}",
    )
    .unwrap();
    check(
        dir.path(),
        "configs/base.json",
        "{/* base */\"compilerOptions\":{},}",
        true,
    )
    .await;
    check(
        dir.path(),
        "configs/custom.json",
        "{\"compilerOptions\":{},}",
        true,
    )
    .await;
    // Do not infer a TypeScript role just because an object has compilerOptions.
    check(
        dir.path(),
        "configs/unrelated.json",
        "{\"compilerOptions\":{},}",
        false,
    )
    .await;
    // Cycles must settle; they do not grant a role to an unrelated data file.
    std::fs::write(
        dir.path().join("configs/base.json"),
        "{\"extends\":\"./middle.json\"}",
    )
    .unwrap();
    check(dir.path(), "configs/unrelated.json", "{\"a\":1,}", false).await;
}

#[tokio::test]
async fn h230_package_specifier_does_not_grant_a_local_configuration_role() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("tsconfig.json"),
        "{\"extends\":\".package\"}",
    )
    .unwrap();
    // TypeScript treats this as a package specifier, not './.package'. A data
    // file with a similar local name must not trigger a compiler invocation.
    check(
        dir.path(),
        ".package.json",
        "{/* ordinary data */\"a\":1,}",
        false,
    )
    .await;
}

#[tokio::test]
async fn h230_relative_path_context_preserves_the_existing_direct_verifier_contract() {
    // Do not change process cwd: other tests may be running concurrently.
    let cwd = std::env::current_dir().unwrap();
    let dir = tempfile::Builder::new()
        .prefix("h230-relative-")
        .tempdir_in(&cwd)
        .unwrap();
    let relative_root = dir.path().strip_prefix(&cwd).unwrap();
    let path = dir.path().join("tsconfig.json");
    std::fs::write(&path, "{\"compilerOptions\":{},}").unwrap();
    let bin = dir.path().join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let executable = bin.join(if cfg!(windows) { "tsc.cmd" } else { "tsc" });
    std::fs::write(
        &executable,
        "fixture: never executed by the recording executor",
    )
    .unwrap();
    let compiler = Arc::new(Compiler {
        accepted: true,
        commands: Mutex::new(Vec::new()),
    });
    let verifier = JsonVerifier::new(compiler.clone());
    assert_eq!(
        verifier
            .verify_in_workspace(&relative_root.join("tsconfig.json"), Some(relative_root))
            .await,
        VerificationOutcome::Passed
    );
    let commands = compiler.commands.lock().unwrap();
    assert_eq!(commands.len(), 1);
    // Windows accepts both separators. Compare the complete literal identity,
    // not PathBuf's choice of separator when two components were joined.
    let comparable = |value: &str| {
        if cfg!(windows) {
            value.replace('\\', "/")
        } else {
            value.to_owned()
        }
    };
    assert!(
        comparable(&commands[0]).contains(&comparable(
            &crate::verification_commands::quote_argument(&path.to_string_lossy())
        )),
        "project argument must be absolute before changing compiler cwd: {}",
        commands[0]
    );
    assert!(
        comparable(&commands[0]).contains(&comparable(
            &crate::verification_commands::quote_argument(&executable.to_string_lossy())
        )),
        "relative workspace root must still discover its local compiler: {}",
        commands[0]
    );
}
