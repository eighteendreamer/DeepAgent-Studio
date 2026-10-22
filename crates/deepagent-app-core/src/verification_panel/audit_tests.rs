//! Opt-in audit probes for the model-backed verification lifecycle.

use super::*;
use deepagent_core::clock::FixedClock;
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig};
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::{
    AgentKernel, CheckpointManager, ModelAgent, RunRequest, RuntimeConfig, TerminalKind,
};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

struct ActiveRequest<'a>(&'a AtomicUsize);
impl Drop for ActiveRequest<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct WaitingTransport {
    entered: Semaphore,
    release: Semaphore,
    active: AtomicUsize,
    plain_calls: AtomicUsize,
    cancellable_calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for WaitingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.plain_calls.fetch_add(1, Ordering::SeqCst);
        // This is the actual serialized skeptic request, not a custom
        // verifier which ignores a supplied cancellation signal.
        assert!(request.body.contains("FILES THE AGENT CHANGED THIS RUN"));
        assert!(request.body.contains("answer.txt"));
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveRequest(&self.active);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        assert!(
            !sink.on_event(r#"{"type":"response.output_text.delta","delta":"VERDICT: PASS"}"#,)?
        );
        assert!(
            sink.on_event(r#"{"type":"response.completed","response":{"status":"completed"}}"#,)?
        );
        Ok(())
    }

    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        flag: Arc<AtomicBool>,
    ) -> Result<()> {
        self.cancellable_calls.fetch_add(1, Ordering::SeqCst);
        tokio::select! {
            result = self.stream(request, sink) => result,
            _ = async {
                while !flag.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            } => Err(deepagent_core::error::CoreError::other("request cancelled")),
        }
    }
}

fn changed_file_checkpoint(
    db: Arc<Database>,
    run_id: &str,
) -> (tempfile::TempDir, Arc<CheckpointManager>) {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let checkpoint = Arc::new(
        CheckpointManager::new(
            db.clone(),
            run_id,
            0,
            &workspace,
            temp.path().join("checkpoints"),
        )
        .unwrap(),
    );
    let file = workspace.join("answer.txt");
    checkpoint.capture_before(&file).unwrap();
    std::fs::write(&file, "answer").unwrap();
    assert_eq!(
        checkpoint.mutation_evidence().unwrap()[0].kind,
        deepagent_runtime::checkpoint::MutationKind::Created,
    );

    (temp, checkpoint)
}

fn main_agent() -> ModelAgent {
    ModelAgent::new(
        Arc::new(ModelClient::new(
            Arc::new(MockTransport::new([
                r#"{"type":"response.output_text.delta","delta":"Created answer.txt."}"#
                    .to_string(),
                r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
            ])),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        "deepseek-flash",
        "system",
        "create answer.txt",
        vec![],
    )
}

/// Exercise the shipped model-backed panel through the kernel. Only the
/// provider transport is offline; checkpoint evidence is a real temporary
/// file mutation. Kept opt-in because these are unfixed audit assertions.
async fn audit_panel_lifecycle(cancel: bool, deadline: bool) {
    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("panel lifecycle audit")).unwrap();
    let task = session.create_task("create answer.txt").unwrap();
    let run_id = "audit-panel-lifecycle";
    RunStore::new(&db)
        .create(
            run_id,
            &session.id().to_string(),
            Some(&task.to_string()),
            1,
        )
        .unwrap();
    let (_temp, checkpoint) = changed_file_checkpoint(db.clone(), run_id);

    let transport = Arc::new(WaitingTransport {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        active: AtomicUsize::new(0),
        plain_calls: AtomicUsize::new(0),
        cancellable_calls: AtomicUsize::new(0),
    });
    let verifier = Arc::new(ModelAdversarialVerifier::new(
        Arc::new(
            ModelSkepticSpawner::new(
                Arc::new(ModelClient::new(
                    transport.clone(),
                    ModelConfig::deepseek("offline-placeholder"),
                )),
                "deepseek-flash",
            )
            .with_system_prompt(GOAL_COVERAGE_SKEPTIC_PROMPT),
        ),
        "create answer.txt",
        DEFAULT_SKEPTIC_COUNT,
    ));
    let registry = ToolRegistry::new();
    let config = RuntimeConfig {
        checkpoint: Some(checkpoint),
        task_timeout: deadline.then_some(Duration::from_millis(300)),
        ..RuntimeConfig::default()
    };
    let kernel =
        AgentKernel::<FixedClock>::new(db.clone(), &registry, Metrics::new(), config, run_id)
            .with_adversarial_verifier(verifier);
    let handle = kernel.handle();
    let mut agent = main_agent();
    let exited = AtomicBool::new(false);
    let run = async {
        let result = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await;
        exited.store(true, Ordering::SeqCst);
        result.unwrap()
    };
    let observe = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            transport
                .entered
                .acquire_many(DEFAULT_SKEPTIC_COUNT)
                .await
                .unwrap()
                .forget();
        })
        .await
        .expect("all production skeptic requests must actually start");
        if cancel {
            assert!(handle.cancel());
            assert!(handle.is_cancelled());
        }
        tokio::time::sleep(Duration::from_millis(if deadline { 450 } else { 100 })).await;
        let exited_before_release = exited.load(Ordering::SeqCst);
        let active_before_release = transport.active.load(Ordering::SeqCst);
        let record = RunStore::new(&db).get(run_id).unwrap().unwrap();
        eprintln!(
            "panel cancel={cancel} deadline={deadline}: exited_before_release={exited_before_release} \
             active_before_release={active_before_release} terminal_before_release={:?}",
            record.terminal_kind,
        );
        // Always release and drain the spawned requests BEFORE asserting
        // the defect, so a failing probe leaves no detached fixture tasks.
        transport
            .release
            .add_permits(DEFAULT_SKEPTIC_COUNT as usize);
        tokio::time::timeout(Duration::from_secs(2), async {
            while transport.active.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all fixture requests cleaned up");
        (exited_before_release, active_before_release)
    };
    let (terminal, (exited_before_release, active_before_release)) =
        tokio::time::timeout(Duration::from_secs(5), async { tokio::join!(run, observe) })
            .await
            .expect("kernel and probe must settle after release");
    let record = RunStore::new(&db).get(run_id).unwrap().unwrap();
    let events = RunStore::new(&db).events_after(run_id, None).unwrap();
    let terminal_count = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .count();
    eprintln!(
        "panel cancel={cancel} deadline={deadline}: terminal={:?} persisted={:?} \
         task={:?} active={} plain_calls={} cancellable_calls={} terminal_count={terminal_count}",
        terminal.kind,
        record.terminal_kind,
        session.state().task(task).unwrap().state,
        transport.active.load(Ordering::SeqCst),
        transport.plain_calls.load(Ordering::SeqCst),
        transport.cancellable_calls.load(Ordering::SeqCst),
    );
    assert_eq!(transport.active.load(Ordering::SeqCst), 0);
    assert_eq!(terminal_count, 1);
    if deadline {
        assert!(
            exited_before_release,
            "task deadline must settle the main kernel"
        );
        assert_eq!(
            active_before_release, 0,
            "timed-out kernel must own and stop its panel"
        );
    } else if cancel {
        assert!(
            exited_before_release && terminal.kind == TerminalKind::Cancelled,
            "user cancellation must stop the panel and never become succeeded: \
             exited={exited_before_release}, terminal={:?}",
            terminal.kind,
        );
    } else {
        assert!(!exited_before_release);
        assert_eq!(active_before_release, DEFAULT_SKEPTIC_COUNT as usize);
        assert_eq!(terminal.kind, TerminalKind::Succeeded);
        assert_eq!(record.terminal_kind.as_deref(), Some("succeeded"));
    }
}

#[tokio::test]
async fn model_panel_completes_after_all_skeptics_reply() {
    audit_panel_lifecycle(false, false).await;
}

#[tokio::test]
#[ignore = "audit probe: cancellation must interrupt the production verification panel"]
async fn audit_cancel_during_model_panel_does_not_complete_successfully() {
    audit_panel_lifecycle(false, false).await;
    audit_panel_lifecycle(true, false).await;
}

#[tokio::test]
#[ignore = "audit probe: task deadline must not detach production skeptic requests"]
async fn audit_kernel_deadline_releases_model_panel_requests() {
    audit_panel_lifecycle(false, true).await;
}
