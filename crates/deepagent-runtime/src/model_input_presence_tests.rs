//! H-150 fault injection after history replacement, not a normal ChatService empty turn.
use super::*;
use crate::{AgentKernel, RunRequest, RuntimeConfig, TerminalKind};
use deepagent_core::clock::FixedClock;
use deepagent_models::{HttpTransport, ModelConfig, WireMode};
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

#[derive(Default)]
struct Recorder(AtomicUsize);

#[async_trait]
impl HttpTransport for Recorder {
    async fn stream(
        &self,
        _: deepagent_models::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        self.0.fetch_add(1, Ordering::SeqCst);
        sink.on_event(r#"{"type":"response.completed","response":{"id":"fixture","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#)?;
        Ok(())
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<RuntimeEvent>>);
impl RuntimeEventSink for Events {
    fn emit(&self, event: RuntimeEvent) {
        self.0.lock().unwrap().push(event);
    }
}

#[tokio::test]
async fn h150_empty_replaced_history_fails_without_retry_or_dispatch() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let transport = Arc::new(Recorder::default());
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        ));
        let events = Arc::new(Events::default());
        let mut agent = ModelAgent::new(
            client,
            "deepseek-flash",
            "synthetic rules",
            "synthetic goal",
            vec![],
        )
        .with_max_model_attempts(5)
        .with_events(events.clone());
        // Exercise the real retry loop after the canonical Surface is lost.
        // No production constructor is changed to manufacture an empty turn.
        agent.response_history = ResponseHistory::from_messages(&[]);
        agent.sync_message_projection();
        let error = agent.think(0, &[]).await.unwrap_err();
        assert!(matches!(error, CoreError::Invalid(_)));
        assert!(!classify_model_error(&error).retryable());
        assert_eq!(transport.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            events
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event, RuntimeEvent::ModelRequestStarted { .. }))
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn h150_kernel_empty_history_failure_and_cancellation_survive_sqlite_reopen() {
    for cancelled in [false, true] {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("h150.sqlite");
        let transport = Arc::new(Recorder::default());
        {
            let db = Arc::new(Database::open(&path).unwrap());
            let clock = FixedClock::new(1);
            let mut session = Session::create(&db, &clock, None).unwrap();
            let task = session.create_task("synthetic lifecycle task").unwrap();
            let registry = ToolRegistry::new();
            let kernel = AgentKernel::<FixedClock>::new(
                db.clone(),
                &registry,
                Metrics::new(),
                RuntimeConfig::default(),
                "h150-run",
            )
            .with_events(Arc::new(Events::default()));
            let mut agent = ModelAgent::new(
                Arc::new(ModelClient::new(
                    transport.clone(),
                    ModelConfig::deepseek("synthetic"),
                )),
                "deepseek-flash",
                "synthetic rules",
                "synthetic goal",
                vec![],
            )
            .with_max_model_attempts(5)
            .with_events(kernel.event_sink());
            agent.response_history = ResponseHistory::from_messages(&[]);
            agent.sync_message_projection();
            if cancelled {
                assert!(kernel.handle().cancel());
            }
            let terminal = kernel
                .start(RunRequest::new(&mut session, task, &mut agent))
                .await
                .unwrap();
            assert_eq!(
                terminal.kind,
                if cancelled {
                    TerminalKind::Cancelled
                } else {
                    TerminalKind::Failed
                }
            );
            assert_eq!(transport.0.load(Ordering::SeqCst), 0);
        }
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        let record = store.get("h150-run").unwrap().unwrap();
        assert_eq!(
            record.terminal_kind.as_deref(),
            Some(if cancelled { "cancelled" } else { "failed" })
        );
        let events = store.events_after("h150-run", None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        assert!(!events.iter().any(|event| matches!(
            event.event_type.as_str(),
            "tool_started" | "action.running" | "run_completed"
        )));
        if !cancelled {
            assert!(record.terminal_reason.unwrap().contains("input"));
        }
        assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    }
}
