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

#[tokio::test]
async fn h131_real_http_progress_succeeds_once_but_actual_idle_failure_stays_failed_after_reopen() {
    use deepagent_app_core::{CostService, ModelPricing};
    use deepagent_core::{event::EventPayload, response_item::ResponseItem};
    use deepagent_models::TransportTimeouts;
    use deepagent_persistence::{cost_store::CostStore, event_store::EventStore};
    use deepagent_runtime::RuntimeEvent;
    use std::{sync::Mutex, time::Duration};

    for idle_failure in [false, true] {
        let mut chunks: Vec<_> = (0..6)
            .map(|sequence| {
                let event = serde_json::json!({"type":"response.output_text.delta",
                    "sequence_number":sequence,"item_id":"h30-output","output_index":0,
                    "content_index":0,"delta":sequence.to_string()});
                (
                    Duration::from_millis(60),
                    format!("data: {event}\n\n").into_bytes(),
                )
            })
            .collect();
        let mut terminal: serde_json::Value =
            serde_json::from_str(&fixture::response_terminal()).unwrap();
        terminal["sequence_number"] = 6.into();
        terminal["response"]["output"][0]["content"][0]["text"] = "012345".into();
        terminal["response"]["usage"] = serde_json::json!({"input_tokens":9,"output_tokens":2,"total_tokens":11,
            "input_tokens_details":{"cached_tokens":4},"output_tokens_details":{"reasoning_tokens":1}});
        chunks.push((
            Duration::from_millis(if idle_failure { 600 } else { 60 }),
            format!("data: {terminal}\n\n").into_bytes(),
        ));
        let server = fixture::Fixture::start_timed(chunks, Duration::ZERO, 200).await;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("h131-synthetic.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("h131-synthetic").await.unwrap();
        let limits = TransportTimeouts {
            connect: Duration::from_secs(2),
            first_byte: Duration::from_secs(2),
            idle: Duration::from_millis(if idle_failure { 200 } else { 1000 }),
            total: Duration::from_millis(if idle_failure { 500 } else { 250 }),
        };
        let transport = Arc::new(LocalTransport {
            base_url: server.config(WireMode::Responses).base_url,
            inner: fixture::Fixture::transport().with_timeouts(limits),
        });
        let cost = Arc::new(CostService::new(db.clone()));
        cost.set_pricing(std::collections::HashMap::from([(
            "deepseek-v4-flash".into(),
            ModelPricing {
                input_cache_hit_per_million: 100.0,
                input_cache_miss_per_million: 1000.0,
                output_per_million: 20.0,
            },
        )]));
        let chat = ChatService::new(db.clone(), settings.clone(), transport, root.path())
            .with_cost(cost.clone());
        let session = chat.create_session(None).unwrap();
        let run = if idle_failure {
            "h131-idle"
        } else {
            "h131-progress"
        };
        let events = Arc::new(Mutex::new(Vec::new()));
        let record = events.clone();
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(12),
            chat.run_in_session(
                "Acknowledge the synthetic stream.",
                Some(&session),
                None,
                None,
                vec![],
                None,
                false,
                Some(run),
                move |event| record.lock().unwrap().push(event),
                |_| panic!("no synthetic tool approval expected"),
            ),
        )
        .await
        .expect("bounded synthetic host run");
        let elapsed = started.elapsed();
        let requests = server.finish().await;
        assert_eq!(
            result.is_ok(),
            !idle_failure,
            "idle={idle_failure}: {result:?}"
        );
        assert!(elapsed > limits.total);
        assert_eq!(requests.len(), if idle_failure { 3 } else { 1 });
        if idle_failure {
            assert!(result.unwrap_err().to_string().contains("SSE idle timeout"));
        }
        let live = events.lock().unwrap();
        assert_eq!(
            live.iter()
                .filter(|event| matches!(event, RuntimeEvent::ModelAttemptReset { .. }))
                .count(),
            if idle_failure { 2 } else { 0 }
        );
        assert_eq!(
            live.iter()
                .filter(|event| matches!(event, RuntimeEvent::Usage { .. }))
                .count(),
            usize::from(!idle_failure)
        );
        let receipts = CostStore::new(&db)
            .run_call_receipts(&session, run)
            .unwrap();
        assert_eq!(receipts.len(), usize::from(!idle_failure));
        let costs = CostStore::new(&db)
            .session_costs_detailed(&session)
            .unwrap();
        assert_eq!(costs.len(), usize::from(!idle_failure));
        let summary = cost.summary(&session).unwrap();
        assert_eq!(
            (summary.input_tokens, summary.output_tokens),
            if idle_failure { (0, 0) } else { (9, 2) }
        );
        let history = EventStore::new(&db)
            .load_session(session.parse().unwrap())
            .unwrap();
        let replies: Vec<_> = history
            .iter()
            .filter_map(|event| match &event.payload {
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message { role, content },
                    ..
                } if role == "assistant" => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(replies, if idle_failure { vec![] } else { vec!["012345"] });
        let run_record = RunStore::new(&db).get(run).unwrap().unwrap();
        let terminal_kind = if idle_failure { "failed" } else { "succeeded" };
        assert_eq!(run_record.terminal_kind.as_deref(), Some(terminal_kind));
        let durable = RunStore::new(&db).events_after(run, None).unwrap();
        assert!(durable
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        assert_eq!(
            durable
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        drop(chat);
        drop(settings);
        drop(cost);
        drop(db);
        let reopened = Arc::new(Database::open(&path).unwrap());
        assert_eq!(
            RunStore::new(&reopened).get(run).unwrap().unwrap(),
            run_record
        );
        assert_eq!(
            RunStore::new(&reopened).events_after(run, None).unwrap(),
            durable
        );
        assert_eq!(
            EventStore::new(&reopened)
                .load_session(session.parse().unwrap())
                .unwrap(),
            history
        );
        assert_eq!(
            serde_json::to_value(
                CostStore::new(&reopened)
                    .run_call_receipts(&session, run)
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(receipts).unwrap()
        );
        assert_eq!(
            CostStore::new(&reopened)
                .session_costs_detailed(&session)
                .unwrap(),
            costs
        );
        assert_eq!(
            CostService::new(reopened).summary(&session).unwrap(),
            summary
        );
    }
}
