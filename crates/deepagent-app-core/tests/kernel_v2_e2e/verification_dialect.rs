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
}

impl Dialect {
    fn content(self) -> &'static str {
        match self {
            Self::StrictJson => "{\"compilerOptions\":{\"strict\":true},\"files\":[\"index.ts\"]}",
            Self::LineComment => "{\n// configuration explanation\n\"compilerOptions\":{\"strict\":true},\"files\":[\"index.ts\"]}",
            Self::BlockComment => "{/* configuration explanation */\"compilerOptions\":{\"strict\":true},\"files\":[\"index.ts\"]}",
            Self::TrailingComma => "{\"compilerOptions\":{\"strict\":true,},\"files\":[\"index.ts\"],}",
            Self::Malformed => "{\"compilerOptions\":{",
        }
    }
}

struct ConfigModel {
    content: &'static str,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for ConfigModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let first = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body).unwrap());
            requests.len() == 1
        };
        let events = if first {
            response_function_call(
                CALL,
                "write_file",
                serde_json::json!({"path":TARGET,"content":self.content}),
            )
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
}

async fn observe(dialect: Dialect, policy: VerificationPolicy, check_compiler: bool) -> Observed {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("workspace");
    std::fs::create_dir_all(root.join("typescript-project")).unwrap();
    std::fs::write(
        root.join("typescript-project/index.ts"),
        "export const answer: number = 42;\n",
    )
    .unwrap();
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
    let model = Arc::new(ConfigModel {
        content: dialect.content(),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &root);
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
        std::fs::read_to_string(root.join(TARGET)).unwrap(),
        dialect.content()
    );
    let events = chat.run_events(RUN, None).unwrap();
    let completions: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_completed" && event.data["call_id"] == CALL)
        .collect();
    assert_eq!(completions.len(), 1);
    let ok = completions[0].data["ok"].as_bool().unwrap();
    let parse_error = completions[0].data["output"]
        .to_string()
        .contains("JSON parse error");
    let actions = RunControlStore::new(&db).list_actions(RUN).unwrap();
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
        assert_eq!(requests.len(), 2);
        let outputs: Vec<_> = requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "function_call_output" && item["call_id"] == CALL)
            .collect();
        assert_eq!(outputs.len(), 1);
        let output = outputs[0]["output"].as_str().unwrap();
        let wire: serde_json::Value = serde_json::from_str(output).unwrap();
        assert_eq!(wire["status"], if ok { "ok" } else { "error" });
        assert_eq!(output.contains("JSON parse error"), parse_error);
    }
    if check_compiler {
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
                .args(["--noEmit", "--pretty", "false", "--project"])
                .arg(root.join(TARGET))
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
    eprintln!("post-edit dialect: dialect={dialect:?}, policy={policy:?}, bytes_preserved=true, tool_ok={ok}, parse_error={parse_error}, action={action:?}, paired_next_wire=true, unique_succeeded=true");
    Observed {
        ok,
        parse_error,
        action,
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
            let observed = observe(
                dialect,
                policy,
                matches!(policy, VerificationPolicy::Strict),
            )
            .await;
            if !matches!(dialect, Dialect::Malformed) {
                accepted &= observed.ok
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
