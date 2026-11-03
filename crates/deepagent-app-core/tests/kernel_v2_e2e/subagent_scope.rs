//! A model-visible child handle is an identity, not cross-conversation authority.
//! Only model I/O is replaced; task execution, ownership and persistence are real.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_models::ModelRole;
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::{SubagentRunRecord, SubagentRunStore};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::Notify;

const MODEL: &str = "deepseek-flash";
const OWNER: &str = "AUDIT_CHILD_SCOPE_OWNER";
const CHILD: &str = "AUDIT_CHILD_SCOPE_WORKER";
const CONTROL: &str = "AUDIT_CHILD_SCOPE_CONTROLLER";
const OWNER_RUN: &str = "audit-child-scope-owner";
const CONTROL_RUN: &str = "audit-child-scope-controller";

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct ScopeModel {
    child_id: Mutex<String>,
    owner: AtomicUsize,
    controller: Mutex<Vec<Value>>,
    entered: Notify,
    release: Notify,
    waiting: AtomicUsize,
    answered: AtomicUsize,
    unknown: bool,
    cancel: bool,
}

#[async_trait]
impl HttpTransport for ScopeModel {
    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        tokio::select! {
            result = self.stream(request, sink) => result,
            _ = deepagent_runtime::cancellation::wait_for_legacy_flag(cancel) => {
                Err(CoreError::other("synthetic stream cancelled"))
            }
        }
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let input = body["input"].to_string();
        let item = if input.contains(CHILD) && !input.contains(OWNER) {
            self.waiting.fetch_add(1, Ordering::AcqRel);
            let _waiting = Waiting(&self.waiting);
            self.entered.notify_one();
            self.release.notified().await;
            self.answered.fetch_add(1, Ordering::AcqRel);
            text("scope-worker-result", "Synthetic child completed normally.")
        } else if input.contains(CONTROL) {
            let index = {
                let mut requests = self.controller.lock().unwrap();
                requests.push(body);
                requests.len() - 1
            };
            let id = if self.unknown {
                "sub_synthetic_absent".to_string()
            } else {
                self.child_id.lock().unwrap().clone()
            };
            let operation = match index {
                0 | 4 => Some("status"),
                1 => Some("resume"),
                2 => Some("cleanup"),
                3 if self.cancel => Some("cancel"),
                3 => Some("status"),
                5 => None,
                _ => panic!("unexpected controller step {index}"),
            };
            // Allow a cooperative stop to settle without ever releasing model I/O.
            if index == 4 && self.cancel {
                let _ = tokio::time::timeout(Duration::from_millis(400), async {
                    while self.waiting.load(Ordering::Acquire) != 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await;
            }
            match operation {
                Some(operation) => call(
                    &format!("scope-control-{index}"),
                    "task",
                    json!({"operation":operation,"subagent_id":id,"prompt":"Synthetic continuation"}),
                ),
                None => text("scope-control-result", "Synthetic controller completed."),
            }
        } else {
            assert!(input.contains(OWNER));
            match self.owner.fetch_add(1, Ordering::AcqRel) {
                0 => call(
                    "scope-launch",
                    "task",
                    json!({"description":"scope worker","prompt":CHILD,"background":true}),
                ),
                1 => text("scope-owner-result", "Synthetic owner completed."),
                index => panic!("unexpected owner step {index}"),
            }
        };
        emit_responses_events(
            sink,
            &completed(
                item["id"].as_str().unwrap(),
                MODEL,
                item.clone(),
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

async fn settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, MODEL).unwrap();
    }
    settings
        .set_active_permission_preset(PermissionPreset::Default)
        .unwrap();
    settings
}

async fn run(chat: &ChatService, prompt: &str, session: Option<&str>, run: &str) -> String {
    tokio::time::timeout(
        Duration::from_secs(8),
        chat.run_in_session(
            prompt,
            session,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(run),
            |_| {},
            |_| {},
        ),
    )
    .await
    .expect("scope workflow must settle")
    .unwrap()
}

fn output(body: &Value, index: usize) -> Value {
    let id = format!("scope-control-{index}");
    let input = body["input"].as_array().unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call" && item["call_id"] == id)
            .count(),
        1
    );
    let outputs: Vec<_> = input
        .iter()
        .filter(|item| item["type"] == "function_call_output" && item["call_id"] == id)
        .collect();
    assert_eq!(outputs.len(), 1);
    serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap()
}

async fn terminal(db: &Database, id: &str) -> SubagentRunRecord {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let record = SubagentRunStore::new(db).get(id).unwrap().unwrap();
            if record.finished_at.is_some() {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background child must be cleaned up before assertions")
}

#[derive(Debug)]
struct Observation {
    accepted: bool,
    state_before_release: String,
    waiting_before_release: usize,
    answered: usize,
    final_state: String,
}

async fn observe(same_session: bool, cancel: bool, unknown: bool) -> Observation {
    observe_case(same_session, cancel, unknown, Scenario::Standard).await
}

#[derive(Clone, Copy)]
enum Scenario {
    Standard,
    ForeignWorkspace,
    HostUserStop,
}

async fn observe_case(
    same_session: bool,
    cancel: bool,
    unknown: bool,
    scenario: Scenario,
) -> Observation {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = Arc::new(Database::open(temporary.path().join("state.sqlite")).unwrap());
    let model = Arc::new(ScopeModel {
        cancel,
        unknown,
        ..Default::default()
    });
    let chat = ChatService::new(
        db.clone(),
        settings(db.clone()).await,
        model.clone(),
        &workspace,
    );
    let owner_session = run(&chat, OWNER, None, OWNER_RUN).await;
    tokio::time::timeout(Duration::from_secs(2), model.entered.notified())
        .await
        .unwrap();
    let child = chat.subagent_runs(OWNER_RUN).unwrap().remove(0);
    assert_eq!(child.state, "running");
    *model.child_id.lock().unwrap() = child.id.clone();
    let controller = if matches!(scenario, Scenario::ForeignWorkspace) {
        let other = temporary.path().join("other-project");
        std::fs::create_dir(&other).unwrap();
        chat.for_workspace(other)
    } else {
        chat.clone()
    };
    let control_session = run(
        &controller,
        CONTROL,
        same_session.then_some(owner_session.as_str()),
        CONTROL_RUN,
    )
    .await;
    assert_eq!(owner_session == control_session, same_session);
    let host_acks = if matches!(scenario, Scenario::HostUserStop) {
        // Model authority is session-scoped; an explicit host-user request is
        // still allowed to stop work in another conversation of this host.
        let first = controller.cancel_subagent(&child.id);
        let repeated = controller.cancel_subagent(&child.id);
        if first {
            terminal(&db, &child.id).await;
        }
        Some((first, repeated))
    } else {
        None
    };
    let before = SubagentRunStore::new(&db).get(&child.id).unwrap().unwrap();
    let waiting_before_release = model.waiting.load(Ordering::Acquire);
    // Always finish the real child, including the future-fixed/normal path.
    model.release.notify_one();
    let final_record = terminal(&db, &child.id).await;
    if let Some((first, repeated)) = host_acks {
        assert!(first);
        assert!(!repeated);
    }
    let requests = model.controller.lock().unwrap();
    assert_eq!(requests.len(), 6);
    for index in 0..3 {
        let result = output(&requests[index + 1], index);
        if unknown {
            assert_eq!(result["status"], "error");
            assert!(result.to_string().contains("not found"));
        } else if !same_session {
            assert_eq!(result["status"], "error");
            assert!(result.to_string().contains("another conversation"));
        } else if index == 0 {
            assert_eq!(result["status"], "ok");
            assert!(result.to_string().contains("running"));
        } else {
            assert_eq!(result["status"], "error"); // Running children cannot resume/cleanup.
        }
    }
    let cancel_output = output(&requests[4], 3);
    let accepted = cancel_output["result"]["cancel_accepted"]
        .as_bool()
        .unwrap_or(false);
    if cancel && (same_session || unknown) {
        assert_eq!(cancel_output["status"], "ok");
        assert_eq!(cancel_output["result"]["cancel_accepted"], !unknown);
    } else if cancel {
        assert_eq!(cancel_output["status"], "error");
        assert!(cancel_output.to_string().contains("another conversation"));
        assert!(cancel_output["result"].is_null());
    }
    let last_status = output(&requests[5], 4);
    if !same_session || unknown {
        assert_eq!(last_status["status"], "error");
    } else {
        assert_eq!(last_status["status"], "ok");
        assert_eq!(last_status["result"]["state"], before.state);
    }
    for run_id in [OWNER_RUN, CONTROL_RUN] {
        assert_eq!(
            RunStore::new(&db)
                .get(run_id)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("succeeded")
        );
        assert_eq!(
            chat.run_events(run_id, None)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
    }
    assert_eq!(final_record.origin_parent_run_id, OWNER_RUN);
    assert!(chat.subagent_runs(CONTROL_RUN).unwrap().is_empty());
    let events = chat.run_events(OWNER_RUN, None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.event_type.as_str(),
                "subagent_cancelled" | "subagent_completed"
            ))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "subagent_notification")
            .count(),
        1
    );
    let transcript: Value = serde_json::from_slice(
        &std::fs::read(final_record.transcript_path.as_ref().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(transcript["state"], final_record.state);
    let observed = Observation {
        accepted,
        state_before_release: before.state,
        waiting_before_release,
        answered: model.answered.load(Ordering::Acquire),
        final_state: final_record.state,
    };
    eprintln!("Subagent scope audit: same_session={same_session} cancel={cancel} unknown={unknown} {observed:?}");
    observed
}

#[tokio::test]
async fn subagent_scope_same_conversation_can_stop_child_from_later_turn() {
    let observed = observe(true, true, false).await;
    assert!(observed.accepted);
    assert_eq!(observed.state_before_release, "cancelled");
    assert_eq!(observed.waiting_before_release, 0);
    assert_eq!(observed.answered, 0);
    assert_eq!(observed.final_state, "cancelled");
}

#[tokio::test]
async fn subagent_scope_cross_conversation_read_resume_cleanup_are_rejected() {
    let observed = observe(false, false, false).await;
    assert!(!observed.accepted);
    assert_eq!(observed.state_before_release, "running");
    assert_eq!(observed.waiting_before_release, 1);
    assert_eq!(observed.answered, 1);
    assert_eq!(observed.final_state, "succeeded");
}

#[tokio::test]
async fn subagent_scope_unknown_cancel_does_not_stop_existing_child() {
    let observed = observe(false, true, true).await;
    assert!(!observed.accepted);
    assert_eq!(observed.state_before_release, "running");
    assert_eq!(observed.waiting_before_release, 1);
    assert_eq!(observed.answered, 1);
    assert_eq!(observed.final_state, "succeeded");
}

#[tokio::test]
async fn subagent_scope_foreign_conversation_must_not_stop_child() {
    let observed = observe(false, true, false).await;
    assert!(
        !observed.accepted
            && observed.state_before_release == "running"
            && observed.waiting_before_release == 1
            && observed.answered == 1
            && observed.final_state == "succeeded",
        "foreign model-visible handle must not grant cancellation authority: {observed:?}"
    );
}

#[tokio::test]
async fn subagent_scope_another_workspace_sharing_the_host_cannot_cancel_child() {
    let observed = observe_case(false, true, false, Scenario::ForeignWorkspace).await;
    assert!(!observed.accepted);
    assert_eq!(observed.state_before_release, "running");
    assert_eq!(observed.waiting_before_release, 1);
    assert_eq!(observed.answered, 1);
    assert_eq!(observed.final_state, "succeeded");
}

#[tokio::test]
async fn subagent_scope_host_user_can_still_stop_child_after_foreign_model_rejection() {
    let observed = observe_case(false, true, false, Scenario::HostUserStop).await;
    assert!(!observed.accepted);
    assert_eq!(observed.state_before_release, "cancelled");
    assert_eq!(observed.waiting_before_release, 0);
    assert_eq!(observed.answered, 0);
    assert_eq!(observed.final_state, "cancelled");
}
