//! H96: real prepared admission, Kernel and file SQLite, offline provider.
use super::*;
use crate::{MemorySecretStore, SettingsService};
use deepagent_core::error::CoreError;
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::schedule::cron::local_to_epoch_ms;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

pub(super) struct CountingTransport(pub(super) AtomicUsize);

#[async_trait]
impl HttpTransport for CountingTransport {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        self.0.fetch_add(1, Ordering::SeqCst);
        for event in [
            r#"{"type":"response.output_text.delta","delta":"H96 done"}"#,
            r#"{"type":"response.completed","response":{"id":"h96-local","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

pub(super) struct Fixture {
    pub(super) root: std::path::PathBuf,
    _owner: Option<tempfile::TempDir>,
    pub(super) db: Arc<Database>,
    settings: Arc<SettingsService>,
    pub(super) transport: Arc<CountingTransport>,
    pub(super) chat: Arc<ChatService>,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let owner = tempfile::tempdir().unwrap();
        let mut fixture = Self::open(owner.path()).await;
        fixture._owner = Some(owner);
        fixture
    }
    pub(super) async fn open(root: &Path) -> Self {
        let root = root.to_path_buf();
        let db = Arc::new(Database::open(root.as_path().join("state.sqlite")).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings
            .initialize("h96-synthetic-credential")
            .await
            .unwrap();
        let transport = Arc::new(CountingTransport(AtomicUsize::new(0)));
        let chat = Arc::new(ChatService::new(
            db.clone(),
            settings.clone(),
            transport.clone(),
            root.as_path(),
        ));
        Self {
            root,
            _owner: None,
            db,
            settings,
            transport,
            chat,
        }
    }
    fn reopen_chat(&self) -> Arc<ChatService> {
        let db = Arc::new(Database::open(self.root.as_path().join("state.sqlite")).unwrap());
        Arc::new(ChatService::new(
            db,
            self.settings.clone(),
            self.transport.clone(),
            self.root.as_path(),
        ))
    }
}

#[cfg(windows)]
pub(super) struct Restore(pub(super) std::path::PathBuf);
#[cfg(windows)]
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = set_readonly(&self.0, false);
    }
}
#[cfg(windows)]
pub(super) fn set_readonly(path: &Path, value: bool) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(value);
    std::fs::set_permissions(path, permissions)
}

#[cfg(windows)]
#[tokio::test]
async fn h96_actual_kernel_delivery_is_not_repeated_after_bookkeeping_failure_and_reopen() {
    for recurring in [false, true] {
        let fixture = Fixture::new().await;
        let scheduler = CronScheduler::for_workspace(fixture.root.as_path());
        let now = time::macros::datetime!(2026-11-04 12:00:00);
        let at = local_to_epoch_ms(now);
        scheduler
            .store()
            .add(
                "* * * * *".into(),
                "H96 model-only synthetic prompt".into(),
                recurring,
                at - 120_000,
            )
            .unwrap();
        let path = scheduler.store().path().to_owned();
        let _restore = Restore(path.clone());
        let mut admitted = None;
        let mut run_id = String::new();
        let failed = scheduler.poll(now, |task, occurrence| {
            run_id.clone_from(&occurrence.run_id);
            let result = fixture.chat.admit_scheduled(task, occurrence)?;
            admitted = result.1;
            set_readonly(&path, true).unwrap();
            Ok(result.0)
        });
        assert!(failed.is_err());
        let origin = RunStore::new(&fixture.db)
            .scheduled_origin(&run_id)
            .unwrap()
            .unwrap();
        assert_eq!(origin.accepted_at_ms, at);
        fixture
            .chat
            .run_prepared(
                admitted.take().unwrap(),
                "H96 model-only synthetic prompt",
                |_| {},
                |_| {},
            )
            .await
            .unwrap();
        assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
        set_readonly(&path, false).unwrap();
        let reopened = CronScheduler::for_workspace(fixture.root.as_path());
        let chat = fixture.reopen_chat();
        reopened
            .poll(now, |task, occurrence| {
                let (receipt, prepared) = chat.admit_scheduled(task, occurrence)?;
                assert!(
                    prepared.is_none(),
                    "cold service cannot execute admitted occurrence again"
                );
                assert_eq!(occurrence.run_id, run_id);
                Ok(receipt)
            })
            .unwrap();
        assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
        let events = RunStore::new(&fixture.db)
            .events_after(&run_id, None)
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_accepted")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "schedule.accepted")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert!(!events
            .iter()
            .filter(|event| event.event_type == "schedule.accepted")
            .any(|event| event.data.to_string().contains("synthetic prompt")));
        if recurring {
            assert_eq!(reopened.store().read().unwrap()[0].last_fired_at, Some(at));
            let mut next = None;
            reopened
                .poll(now + time::Duration::minutes(1), |task, occurrence| {
                    assert_ne!(occurrence.run_id, run_id);
                    let (receipt, prepared) = chat.admit_scheduled(task, occurrence)?;
                    next = prepared.map(|prepared| (prepared, task.prompt.clone()));
                    Ok(receipt)
                })
                .unwrap();
            let (prepared, prompt) = next.unwrap();
            chat.run_prepared(prepared, &prompt, |_| {}, |_| {})
                .await
                .unwrap();
            assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 2);
        } else {
            assert!(reopened.store().read().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn h96_failed_admission_remains_due_and_does_not_publish_a_run_or_call_the_model() {
    let fixture = Fixture::new().await;
    let scheduler = CronScheduler::for_workspace(fixture.root.as_path());
    let now = time::macros::datetime!(2026-11-04 12:00:00);
    scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H96 admission rollback".into(),
            false,
            local_to_epoch_ms(now) - 120_000,
        )
        .unwrap();
    fixture.db.with_conn(|connection| connection.execute_batch("CREATE TRIGGER h96_fail BEFORE INSERT ON run_events WHEN NEW.event_type='schedule.accepted' BEGIN SELECT RAISE(ABORT, 'H96 synthetic admission failure'); END;")
        .map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    let mut run = String::new();
    assert!(scheduler
        .poll(now, |task, occurrence| {
            run.clone_from(&occurrence.run_id);
            fixture
                .chat
                .admit_scheduled(task, occurrence)
                .map(|(receipt, _)| receipt)
        })
        .is_err());
    assert!(RunStore::new(&fixture.db).get(&run).unwrap().is_none());
    assert!(RunStore::new(&fixture.db)
        .events_after(&run, None)
        .unwrap()
        .is_empty());
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 0);
    assert_eq!(scheduler.store().read().unwrap().len(), 1);
    fixture
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h96_fail")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    let mut prepared = None;
    scheduler
        .poll(now, |task, occurrence| {
            assert_eq!(occurrence.run_id, run);
            let (receipt, fresh) = fixture.chat.admit_scheduled(task, occurrence)?;
            prepared = fresh;
            Ok(receipt)
        })
        .unwrap();
    fixture
        .chat
        .run_prepared(prepared.unwrap(), "H96 admission rollback", |_| {}, |_| {})
        .await
        .unwrap();
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h96_real_tick_dispatch_executes_the_same_accepted_run_once() {
    let fixture = Fixture::new().await;
    let cron = Arc::new(CronService::new(
        fixture.root.as_path(),
        fixture.chat.clone(),
    ));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    cron.scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H96 actual tick".into(),
            false,
            now - 120_000,
        )
        .unwrap();
    let shutdown = CancellationToken::new();
    let owner = cron.clone();
    let stop = shutdown.clone();
    let worker = tokio::spawn(async move { owner.run_tick_loop(stop).await });
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let complete = fixture
                .db
                .with_conn(|connection| {
                    connection
                        .query_row(
                            "SELECT count(*) FROM runs WHERE finished_at IS NOT NULL",
                            [],
                            |row| row.get::<_, usize>(0),
                        )
                        .map_err(|error| CoreError::Persistence(error.to_string()))
                })
                .unwrap();
            if complete == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    shutdown.cancel();
    worker.await.unwrap().unwrap();
    result.unwrap();
    assert_eq!(fixture.transport.0.load(Ordering::SeqCst), 1);
    assert!(cron.list().await.unwrap().is_empty());
}
