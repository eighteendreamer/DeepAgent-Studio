//! Exercise missing optional lint with real shell status, not a mocked spawn error.

use super::*;
use deepagent_app_core::settings::VerificationPolicy;
use deepagent_builtins::{CommandExecutor, SystemExecutor};
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use std::time::Duration;

const CHILD: &str = "DEEPAGENT_AUDIT_TOOLCHAIN_CHILD";
const CALL: &str = "audit-python-write";
const SOURCE: &str = "answer = 42\n";

struct Capture {
    replay: ReplayTransport,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for Capture {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        self.replay.stream(request, sink).await
    }
}

async fn chat_write(policy: VerificationPolicy) -> bool {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    settings.set_verification_policy(policy).unwrap();
    let transport = Arc::new(Capture {
        replay: ReplayTransport::new([
            response_function_call(
                CALL,
                "write_file",
                serde_json::json!({"path":"answer.py","content":SOURCE}),
            ),
            response_text("Fixture completed without rewriting the file."),
        ]),
        requests: Mutex::new(Vec::new()),
    });
    // The production decorator constructs its own standard SystemExecutor.
    // No injected fake verifier/executor or completion build plan is involved.
    let chat = ChatService::new(db.clone(), settings, transport.clone(), &root);
    tokio::time::timeout(
        Duration::from_secs(12),
        chat.run_in_session(
            "Create answer.py with the exact supplied source.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-missing-ruff"),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("finite offline run must settle")
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("answer.py")).unwrap(),
        SOURCE
    );
    let events = chat.run_events("audit-missing-ruff", None).unwrap();
    let completed: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed" && event.data["call_id"] == CALL)
        .collect();
    assert_eq!(completed.len(), 1);
    let ok = completed[0].data["ok"].as_bool().unwrap();
    let output = completed[0].data["output"].to_string();
    let false_failure = output.contains("failed:") && output.contains("ruff");
    let actions = RunControlStore::new(&db)
        .list_actions("audit-missing-ruff")
        .unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0].state,
        if ok {
            RunActionState::Completed
        } else {
            RunActionState::Failed
        }
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(events
        .iter()
        .any(|event| event.event_type == "run_terminal" && event.data["kind"] == "succeeded"));
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let results: Vec<_> = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
        .collect();
    assert_eq!(results.len(), 1);
    let wire = results[0]["output"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(wire).unwrap();
    assert_eq!(parsed["status"], if ok { "ok" } else { "error" });
    assert_eq!(
        wire.contains("ruff"),
        !matches!(policy, VerificationPolicy::Disabled)
    );
    eprintln!("optional ruff chat: policy={policy:?}, source_preserved=true, ok={ok}, false_failure={false_failure}, action={:?}, next_wire_paired=true, unique_succeeded=true", actions[0].state);
    ok && !false_failure
}

#[tokio::test]
#[ignore = "fixture child; run audit_missing_optional_ruff_is_not_a_source_failure"]
async fn optional_ruff_child() {
    assert!(
        std::env::var_os(CHILD).is_some(),
        "fixture child must be launched by audit_missing_optional_ruff_is_not_a_source_failure"
    );
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source.py");
    std::fs::write(&source, SOURCE).unwrap();
    let cwd = temporary.path().to_str().unwrap();
    let compile = SystemExecutor
        .run(
            &format!("python -m py_compile \"{}\"", source.display()),
            cwd,
        )
        .await
        .expect("the real shell must spawn");
    assert_eq!(compile.exit_code, Some(0), "{compile:?}");
    assert!(temporary.path().join("__pycache__").is_dir());
    let missing = SystemExecutor
        .run("ruff --version", cwd)
        .await
        .expect("missing nested command is not a shell spawn error");
    assert_ne!(missing.exit_code, Some(0));
    assert!(missing.stderr.contains("ruff"));
    eprintln!("real executor: python_compile_exit=0, pycache_created=true, missing_ruff_is_ok_outcome=true, missing_ruff_exit={:?}", missing.exit_code);
    let mut accepted = true;
    for policy in [
        VerificationPolicy::Disabled,
        VerificationPolicy::Enabled,
        VerificationPolicy::Strict,
    ] {
        let honest = chat_write(policy).await;
        if matches!(policy, VerificationPolicy::Disabled) {
            assert!(honest, "disabled verification is the normal control");
        }
        accepted &= honest;
    }
    // A real syntax error remains a failure even without the optional linter.
    std::fs::write(&source, "def broken(:\n").unwrap();
    let bad = SystemExecutor
        .run(
            &format!("python -m py_compile \"{}\"", source.display()),
            cwd,
        )
        .await
        .unwrap();
    assert_ne!(bad.exit_code, Some(0));
    assert!(bad.stderr.contains("SyntaxError"));
    eprintln!("real invalid-source control: nonzero_exit=true, SyntaxError=true");
    assert!(
        accepted,
        "missing optional ruff was misreported as a source-code verification failure"
    );
}

#[tokio::test]
#[ignore = "audit: requires DEEPAGENT_AUDIT_PYTHON pointing to an installed Python executable"]
async fn audit_missing_optional_ruff_is_not_a_source_failure() {
    let python = PathBuf::from(
        std::env::var_os("DEEPAGENT_AUDIT_PYTHON")
            .expect("set DEEPAGENT_AUDIT_PYTHON to a verified installed Python executable"),
    )
    .canonicalize()
    .unwrap();
    assert!(python.is_file());
    let original_path = std::env::var_os("PATH").unwrap();
    let shell_dir = std::env::split_paths(&original_path)
        .find(|dir| dir.join("pwsh.exe").is_file())
        .expect("explicit Windows audit requires installed PowerShell 7");
    let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    // Only this child inherits the restricted PATH. No global env mutation,
    // registry edit, executable replacement, or dependency installation.
    let restricted_path = std::env::join_paths([
        python.parent().unwrap().to_path_buf(),
        shell_dir,
        windows.join("System32"),
    ])
    .unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(45),
        tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "verification_toolchain::optional_ruff_child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("PATH", restricted_path)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("fixture child must settle")
    .unwrap();
    eprintln!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "isolated production-path invariant failed"
    );
}
