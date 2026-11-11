//! A file's extension alone must not override its consumer's configuration dialect.

use super::*;
use deepagent_app_core::settings::VerificationPolicy;
use deepagent_persistence::run_control::{RunActionState, RunControlStore};
use std::time::Duration;

const CALL: &str = "audit-config-write";
const RUN: &str = "audit-config-dialect";
const TARGET: &str = "typescript-project/tsconfig.json";

#[derive(Clone, Copy, Debug)]
enum Dialect {
    StrictJson,
    LineComment,
    BlockComment,
    TrailingComma,
    Malformed,
    ConsumerSyntax,
    Incremental,
}

impl Dialect {
    fn content(self) -> &'static str {
        match self {
            Self::StrictJson => "{\"compilerOptions\":{\"strict\":true},\"files\":[\"index.ts\"]}",
            Self::LineComment => "{\n// configuration explanation\n\"compilerOptions\":{\"strict\":true},\"files\":[\"index.ts\"]}",
            Self::BlockComment => "{/* configuration explanation */\"compilerOptions\":{\"strict\":true},\"files\":[\"index.ts\"]}",
            Self::TrailingComma => "{\"compilerOptions\":{\"strict\":true,},\"files\":[\"index.ts\"],}",
            Self::Malformed => "{\"compilerOptions\":{",
            Self::ConsumerSyntax => "{\"compilerOptions\":{\"maxNodeModuleJsDepth\":0b10,\"baseUrl\":\"\\x2e\"},\"files\":[\"index.ts\"]}",
            Self::Incremental => "{\"compilerOptions\":{\"incremental\":true,\"composite\":true},\"files\":[\"index.ts\"]}",
        }
    }
}

struct ConfigModel {
    content: &'static str,
    target: &'static str,
    tool: &'static str,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for ConfigModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let step = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len() - 1
        };
        let read_first = self.tool != "write_file";
        let events = if read_first && step == 0 {
            response_function_call(
                "h230-read",
                "read_file",
                serde_json::json!({"path":self.target}),
            )
        } else if step == usize::from(read_first) {
            let args = match self.tool {
                "edit_file" => {
                    serde_json::json!({"path":self.target,"old":"H230-before","new":self.content})
                }
                "multi_edit" => {
                    serde_json::json!({"path":self.target,"edits":[{"old":"H230-before","new":self.content}]})
                }
                _ => serde_json::json!({"path":self.target,"content":self.content}),
            };
            response_function_call(CALL, self.tool, args)
        } else {
            response_text("Fixture completed; do not rewrite the file.")
        };
        emit_responses_events(sink, &events)
    }
}

struct Observed {
    ok: bool,
    parse_error: bool,
    action: RunActionState,
    verified: bool,
}

// Default tests use a controlled compiler outcome through the real Kernel and
// command pipeline. Explicit acceptance below uses SystemExecutor and real tsc.
struct CompilerOutcome(Dialect);

#[async_trait]
impl deepagent_builtins::CommandExecutor for CompilerOutcome {
    async fn run(&self, command: &str, _: &str) -> Result<deepagent_builtins::CommandOutcome> {
        assert!(
            command.contains("--noEmit --pretty false --tsBuildInfoFile")
                && command.contains("--project"),
            "{command}"
        );
        let invalid = matches!(self.0, Dialect::Malformed);
        Ok(deepagent_builtins::CommandOutcome {
            exit_code: Some(if invalid { 2 } else { 0 }),
            stdout: if invalid {
                "error TS1005: '}' expected."
            } else {
                ""
            }
            .into(),
            stderr: String::new(),
        })
    }
}

fn has_diagnostic(text: &str) -> bool {
    text.contains("JSON parse error") || text.contains("TypeScript verification failed")
}

async fn observe(dialect: Dialect, policy: VerificationPolicy, check_compiler: bool) -> Observed {
    observe_case(dialect, policy, check_compiler, TARGET, "write_file").await
}

async fn observe_case(
    dialect: Dialect,
    policy: VerificationPolicy,
    check_compiler: bool,
    target: &'static str,
    tool: &'static str,
) -> Observed {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("workspace");
    std::fs::create_dir_all(root.join("typescript-project")).unwrap();
    if check_compiler {
        let compiler = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/node_modules/typescript/lib/tsc.js");
        assert!(
            compiler.is_file(),
            "explicit acceptance requires installed TypeScript"
        );
        let bin = root.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        #[cfg(windows)]
        std::fs::write(
            bin.join("tsc.cmd"),
            format!("@node \"{}\" %*\r\n", compiler.display()),
        )
        .unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let launcher = bin.join("tsc");
            std::fs::write(
                &launcher,
                format!("#!/bin/sh\nexec node '{}' \"$@\"\n", compiler.display()),
            )
            .unwrap();
            std::fs::set_permissions(launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    std::fs::write(
        root.join("typescript-project/index.ts"),
        "export const answer: number = 42;\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(target).parent().unwrap()).unwrap();
    if tool != "write_file" {
        std::fs::write(root.join(target), "H230-before").unwrap();
    }
    if target.ends_with("base.json") {
        std::fs::write(
            root.join("typescript-project/configs/index.ts"),
            "export const answer: number = 42;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("typescript-project/tsconfig.json"),
            "{\"extends\":\"./configs/base\",\"files\":[\"index.ts\"]}",
        )
        .unwrap();
    }
    // No root build system: isolate the post-edit check from the independent
    // completion-plan discovery (and do not depend on a global tsc executable).
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
    settings
        .set_approval_policy(deepagent_app_core::ApprovalPolicy::FullAccess)
        .unwrap();
    let model = Arc::new(ConfigModel {
        content: dialect.content(),
        target,
        tool,
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &root);
    let chat = if check_compiler {
        chat
    } else {
        chat.with_local_command_executor(Arc::new(CompilerOutcome(dialect)))
    };
    tokio::time::timeout(
        Duration::from_secs(10),
        chat.run_in_session(
            "Create the TypeScript configuration with the exact supplied content.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(RUN),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("finite offline tool run must settle")
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join(target)).unwrap(),
        dialect.content()
    );
    let events = chat.run_events(RUN, None).unwrap();
    let completions: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed" && event.data["call_id"] == CALL)
        .collect();
    assert_eq!(completions.len(), 1);
    let ok = completions[0].data["ok"].as_bool().unwrap();
    let output_text = completions[0].data["output"].to_string();
    if check_compiler && !ok {
        eprintln!("real consumer diagnostic (synthetic fixture only): {output_text}");
    }
    let parse_error = has_diagnostic(&output_text);
    if check_compiler
        && matches!(dialect, Dialect::Malformed)
        && policy != VerificationPolicy::Disabled
    {
        assert!(
            output_text.contains("TS1005"),
            "actual syntax diagnostic must not be hidden: {output_text}"
        );
    }
    let verified = output_text.contains("verified:");
    let actions: Vec<_> = RunControlStore::new(&db)
        .list_actions(RUN)
        .unwrap()
        .into_iter()
        .filter(|action| action.call_id == CALL)
        .collect();
    assert_eq!(actions.len(), 1);
    let action = actions[0].state;
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
    {
        let requests = model.requests.lock().unwrap();
        assert_eq!(requests.len(), if tool == "write_file" { 2 } else { 3 });
        let outputs: Vec<_> = requests.last().unwrap()["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
            .collect();
        assert_eq!(outputs.len(), 1);
        let output = outputs[0]["output"].as_str().unwrap();
        let wire: serde_json::Value = serde_json::from_str(output).unwrap();
        assert_eq!(wire["status"], if ok { "ok" } else { "error" });
        assert_eq!(has_diagnostic(output), parse_error);
    }
    if check_compiler {
        assert!(
            !root
                .join("typescript-project/tsconfig.tsbuildinfo")
                .exists(),
            "configuration verification must not build or emit incremental state"
        );
        let compiler = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/node_modules/typescript/lib/tsc.js");
        assert!(
            compiler.is_file(),
            "explicit audit requires installed desktop TypeScript"
        );
        let compiler_output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new("node")
                .arg(compiler)
                .args([
                    if matches!(dialect, Dialect::Incremental) {
                        "--showConfig"
                    } else {
                        "--noEmit"
                    },
                    "--pretty",
                    "false",
                    "--project",
                ])
                .arg(root.join(target))
                .kill_on_drop(true)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            compiler_output.status.success(),
            !matches!(dialect, Dialect::Malformed),
            "real tsc: {}{}",
            String::from_utf8_lossy(&compiler_output.stdout),
            String::from_utf8_lossy(&compiler_output.stderr)
        );
        eprintln!(
            "compiler oracle: dialect={dialect:?}, exit={:?}",
            compiler_output.status.code()
        );
    }
    // Reopen the actual file-backed store, not an in-memory projection. No
    // second run is dispatched and the file's original bytes remain intact.
    drop(chat);
    drop(db);
    let reopened = Database::open(temporary.path().join("state.sqlite")).unwrap();
    let restored = deepagent_persistence::run_store::RunStore::new(&reopened)
        .events_after(RUN, None)
        .unwrap();
    assert_eq!(restored, events);
    let restored_actions = RunControlStore::new(&reopened).list_actions(RUN).unwrap();
    assert_eq!(
        restored_actions
            .iter()
            .find(|item| item.call_id == CALL)
            .unwrap(),
        &actions[0]
    );
    eprintln!("post-edit dialect: dialect={dialect:?}, policy={policy:?}, bytes_preserved=true, tool_ok={ok}, parse_error={parse_error}, action={action:?}, paired_next_wire=true, unique_succeeded=true");
    Observed {
        ok,
        parse_error,
        action,
        verified,
    }
}

#[tokio::test]
async fn h230_all_mutation_entries_preserve_dialects_policy_wire_and_reopen() {
    for tool in ["write_file", "edit_file", "multi_edit"] {
        for policy in [
            VerificationPolicy::Disabled,
            VerificationPolicy::Enabled,
            VerificationPolicy::Strict,
        ] {
            for (target, dialect, valid) in [
                (TARGET, Dialect::LineComment, true),
                (
                    "typescript-project/tsconfig.build.json",
                    Dialect::TrailingComma,
                    true,
                ),
                (
                    "typescript-project/configs/base.json",
                    Dialect::BlockComment,
                    true,
                ),
                (
                    "typescript-project/settings.jsonc",
                    Dialect::TrailingComma,
                    true,
                ),
                (
                    "typescript-project/.vscode/settings.json",
                    Dialect::BlockComment,
                    true,
                ),
                ("typescript-project/package.json", Dialect::StrictJson, true),
                ("typescript-project/data.json", Dialect::LineComment, false),
                (TARGET, Dialect::Malformed, false),
            ] {
                let observed = observe_case(dialect, policy, false, target, tool).await;
                let error = !valid && policy != VerificationPolicy::Disabled;
                assert_eq!(observed.parse_error, error, "{tool} {target} {policy:?}");
                let failed = error && policy == VerificationPolicy::Strict;
                assert_eq!(observed.ok, !failed);
                assert_eq!(
                    observed.action,
                    if failed {
                        RunActionState::Failed
                    } else {
                        RunActionState::Completed
                    }
                );
            }
        }
    }
}

#[tokio::test]
async fn h230_valid_config_dialects_default_regression() {
    for dialect in [
        Dialect::LineComment,
        Dialect::BlockComment,
        Dialect::TrailingComma,
    ] {
        for policy in [VerificationPolicy::Enabled, VerificationPolicy::Strict] {
            let observed = observe(dialect, policy, false).await;
            assert!(observed.ok && !observed.parse_error && observed.verified);
            assert_eq!(observed.action, RunActionState::Completed);
        }
    }
}

#[tokio::test]
#[ignore = "acceptance: installed node + TypeScript tsc on PATH; real consumer via production pipeline"]
async fn h230_real_consumer_all_mutations_and_custom_base_configuration() {
    for tool in ["write_file", "edit_file", "multi_edit"] {
        for (target, dialect) in [
            (TARGET, Dialect::LineComment),
            (
                "typescript-project/tsconfig.build.json",
                Dialect::TrailingComma,
            ),
            (
                "typescript-project/configs/base.json",
                Dialect::BlockComment,
            ),
            (TARGET, Dialect::ConsumerSyntax),
            (TARGET, Dialect::Incremental),
        ] {
            let observed =
                observe_case(dialect, VerificationPolicy::Strict, true, target, tool).await;
            assert!(
                observed.ok && observed.verified && !observed.parse_error,
                "{target} {tool}"
            );
            assert_eq!(observed.action, RunActionState::Completed);
        }
        let invalid = observe_case(
            Dialect::Malformed,
            VerificationPolicy::Strict,
            true,
            TARGET,
            tool,
        )
        .await;
        assert!(!invalid.ok && invalid.parse_error);
        assert_eq!(invalid.action, RunActionState::Failed);
    }
}

#[tokio::test]
async fn strict_json_and_real_invalid_json_controls() {
    for policy in [
        VerificationPolicy::Disabled,
        VerificationPolicy::Enabled,
        VerificationPolicy::Strict,
    ] {
        let observed = observe(Dialect::StrictJson, policy, false).await;
        assert!(observed.ok && !observed.parse_error);
        assert_eq!(observed.action, RunActionState::Completed);
    }
    let invalid = observe(Dialect::Malformed, VerificationPolicy::Strict, false).await;
    assert!(!invalid.ok && invalid.parse_error);
    assert_eq!(invalid.action, RunActionState::Failed);
    let disabled = observe(Dialect::LineComment, VerificationPolicy::Disabled, false).await;
    assert!(disabled.ok && !disabled.parse_error);
}

#[tokio::test]
#[ignore = "audit: requires node + desktop TypeScript; valid tsconfig dialect must not be a tool failure"]
async fn audit_valid_typescript_configs_are_not_reported_as_invalid_json() {
    let mut accepted = true;
    for dialect in [
        Dialect::StrictJson,
        Dialect::LineComment,
        Dialect::BlockComment,
        Dialect::TrailingComma,
        Dialect::Malformed,
    ] {
        for policy in [VerificationPolicy::Enabled, VerificationPolicy::Strict] {
            let observed = observe(dialect, policy, true).await;
            if !matches!(dialect, Dialect::Malformed) {
                accepted &= observed.ok
                    && observed.verified
                    && !observed.parse_error
                    && observed.action == RunActionState::Completed;
            }
        }
    }
    assert!(
        accepted,
        "valid TypeScript configuration was incorrectly reported as broken JSON"
    );
}
