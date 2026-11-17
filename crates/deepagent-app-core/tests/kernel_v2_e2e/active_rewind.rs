//! Rewinding a persisted session must fence an already running turn's writes.

use super::*;
use deepagent_app_core::AppService;
use deepagent_core::{clock::FixedClock, event::EventPayload, message::Message};
use deepagent_models::ModelRole;
use deepagent_persistence::{event_store::EventStore, run_store::RunStore};
use deepagent_session::Session;
use std::time::Duration;
use tokio::sync::Notify;

const OLD_USER: &str = "audit-active-rewind-discarded-user-85fa";
const OLD_ANSWER: &str = "audit-active-rewind-late-answer-85fa";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Success,
    ProviderFailure,
    Cancelled,
}

struct HeldAnswer {
    started: Arc<Notify>,
    release: Arc<Notify>,
    ending: Ending,
}

#[async_trait]
impl HttpTransport for HeldAnswer {
    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        // The trait's compatibility default only checks BEFORE stream(). This
        // barrier transport must implement the in-flight flag contract, like
        // the real HTTP transport, for a real cancellation-path regression.
        tokio::select! {
            result = self.stream(request, sink) => result,
            () = async {
                while !cancel.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => Err(CoreError::other("request cancelled")),
        }
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        assert!(request.body.contains(OLD_USER));
        self.started.notify_one();
        self.release.notified().await;
        if self.ending == Ending::ProviderFailure {
            return Err(CoreError::provider(
                Some(400),
                Some("fixture_rejection".into()),
                "offline history-control fixture rejection",
            ));
        }
        let item = serde_json::json!({
            "type":"message", "id":"audit-active-rewind-answer", "role":"assistant",
            "content":[{"type":"output_text","text":OLD_ANSWER}],
        });
        emit_responses_events(
            sink,
            &[
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash",
                    "status":"completed","output":[item]
                }})
                .to_string(),
            ],
        )
    }
}

fn history_contains(db: &Database, session_id: &str, marker: &str) -> bool {
    EventStore::new(db)
        .load_session(session_id.parse().unwrap())
        .unwrap()
        .iter()
        .any(|event| match &event.payload {
            EventPayload::MessageAppended { message } => message.content.contains(marker),
            EventPayload::ResponseItemAppended { item, .. } => {
                serde_json::to_string(item).unwrap().contains(marker)
            }
            _ => false,
        })
}

struct Observation {
    rewind_accepted: bool,
    discarded_before_release: bool,
    old_user_after: bool,
    old_answer_after: bool,
}

async fn observe_active_turn(rewind: bool, ending: Ending) -> Observation {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1_000);
    let mut session = Session::create(&db, &clock, Some("active rewind audit")).unwrap();
    for message in [
        Message::user("audit-active-rewind-kept-user"),
        Message::assistant("audit-active-rewind-kept-answer"),
    ] {
        session
            .append(EventPayload::MessageAppended { message })
            .unwrap();
    }
    let keep = EventStore::new(&db)
        .load_session(session.id())
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    let session_id = session.id().to_string();
    drop(session);

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
    let workspace = tempfile::tempdir().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let chat = Arc::new(ChatService::new(
        db.clone(),
        settings,
        Arc::new(HeldAnswer {
            started: started.clone(),
            release: release.clone(),
            ending,
        }),
        workspace.path(),
    ));
    let runner = chat.clone();
    let target = session_id.clone();
    let task = tokio::spawn(async move {
        runner
            .run_in_session(
                OLD_USER,
                Some(&target),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("audit-active-rewind"),
                |_| {},
                |_| {},
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("the real model request must reach the transport barrier");
    let active = RunStore::new(&db)
        .get("audit-active-rewind")
        .unwrap()
        .unwrap();
    assert_eq!(active.session_id, session_id);
    assert!(active.finished_at.is_none());
    assert!(history_contains(&db, &session_id, OLD_USER));
    assert!(!history_contains(&db, &session_id, OLD_ANSWER));

    let before = serde_json::to_value(
        EventStore::new(&db)
            .load_session(session_id.parse().unwrap())
            .unwrap(),
    )
    .unwrap();
    let rewind_accepted = if rewind {
        let result = AppService::from_shared(db.clone()).rewind_session(&session_id, keep);
        if let Err(error) = &result {
            assert!(error.to_string().contains("is busy"), "{error}");
            assert_eq!(
                serde_json::to_value(
                    EventStore::new(&db)
                        .load_session(session_id.parse().unwrap())
                        .unwrap()
                )
                .unwrap(),
                before,
            );
        }
        result.is_ok()
    } else {
        false
    };
    let discarded_before_release = !history_contains(&db, &session_id, OLD_USER);
    if ending == Ending::Cancelled {
        assert!(chat.request_cancel("audit-active-rewind").unwrap().accepted);
        // Do not release the response: prove the real cancellation path drops
        // the wait and returns its history ownership, not a successful reply.
    } else {
        release.notify_one();
    }
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("the released turn must finish")
        .unwrap();
    let terminal = RunStore::new(&db)
        .get("audit-active-rewind")
        .unwrap()
        .unwrap();
    let events = chat.run_events("audit-active-rewind", None).unwrap();
    assert!(terminal.finished_at.is_some());
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    assert!(history_contains(
        &db,
        &session_id,
        "audit-active-rewind-kept-answer"
    ));
    let observation = Observation {
        rewind_accepted,
        discarded_before_release,
        old_user_after: history_contains(&db, &session_id, OLD_USER),
        old_answer_after: history_contains(&db, &session_id, OLD_ANSWER),
    };
    eprintln!(
        "active rewind audit: ending={ending:?}, rewind={rewind}, accepted={}, removed_user_before_release={}, old_user_after={}, late_answer_after={}, run_ok={}, terminal={:?}",
        observation.rewind_accepted,
        observation.discarded_before_release,
        observation.old_user_after,
        observation.old_answer_after,
        result.is_ok(),
        terminal.terminal_kind,
    );
    if !rewind_accepted {
        match ending {
            Ending::Success => {
                result.unwrap();
                assert_eq!(terminal.terminal_kind.as_deref(), Some("succeeded"));
            }
            Ending::ProviderFailure => {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("fixture rejection"));
                assert_eq!(terminal.terminal_kind.as_deref(), Some("failed"));
            }
            Ending::Cancelled => {
                result.unwrap();
                assert_eq!(terminal.terminal_kind.as_deref(), Some("cancelled"));
            }
        }
    }
    if rewind {
        // Busy rejection is temporary: once the real invocation (including its
        // outer terminal projection) exits, rewind succeeds and stays clean.
        AppService::from_shared(db.clone())
            .rewind_session(&session_id, keep)
            .unwrap();
        assert!(!history_contains(&db, &session_id, OLD_USER));
        assert!(!history_contains(&db, &session_id, OLD_ANSWER));
        let record = RunStore::new(&db)
            .get("audit-active-rewind")
            .unwrap()
            .unwrap();
        assert!(record.finished_at.is_some());
        assert_eq!(
            chat.run_events("audit-active-rewind", None)
                .unwrap()
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
    }
    observation
}

#[tokio::test]
async fn held_model_answer_without_rewind_preserves_its_user() {
    let observed = observe_active_turn(false, Ending::Success).await;
    assert!(!observed.rewind_accepted);
    assert!(!observed.discarded_before_release);
    assert!(observed.old_user_after);
    assert!(observed.old_answer_after);
}

#[tokio::test]
async fn audit_active_rewind_does_not_commit_the_discarded_turn_answer() {
    let observed = observe_active_turn(true, Ending::Success).await;
    assert!(
        !observed.rewind_accepted
            || (observed.discarded_before_release
                && !observed.old_user_after
                && !observed.old_answer_after),
        "accepted rewind allowed the discarded active turn to write into the kept history"
    );
}

#[tokio::test]
async fn active_rewind_after_provider_failure_releases_history_ownership() {
    let observed = observe_active_turn(true, Ending::ProviderFailure).await;
    assert!(!observed.rewind_accepted);
    assert!(!observed.discarded_before_release);
    assert!(observed.old_user_after);
    assert!(!observed.old_answer_after);
}

#[tokio::test]
async fn active_rewind_after_real_cancellation_releases_history_ownership() {
    let observed = observe_active_turn(true, Ending::Cancelled).await;
    assert!(!observed.rewind_accepted);
    assert!(!observed.discarded_before_release);
    assert!(observed.old_user_after);
    assert!(!observed.old_answer_after);
}
