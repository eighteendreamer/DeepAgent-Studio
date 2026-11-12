//! Periodic todo reminders must derive their counters from the durable session
//! history, not from the lifetime of one freshly assembled ModelAgent.

use super::*;
use deepagent_models::ModelRole;
use std::path::Path;

const REMINDER_MARKER: &str = "todo-tracking tool hasn't been used in a while";

struct RecordingTodoModel {
    requests: Mutex<Vec<serde_json::Value>>,
    todo_write_at: Option<usize>,
}

impl RecordingTodoModel {
    fn new(todo_write_at: Option<usize>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            todo_write_at,
        }
    }
}

#[async_trait]
impl HttpTransport for RecordingTodoModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let request_number = {
            let mut requests = self
                .requests
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            requests.push(serde_json::from_str(&request.body)?);
            requests.len()
        };
        let events = if self.todo_write_at == Some(request_number) {
            response_function_call(
                "audit-todo-write",
                "todo_write",
                serde_json::json!({
                    "todos": [{
                        "content": "Continue the synthetic audit",
                        "active_form": "Continuing the synthetic audit",
                        "status": "in_progress"
                    }]
                }),
            )
        } else {
            response_text("offline todo reminder audit completed")
        };
        emit_responses_events(sink, &events)
    }
}

async fn audit_chat(db_path: &Path, root: &Path, model: Arc<RecordingTodoModel>) -> ChatService {
    let db = Arc::new(Database::open(db_path).unwrap());
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
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    ChatService::new(db, settings, model, root)
}

fn persisted_progress(db_path: &Path, session_id: &str) -> (usize, (u64, u64)) {
    let db = Database::open(db_path).unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(session_id.parse().unwrap())
        .unwrap();
    let progress: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            deepagent_core::event::EventPayload::TodoReminderProgress {
                version: 1,
                turns_since_todo_write,
                turns_since_todo_reminder,
            } => Some((*turns_since_todo_write, *turns_since_todo_reminder)),
            _ => None,
        })
        .collect();
    (
        progress.len(),
        *progress.last().expect("checkpoint must persist"),
    )
}

#[tokio::test]
async fn audit_todo_reminder_counts_survive_independent_session_turns() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db_path = root.path().join("audit.sqlite");
    let model = Arc::new(RecordingTodoModel::new(None));
    let mut session_id = None;
    for start in [0, 5, 10] {
        let end = (start + 5).min(11);
        let chat = audit_chat(&db_path, &workspace, model.clone()).await;
        for turn in start..end {
            let run_id = format!("audit-todo-reminder-turn-{turn}");
            let next_session = chat
                .run_in_session(
                    &format!("Continue the audit turn {turn}; reply briefly."),
                    session_id.as_deref(),
                    None,
                    None,
                    Vec::new(),
                    None,
                    false,
                    Some(&run_id),
                    |_| {},
                    |_| {},
                )
                .await
                .unwrap();
            session_id = Some(next_session);
        }
    }

    let requests = model
        .requests
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let reminder_requests: Vec<_> = requests
        .iter()
        .enumerate()
        .filter(|(_, request)| request.to_string().contains(REMINDER_MARKER))
        .map(|(index, _)| index + 1)
        .collect();
    eprintln!(
        "todo reminder lifecycle audit: independent_turns={} requests={} reminder_requests={reminder_requests:?}",
        11,
        requests.len(),
    );
    assert_eq!(requests.len(), 11);
    assert_eq!(
        reminder_requests,
        vec![10],
        "the reminder cadence must survive independent ModelAgent construction and database reopen"
    );
    drop(requests);
    assert_eq!(
        persisted_progress(&db_path, session_id.as_deref().unwrap()),
        (11, (11, 1))
    );
}

#[tokio::test]
async fn todo_write_resets_reminder_progress_across_independent_session_turns() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db_path = root.path().join("audit.sqlite");
    let model = Arc::new(RecordingTodoModel::new(Some(10)));
    let mut session_id = None;
    for start in [0, 5, 10, 15] {
        let end = (start + 5).min(19);
        let chat = audit_chat(&db_path, &workspace, model.clone()).await;
        for turn in start..end {
            let run_id = format!("audit-todo-reset-turn-{turn}");
            let next_session = chat
                .run_in_session(
                    &format!("Continue the reset audit turn {turn}; reply briefly."),
                    session_id.as_deref(),
                    None,
                    None,
                    Vec::new(),
                    None,
                    false,
                    Some(&run_id),
                    |_| {},
                    |_| {},
                )
                .await
                .unwrap();
            session_id = Some(next_session);
        }
    }

    let requests = model
        .requests
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let reminder_occurrences: Vec<_> = requests
        .iter()
        .enumerate()
        .filter_map(|(index, request)| {
            let count = request.to_string().matches(REMINDER_MARKER).count();
            (count > 0).then_some((index + 1, count))
        })
        .collect();
    assert_eq!(requests.len(), 20);
    assert_eq!(
        reminder_occurrences,
        vec![(10, 1), (11, 1), (20, 1)],
        "request 11 carries the single reminder from its immediately preceding request; todo_write then restarts the next reminder window"
    );
    drop(requests);
    assert_eq!(
        persisted_progress(&db_path, session_id.as_deref().unwrap()),
        (20, (10, 0))
    );
}
