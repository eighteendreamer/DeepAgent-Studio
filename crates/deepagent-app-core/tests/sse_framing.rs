//! H-30: actual HTTP framing -> ordinary ChatService -> one durable terminal.
#![cfg(feature = "web")]

#[path = "../../deepagent-models/tests/support/sse_http_fixture.rs"]
mod fixture;

use async_trait::async_trait;
use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_core::error::Result;
use deepagent_models::{
    transport::{EventSink, HttpTransport, TransportRequest},
    MockTransport, ReqwestTransport, WireMode,
};
use deepagent_persistence::{run_store::RunStore, Database};
use std::sync::{atomic::AtomicBool, Arc};

struct LocalTransport {
    base_url: String,
    inner: ReqwestTransport,
}

impl LocalTransport {
    fn local(&self, mut request: TransportRequest) -> TransportRequest {
        assert!(request.url.ends_with("/responses"));
        request.url = format!("{}/responses", self.base_url);
        request
    }
}

#[async_trait]
impl HttpTransport for LocalTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.inner.stream(self.local(request), sink).await
    }

    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        self.inner
            .stream_cancelled(self.local(request), sink, cancel)
            .await
    }
}

#[tokio::test]
async fn h30_real_http_framing_controls_the_single_terminal_after_sqlite_reopen() {
    for (name, newline, multiline, terminator, expected_success) in [
        ("single-lf", "\n", false, "\n\n", true),
        ("multiline-lf", "\n", true, "\n\n", true),
        ("multiline-cr", "\r", true, "\r\r", true),
        ("multiline-crlf", "\r\n", true, "\r\n\r\n", true),
        ("unterminated", "\n", false, "", false),
        ("line-only", "\n", false, "\n", false),
        ("crlf-line-only", "\r\n", false, "\r\n", false),
    ] {
        let payload = fixture::response_terminal();
        let payload = if multiline {
            payload.replacen(':', &format!(":{newline}data: "), 1)
        } else {
            payload
        };
        let wire = format!(": heartbeat{newline}data: {payload}{terminator}");
        let server = fixture::Fixture::start(wire.bytes().map(|byte| vec![byte]).collect()).await;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("h30-synthetic").await.unwrap();
        let transport = Arc::new(LocalTransport {
            base_url: server.config(WireMode::Responses).base_url,
            inner: fixture::Fixture::transport(),
        });
        let chat = ChatService::new(db.clone(), settings.clone(), transport, root.path());
        let run = format!("h30-{name}");
        let session = chat.create_session(None).unwrap();
        let result = chat
            .run_in_session(
                "Acknowledge the synthetic input.",
                Some(&session),
                None,
                None,
                vec![],
                None,
                false,
                Some(&run),
                |_| {},
                |_| panic!("no tool approval expected"),
            )
            .await;
        let requests = server.finish().await;
        assert!(
            !requests.is_empty() && requests.len() <= 3,
            "unexpected retry count: {}",
            requests.len()
        );
        assert_eq!(result.is_ok(), expected_success, "{name}: {result:?}");
        if expected_success {
            assert_eq!(requests.len(), 1);
        }
        let expected_terminal = if expected_success {
            "succeeded"
        } else {
            "failed"
        };
        let stored = RunStore::new(&db).get(&run).unwrap().unwrap();
        assert_eq!(stored.terminal_kind.as_deref(), Some(expected_terminal));
        let events = RunStore::new(&db).events_after(&run, None).unwrap();
        let terminals: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], expected_terminal);
        drop(chat);
        drop(settings);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            RunStore::new(&reopened)
                .get(&run)
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some(expected_terminal)
        );
        assert_eq!(
            RunStore::new(&reopened).events_after(&run, None).unwrap(),
            events
        );
    }
}

#[tokio::test]
async fn h257_real_http_encoding_failures_are_not_retried_and_survive_sqlite_reopen_once() {
    use deepagent_app_core::CostService;
    use deepagent_persistence::cost_store::CostStore;
    use deepagent_runtime::events::RuntimeEvent;
    use std::sync::Mutex;

    for (name, tail, expected_error) in [
        ("invalid", vec![0xff], Some("invalid UTF-8")),
        (
            "incomplete",
            vec![0xf0, 0x9f, 0x99],
            Some("incomplete UTF-8"),
        ),
        ("valid", vec![], None),
    ] {
        let mut wire = b"data: {\"type\":\"response.output_text.delta\",\"sequence_number\":0,\"item_id\":\"h257-output\",\"output_index\":0,\"content_index\":0,\"delta\":\"before|\"}\n\n".to_vec();
        wire.extend(tail);
        if name != "incomplete" {
            let mut terminal: serde_json::Value =
                serde_json::from_str(&fixture::response_terminal()).unwrap();
            terminal["sequence_number"] = 1.into();
            terminal["response"]["output"][0]["id"] = "h257-output".into();
            terminal["response"]["output"][0]["content"][0]["text"] = "before|".into();
            wire.extend(format!("data: {terminal}\n\n").as_bytes());
        }
        let server =
            fixture::Fixture::start(wire.into_iter().map(|byte| vec![byte]).collect()).await;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("h257-synthetic").await.unwrap();
        let transport = Arc::new(LocalTransport {
            base_url: server.config(WireMode::Responses).base_url,
            inner: fixture::Fixture::transport(),
        });
        let cost = Arc::new(CostService::new(db.clone()));
        let chat = ChatService::new(db.clone(), settings.clone(), transport, root.path())
            .with_cost(cost.clone());
        let run = format!("h257-{name}");
        let session = chat.create_session(None).unwrap();
        let live = Arc::new(Mutex::new(Vec::new()));
        let callback = live.clone();
        let result = chat
            .run_in_session(
                "Acknowledge the synthetic input.",
                Some(&session),
                None,
                None,
                vec![],
                None,
                false,
                Some(&run),
                move |event| callback.lock().unwrap().push(event),
                |_| panic!("no tool approval expected"),
            )
            .await;
        let requests = server.finish().await;
        assert_eq!(requests.len(), 1, "{name}: must not retry or fall back");
        let expected_terminal = if let Some(expected_error) = expected_error {
            let error = result.unwrap_err();
            assert!(error.to_string().contains(expected_error), "{error}");
            "failed"
        } else {
            assert!(result.is_ok(), "{result:?}");
            "succeeded"
        };
        assert!(!live.lock().unwrap().iter().any(|event| matches!(
            event,
            RuntimeEvent::ModelAttemptReset { .. } | RuntimeEvent::Usage { .. }
        )));
        let receipts = CostStore::new(&db)
            .run_call_receipts(&session, &run)
            .unwrap();
        assert_eq!(receipts.len(), usize::from(expected_error.is_none()));
        assert!(receipts.iter().all(|receipt| receipt.ledger_id.is_none()));
        assert!(CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap()
            .is_empty());
        let summary = cost.summary(&session).unwrap();
        assert_eq!((summary.input_tokens, summary.output_tokens), (0, 0));
        let stored = RunStore::new(&db).get(&run).unwrap().unwrap();
        assert_eq!(stored.terminal_kind.as_deref(), Some(expected_terminal));
        let events = RunStore::new(&db).events_after(&run, None).unwrap();
        assert!(events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        let terminals: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], expected_terminal);
        drop(chat);
        drop(settings);
        drop(cost);
        drop(db);
        let reopened = Arc::new(Database::open(&path).unwrap());
        assert_eq!(RunStore::new(&reopened).get(&run).unwrap().unwrap(), stored);
        assert_eq!(
            RunStore::new(&reopened).events_after(&run, None).unwrap(),
            events
        );
        assert_eq!(
            serde_json::to_value(
                CostStore::new(&reopened)
                    .run_call_receipts(&session, &run)
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(receipts).unwrap()
        );
        assert_eq!(
            CostService::new(reopened).summary(&session).unwrap(),
            summary
        );
    }
}
