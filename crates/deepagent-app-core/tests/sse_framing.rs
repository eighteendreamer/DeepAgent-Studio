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
