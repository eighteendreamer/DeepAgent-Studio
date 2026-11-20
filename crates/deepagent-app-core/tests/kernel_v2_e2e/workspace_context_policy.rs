//! Compare tool Read denial with automatically collected workspace context.

use super::*;
use deepagent_hooks::PermissionRules;
use deepagent_persistence::run_store::RunStore;
use serde_json::{json, Value};

const CANARY: &str = "audit-workspace-context-canary-8d2e";
const READ_ID: &str = "audit-context-read";
const RUN_ID: &str = "audit-workspace-context-policy";

#[derive(Clone, Copy, Debug)]
enum FileCase {
    Plain,
    Readme,
    Package,
}

impl FileCase {
    fn fixture(self) -> (&'static str, String) {
        match self {
            Self::Plain => ("notes.txt", format!("{CANARY}\n")),
            Self::Readme => ("README.md", format!("# Synthetic project\n{CANARY}\n")),
            Self::Package => ("package.json", json!({"name":CANARY}).to_string()),
        }
    }
}

struct ContextPolicyModel {
    path: String,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for ContextPolicyModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len() - 1
        };
        let item = match index {
            0 => responses_fixture::call(READ_ID, "read_file", json!({"path":self.path})),
            1 => responses_fixture::text("audit-context-final", "Offline context audit finished."),
            _ => panic!("unexpected model request {index}"),
        };
        emit_responses_events(
            sink,
            &responses_fixture::completed(
                &format!("audit-context-response-{index}"),
                "deepseek-flash",
                item,
                json!({"input_tokens":24,"output_tokens":8,"total_tokens":32}),
            ),
        )
    }
}

#[derive(Debug)]
struct Observation {
    first_instructions_have_canary: bool,
    first_input_has_canary: bool,
    next_instructions_have_canary: bool,
    tool_result_has_canary: bool,
    blocked_reads: usize,
    completed_reads: usize,
    approvals: usize,
}

async fn observe(case: FileCase, deny_read: bool) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let (path, content) = case.fixture();
    std::fs::write(workspace.join(path), &content).unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    settings
        .set_permission_rules(PermissionRules::new(
            [],
            [],
            if deny_read {
                vec!["Read".to_string()]
            } else {
                Vec::new()
            },
        ))
        .unwrap();
    let model = Arc::new(ContextPolicyModel {
        path: path.to_string(),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), &workspace);
    chat.run_in_session(
        "Try reading the fixture, then finish. The file contents are not provided in this input.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(RUN_ID),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let events = chat.run_events(RUN_ID, None).unwrap();
    assert_eq!(
        RunStore::new(&db)
            .get(RUN_ID)
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "one tool call and its real continuation");
    let outputs: Vec<_> = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == READ_ID)
        .collect();
    assert_eq!(
        outputs.len(),
        1,
        "the continuation carries one actual tool result"
    );
    let outcome = Observation {
        first_instructions_have_canary: super::cache_prefix::wire_system_text(&requests[0])
            .contains(CANARY),
        first_input_has_canary: requests[0]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["role"] != "system")
            .any(|item| item.to_string().contains(CANARY)),
        next_instructions_have_canary: super::cache_prefix::wire_system_text(&requests[1])
            .contains(CANARY),
        tool_result_has_canary: outputs[0]["output"].as_str().unwrap().contains(CANARY),
        blocked_reads: events
            .iter()
            .filter(|event| {
                event.event_type == "tool_blocked"
                    && event.data["name"] == "read_file"
                    && event.data["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("permission rule"))
            })
            .count(),
        completed_reads: events
            .iter()
            .filter(|event| {
                event.event_type == "tool_completed"
                    && event.data["name"] == "read_file"
                    && event.data["ok"] == true
            })
            .count(),
        approvals: events
            .iter()
            .filter(|event| event.event_type == "approval_requested")
            .count(),
    };
    assert_eq!(
        std::fs::read_to_string(workspace.join(path)).unwrap(),
        content
    );
    assert!(
        !outcome.first_input_has_canary,
        "user/history did not supply the canary"
    );
    assert_eq!(outcome.blocked_reads, usize::from(deny_read));
    assert_eq!(outcome.completed_reads, usize::from(!deny_read));
    assert_eq!(outcome.tool_result_has_canary, !deny_read);
    assert_eq!(
        outcome.approvals, 0,
        "deny/ordinary read should not wait for approval"
    );
    outcome
}

#[tokio::test]
async fn workspace_context_policy_allowed_and_nonpreloaded_controls() {
    for (case, deny_read) in [
        (FileCase::Plain, false),
        (FileCase::Plain, true),
        (FileCase::Readme, false),
        (FileCase::Package, false),
    ] {
        let outcome = observe(case, deny_read).await;
        eprintln!("Workspace context control {case:?}/deny={deny_read}: {outcome:?}");
        let preloaded = !matches!(case, FileCase::Plain);
        assert_eq!(outcome.first_instructions_have_canary, preloaded);
        assert_eq!(outcome.next_instructions_have_canary, preloaded);
    }
}

#[tokio::test]
async fn audit_automatic_workspace_context_respects_effective_read_denial() {
    let mut leaked = Vec::new();
    for case in [FileCase::Readme, FileCase::Package] {
        let outcome = observe(case, true).await;
        eprintln!("Workspace context audit {case:?}/deny=true: {outcome:?}");
        if outcome.first_instructions_have_canary || outcome.next_instructions_have_canary {
            leaked.push(format!("{case:?}"));
        }
    }
    assert!(
        leaked.is_empty(),
        "automatic context disclosed denied file contents: {leaked:?}"
    );
}
