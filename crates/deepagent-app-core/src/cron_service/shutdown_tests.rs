//! H97: synthetic file SQLite, real admission/kernel, offline held transport.
use super::*;
use crate::{AppService, MemorySecretStore, SettingsService};
use deepagent_core::error::CoreError;
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::{run_store::RunStore, Database};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Default)]
struct HeldTransport {
    calls: AtomicUsize,
    cancelled: AtomicUsize,
    release: AtomicBool,
    uncooperative: bool,
}

#[async_trait]
impl HttpTransport for HeldTransport {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        panic!("kernel must use controlled streaming");
    }
    async fn stream_cancelled(
        &self,
        _: TransportRequest,
        _: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        loop {
            if !self.uncooperative && cancel.load(Ordering::Acquire) {
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                return Err(CoreError::other("H97 synthetic model cancelled"));
            }
            if self.release.load(Ordering::Acquire) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    db: Arc<Database>,
    transport: Arc<HeldTransport>,
    chat: Arc<ChatService>,
    cron: Arc<CronService>,
}

impl Fixture {
    async fn new(uncooperative: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        crate::registry_fixture::configure(&settings, "h97-synthetic-credential")
            .await
            .unwrap();
        let transport = Arc::new(HeldTransport {
            uncooperative,
            ..Default::default()
        });
        let chat = Arc::new(ChatService::new(
            db.clone(),
            settings,
            transport.clone(),
            root.path(),
        ));
        let cron = Arc::new(CronService::new(root.path(), chat.clone()));
        Self {
            root,
            db,
            transport,
            chat,
            cron,
        }
    }
    fn due(&self) {
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        self.cron
            .scheduler
            .store()
            .add(
                "* * * * *".into(),
                "H97 synthetic scheduled request".into(),
                false,
                at - 120_000,
            )
            .unwrap();
    }
    fn start(&self, stop: CancellationToken) -> tokio::task::JoinHandle<Result<()>> {
        let cron = self.cron.clone();
        tokio::spawn(async move { cron.run_tick_loop(stop).await })
    }
    async fn started(&self, count: usize) {
        wait(|| self.transport.calls.load(Ordering::SeqCst) >= count).await;
    }
    fn run_ids(&self) -> Vec<String> {
        self.db
            .with_conn(|conn| {
                let mut query = conn
                    .prepare("SELECT id FROM runs ORDER BY created_at,id")
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                let rows = query
                    .query_map([], |row| row.get(0))
                    .map_err(|e| CoreError::Persistence(e.to_string()))?;
                rows.collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|e| CoreError::Persistence(e.to_string()))
            })
            .unwrap()
    }
    fn terminal(&self, id: &str, kind: &str) {
        let reopened = Database::open(self.root.path().join("state.sqlite")).unwrap();
        let store = RunStore::new(&reopened);
        let run = store.get(id).unwrap().unwrap();
        assert_eq!(run.terminal_kind.as_deref(), Some(kind));
        let events = store.events_after(id, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "schedule.accepted")
                .count(),
            1
        );
        if kind == "cancelled" {
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e.event_type == "cancel.requested")
                    .count(),
                1
            );
        }
    }
}

async fn wait(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("H97 fixture must reach its real lifecycle boundary");
}

#[tokio::test]
async fn h97_pre_cancelled_loop_never_admits_due_work() {
    let fixture = Fixture::new(false).await;
    fixture.due();
    for _ in 0..32 {
        let stop = CancellationToken::new();
        stop.cancel();
        fixture.cron.clone().run_tick_loop(stop).await.unwrap();
    }
    assert!(fixture.run_ids().is_empty());
    assert_eq!(fixture.transport.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.cron.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn h97_shutdown_drains_multiple_runs_without_cancelling_other_scope() {
    let fixture = Fixture::new(false).await;
    fixture.due();
    fixture.due();
    let stop = CancellationToken::new();
    let worker = fixture.start(stop.clone());
    fixture.started(2).await;
    let first = fixture.run_ids();
    let other_root = tempfile::tempdir().unwrap();
    let other = Arc::new(CronService::new(other_root.path(), fixture.chat.clone()));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    other
        .scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H97 independent owner".into(),
            false,
            at - 120_000,
        )
        .unwrap();
    let other_stop = CancellationToken::new();
    let stop_copy = other_stop.clone();
    let other_worker = tokio::spawn(async move { other.run_tick_loop(stop_copy).await });
    fixture.started(3).await;
    let foreign = fixture
        .run_ids()
        .into_iter()
        .find(|id| !first.contains(id))
        .unwrap();
    stop.cancel();
    worker.await.unwrap().unwrap();
    for id in &first {
        fixture.terminal(id, "cancelled");
    }
    assert!(RunStore::new(&fixture.db)
        .get(&foreign)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    assert_eq!(fixture.transport.cancelled.load(Ordering::SeqCst), 2);
    other_stop.cancel();
    other_worker.await.unwrap().unwrap();
    fixture.terminal(&foreign, "cancelled");
    assert_eq!(fixture.chat.pending_approvals().pending_count(), 0);
}

#[tokio::test]
async fn h97_aborted_loop_routes_cancel_and_does_not_abort_the_terminal_writer() {
    let fixture = Fixture::new(false).await;
    fixture.due();
    let worker = fixture.start(CancellationToken::new());
    fixture.started(1).await;
    let id = fixture.run_ids().pop().unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    wait(|| {
        RunStore::new(&fixture.db)
            .get(&id)
            .unwrap()
            .unwrap()
            .finished_at
            .is_some()
    })
    .await;
    fixture.terminal(&id, "cancelled");
    assert_eq!(fixture.transport.cancelled.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h97_noncooperative_run_is_reported_unresolved_without_fabricated_terminal() {
    let fixture = Fixture::new(true).await;
    fixture.due();
    let stop = CancellationToken::new();
    let worker = fixture.start(stop.clone());
    fixture.started(1).await;
    let id = fixture.run_ids().pop().unwrap();
    stop.cancel();
    let result = tokio::time::timeout(Duration::from_secs(13), worker)
        .await
        .unwrap()
        .unwrap();
    let unfinished = RunStore::new(&fixture.db)
        .get(&id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none();
    // Release even a red assertion so the fixture never leaves live work.
    fixture.transport.release.store(true, Ordering::Release);
    wait(|| {
        RunStore::new(&fixture.db)
            .get(&id)
            .unwrap()
            .unwrap()
            .finished_at
            .is_some()
    })
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("unresolved after 10s"));
    assert!(
        unfinished,
        "timeout must not manufacture a terminal for a live request"
    );
    fixture.terminal(&id, "cancelled");
}

#[tokio::test]
async fn h97_failed_terminal_transaction_is_not_hidden_by_worker_completion() {
    let fixture = Fixture::new(false).await;
    fixture.due();
    let stop = CancellationToken::new();
    let worker = fixture.start(stop.clone());
    fixture.started(1).await;
    let id = fixture.run_ids().pop().unwrap();
    fixture.db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER h97_fail BEFORE INSERT ON run_events WHEN NEW.event_type='run_terminal' BEGIN SELECT RAISE(ABORT,'H97 terminal failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
    stop.cancel();
    let result = worker.await.unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("without durable terminal"));
    assert!(RunStore::new(&fixture.db)
        .get(&id)
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    fixture
        .db
        .with_conn(|conn| {
            conn.execute_batch("DROP TRIGGER h97_fail")
                .map_err(|e| CoreError::Persistence(e.to_string()))
        })
        .unwrap();
    let app = AppService::from_shared(fixture.db.clone());
    assert_eq!(app.recover_unfinished_runs().unwrap().len(), 1);
    fixture.terminal(&id, "failed");
    assert!(app.recover_unfinished_runs().unwrap().is_empty());
}

#[tokio::test]
async fn h97_poll_read_failure_cancels_and_drains_already_admitted_work() {
    let fixture = Fixture::new(false).await;
    fixture.due();
    let worker = fixture.start(CancellationToken::new());
    fixture.started(1).await;
    let id = fixture.run_ids().pop().unwrap();
    std::fs::write(fixture.cron.scheduler.store().path(), "{").unwrap();
    // The actual production cadence is 60s; do not replace its read error path.
    let error = tokio::time::timeout(Duration::from_secs(70), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("read schedule before dispatch"));
    fixture.terminal(&id, "cancelled");
    assert_eq!(fixture.transport.cancelled.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read_to_string(fixture.cron.scheduler.store().path()).unwrap(),
        "{"
    );
}

#[tokio::test]
async fn h97_successful_completion_wins_without_a_second_terminal_or_cancel_audit() {
    let fixture = super::delivery_tests::Fixture::new().await;
    let cron = Arc::new(CronService::new(&fixture.root, fixture.chat.clone()));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    cron.scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H97 normal completion".into(),
            false,
            at - 120_000,
        )
        .unwrap();
    let stop = CancellationToken::new();
    let copy = stop.clone();
    let worker = tokio::spawn(async move { cron.run_tick_loop(copy).await });
    wait(|| {
        RunStore::new(&fixture.db).unfinished().unwrap().is_empty()
            && fixture.transport.0.load(Ordering::SeqCst) == 1
    })
    .await;
    stop.cancel();
    worker.await.unwrap().unwrap();
    fixture.db.with_conn(|conn| {
        let counts: (i64, i64, i64) = conn.query_row("SELECT (SELECT count(*) FROM runs WHERE terminal_kind='succeeded'), (SELECT count(*) FROM run_events WHERE event_type='run_terminal'), (SELECT count(*) FROM run_events WHERE event_type='cancel.requested')", [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))
            .map_err(|e| CoreError::Persistence(e.to_string()))?;
        assert_eq!(counts, (1,1,0)); Ok(())
    }).unwrap();
}

#[tokio::test]
async fn h97_shutdown_while_a_real_tool_waits_uses_the_kernel_cancellation_flag() {
    use deepagent_builtins::bash_tool::{CommandExecutor, CommandOutcome, CommandShell};
    struct HeldCommand {
        started: AtomicBool,
        cancelled: AtomicBool,
    }
    #[async_trait]
    impl CommandExecutor for HeldCommand {
        async fn run(&self, _: &str, _: &str) -> Result<CommandOutcome> {
            panic!("must use the kernel-controlled command seam");
        }
        async fn run_controlled(
            &self,
            _: &str,
            _: &str,
            _: CommandShell,
            cancel: Arc<AtomicBool>,
            _: Duration,
        ) -> Result<CommandOutcome> {
            self.started.store(true, Ordering::Release);
            while !cancel.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            self.cancelled.store(true, Ordering::Release);
            Err(CoreError::other("H97 synthetic tool cancellation"))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "h97-synthetic-credential")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(crate::PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_approval_policy(crate::ApprovalPolicy::FullAccess)
        .unwrap();
    // As in the existing kernel fixture, a non-Direct profile retains the
    // injected command seam. FullAccess+FullAccess intentionally uses host
    // execution and therefore cannot exercise this synthetic held executor.
    settings
        .set_sandbox_mode(crate::SandboxMode::WorkspaceWrite)
        .unwrap();
    let item = serde_json::json!({"type":"function_call","id":"h97-tool","call_id":"h97-tool","name":"bash","arguments":r#"{"command":"echo h97-synthetic"}"#});
    let transport = Arc::new(crate::test_sequence::mock([
        serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
        serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
        serde_json::json!({"type":"response.completed","response":{"id":"h97-offline","object":"response","created_at":1,"model":"deepseek-flash","output":[item],"status":"completed"}}).to_string(),
    ]));
    let command = Arc::new(HeldCommand {
        started: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
    });
    let chat = Arc::new(
        ChatService::new(db.clone(), settings, transport, root.path())
            .with_local_command_executor(command.clone()),
    );
    let cron = Arc::new(CronService::new(root.path(), chat.clone()));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    cron.scheduler
        .store()
        .add(
            "* * * * *".into(),
            "H97 synthetic controlled tool".into(),
            false,
            at - 120_000,
        )
        .unwrap();
    let stop = CancellationToken::new();
    let copy = stop.clone();
    let worker = tokio::spawn(async move { cron.run_tick_loop(copy).await });
    wait(|| command.started.load(Ordering::Acquire)).await;
    stop.cancel();
    worker.await.unwrap().unwrap();
    assert!(command.cancelled.load(Ordering::Acquire));
    assert!(RunStore::new(&db).unfinished().unwrap().is_empty());
    assert_eq!(chat.pending_approvals().pending_count(), 0);
    db.with_conn(|conn| {
        let counts: (i64,i64,i64) = conn.query_row("SELECT (SELECT count(*) FROM runs WHERE terminal_kind='cancelled'),(SELECT count(*) FROM run_events WHERE event_type='run_terminal'),(SELECT count(*) FROM run_events WHERE event_type='cancel.requested')", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e| CoreError::Persistence(e.to_string()))?;
        assert_eq!(counts,(1,1,1)); Ok(())
    }).unwrap();
}
