//! Periodic todo reminders must derive their counters from the durable session
//! history, not from the lifetime of one freshly assembled ModelAgent.

use super::*;
use deepagent_models::ModelRole;

const REMINDER_MARKER: &str = "todo-tracking tool hasn't been used in a while";

#[derive(Default)]
struct RecordingTodoModel {
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for RecordingTodoModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.requests
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(serde_json::from_str(&request.body)?);
        emit_responses_events(
            sink,
            &response_text("offline todo reminder audit completed"),
        )
    }
}

fn audit_settings(db: Arc<Database>) -> Arc<SettingsService> {
    Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ))
}

#[tokio::test]
#[ignore = "audit H-141: periodic todo reminder must survive independent session turns"]
async fn audit_todo_reminder_counts_survive_independent_session_turns() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = audit_settings(db.clone());
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }

    let model = Arc::new(RecordingTodoModel::default());
    let chat = ChatService::new(db, settings, model.clone(), root.path());
    let mut session_id = None;
    for turn in 0..=10 {
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
    assert!(
        !reminder_requests.is_empty(),
        "the periodic reminder must be derived from prior assistant turns when each turn assembles a new ModelAgent"
    );
}
