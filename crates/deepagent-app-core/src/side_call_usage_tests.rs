//! Exercise the shipped auxiliary consumers, not synthetic accounting reports.
use super::*;
use crate::responses_fixture;
use crate::stall_classifier::ModelStallClassifier;
use crate::verification_panel::{ModelSkepticSpawner, SkepticSpawner};
use async_trait::async_trait;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig};
use deepagent_runtime::stall_detector::StallClassifier;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    Completed,
    FailedMeasured,
    FailedTerminal,
    FailedUnmeasured,
    WaitMeasured,
}

struct Transport {
    reply: Reply,
    text: &'static str,
    calls: AtomicUsize,
    observed: Notify,
}

#[async_trait]
impl HttpTransport for Transport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], "side-model");
        if self.reply == Reply::FailedTerminal {
            sink.on_event(
                &json!({"type":"response.failed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"failed",
                "error":{"code":"offline_failed","message":"controlled side provider failure"},
                "usage":{"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                    "input_tokens_details":{"cached_tokens":800}}}})
                .to_string(),
            )?;
            return Ok(());
        }
        if self.reply != Reply::FailedUnmeasured {
            let frames = responses_fixture::completed(
                "side-response",
                "side-model",
                responses_fixture::text("side-text", self.text),
                json!({"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                    "input_tokens_details":{"cached_tokens":800},"secret":"secret-canary"}),
            );
            for frame in &frames {
                sink.on_event(frame)?;
            }
            // A repeated terminal usage snapshot is not another physical call.
            sink.on_event(frames.last().unwrap())?;
        }
        self.observed.notify_one();
        match self.reply {
            Reply::Completed => Ok(()),
            Reply::WaitMeasured => std::future::pending().await,
            _ => Err(CoreError::other("controlled side provider failure")),
        }
    }
}

fn client(reply: Reply, text: &'static str) -> (Arc<ModelClient>, Arc<Transport>) {
    let transport = Arc::new(Transport {
        reply,
        text,
        calls: AtomicUsize::new(0),
        observed: Notify::new(),
    });
    (
        Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        transport,
    )
}

fn assert_settled(owner: &RunCallUsageOwner, purpose: &str, outcome: &str, measured: bool) {
    let records = RunStore::new(&owner.db).events_after("run", None).unwrap();
    let first = records
        .iter()
        .find(|event| event.event_type == "model_side_call_started")
        .unwrap();
    let last = records.last().unwrap();
    assert_eq!(first.event_type, "model_side_call_started");
    assert_eq!(last.event_type, "model_side_call_finished");
    assert_eq!(last.data["outcome"], outcome);
    assert_eq!(first.data["invocation_id"], last.data["invocation_id"]);
    assert_eq!(first.data["purpose"], purpose);
    assert_eq!(first.data["model_id"], "side-model");
    assert!(records
        .windows(2)
        .all(|pair| pair[0].sequence < pair[1].sequence));
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("secret-canary"));
    let receipts = CostStore::new(&owner.db)
        .run_call_receipts(&owner.session_id, "run")
        .unwrap();
    assert_eq!(receipts.len(), usize::from(measured));
    if measured {
        assert_eq!(receipts[0].report["model_id"], "side-model");
        assert_eq!(receipts[0].report["origin"], "side_call");
        assert_eq!(receipts[0].report["usage"]["prompt_cache_hit_tokens"], 800);
        assert_eq!(
            receipts[0].report["invocation_id"],
            first.data["invocation_id"]
        );
    }
    let usage = CostStore::new(&owner.db)
        .session_usage(&owner.session_id)
        .unwrap();
    assert_eq!(usage.input_tokens, if measured { 1000 } else { 0 });
    assert_eq!(usage.output_tokens, if measured { 100 } else { 0 });
}

#[tokio::test]
async fn skeptic_success_refute_and_parse_failure_do_not_control_billing() {
    for text in ["VERDICT: PASS", "VERDICT: FAIL", "not a verdict"] {
        for cancelled_entry in [false, true] {
            let (owner, _events) = setup();
            let (client, _) = client(Reply::Completed, text);
            let spawner =
                ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
            let result = if cancelled_entry {
                spawner
                    .spawn_cancelled(0, "offline", CancellationToken::new())
                    .await
            } else {
                spawner.spawn(0, "offline").await
            };
            assert_eq!(result.unwrap(), text);
            assert_settled(&owner, "adversarial_verification", "completed", true);
        }
    }
}

#[tokio::test]
async fn classifier_valid_invalid_and_provider_errors_keep_measured_usage() {
    for (reply, text, valid, outcome, measured) in [
        (
            Reply::Completed,
            r#"{"category":"not_stalled_complete","confidence":0.9,"evidence":"offline"}"#,
            true,
            "completed",
            true,
        ),
        (Reply::Completed, "not JSON", false, "completed", true),
        (Reply::FailedMeasured, "not JSON", false, "failed", true),
        (Reply::FailedUnmeasured, "not JSON", false, "failed", false),
        (Reply::FailedTerminal, "not JSON", false, "failed", true),
    ] {
        let (owner, _events) = setup();
        let (client, _) = client(reply, text);
        let classifier =
            ModelStallClassifier::new(client, "side-model").with_call_usage(owner.clone());
        assert_eq!(classifier.classify("offline").await.is_some(), valid);
        assert_settled(&owner, "stall_classification", outcome, measured);
    }
}

#[tokio::test]
async fn dropping_a_measured_skeptic_books_before_its_cancelled_lifecycle_event() {
    let (owner, _events) = setup();
    let (client, transport) = client(Reply::WaitMeasured, "VERDICT: PASS");
    let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
    let mut call = Box::pin(spawner.spawn(0, "offline"));
    tokio::select! {
        result = &mut call => panic!("request must still be waiting: {result:?}"),
        _ = transport.observed.notified() => {}
    }
    drop(call);
    assert_settled(&owner, "adversarial_verification", "cancelled", true);
}

#[tokio::test]
async fn explicit_panel_cancellation_keeps_usage_without_another_terminal() {
    let (owner, _events) = setup();
    let (client, transport) = client(Reply::WaitMeasured, "VERDICT: PASS");
    let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
    let token = CancellationToken::new();
    let call = spawner.spawn_cancelled(0, "offline", token.clone());
    let cancel = async {
        transport.observed.notified().await;
        token.cancel();
    };
    let (result, ()) = tokio::join!(call, cancel);
    assert!(result.is_err());
    assert_settled(&owner, "adversarial_verification", "cancelled", true);
    assert!(RunStore::new(&owner.db)
        .get("run")
        .unwrap()
        .unwrap()
        .terminal_kind
        .is_none());
}

#[tokio::test]
async fn classifier_real_timeout_settles_snapshot_without_changing_advisory_contract() {
    let (owner, _events) = setup();
    let (client, _) = client(Reply::WaitMeasured, "not JSON");
    let classifier = ModelStallClassifier::new(client, "side-model").with_call_usage(owner.clone());
    assert!(classifier.classify("offline").await.is_none());
    assert_settled(&owner, "stall_classification", "timed_out", true);
}

#[tokio::test]
async fn missing_cost_service_keeps_auditable_usage_without_fabricating_a_ledger_row() {
    let (mut owner, events) = setup();
    owner.cost = None;
    let (client, _) = client(Reply::Completed, "VERDICT: PASS");
    let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
    spawner.spawn(0, "offline").await.unwrap();
    let receipts = CostStore::new(&owner.db)
        .run_call_receipts(&owner.session_id, "run")
        .unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].report["usage"]["prompt_tokens"], 1000);
    assert_eq!(receipts[0].report["ledger_available"], false);
    assert!(receipts[0].ledger_id.is_none());
    assert!(CostStore::new(&owner.db)
        .session_costs_detailed(&owner.session_id)
        .unwrap()
        .is_empty());
    assert!(events.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn auxiliary_budget_rejection_precedes_dispatch_and_failed_writes_are_not_success() {
    for failed_write in [false, true] {
        let (owner, events) = setup();
        if failed_write {
            owner.db.with_conn(|conn| conn.execute_batch("CREATE TEMP TRIGGER fail_cost BEFORE INSERT ON costs BEGIN SELECT RAISE(FAIL,'controlled side cost failure'); END;").map_err(|e| CoreError::Persistence(e.to_string()))).unwrap();
        } else {
            owner
                .cost
                .as_ref()
                .unwrap()
                .set_budget(crate::cost_service::BudgetConfig {
                    daily_limit: Some(0.0),
                    monthly_limit: None,
                })
                .unwrap();
        }
        let (client, transport) = client(Reply::Completed, "VERDICT: PASS");
        let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
        let result = spawner.spawn(0, "offline").await;
        if failed_write {
            assert!(result.is_err());
        } else {
            assert_eq!(
                crate::verification_panel::parse_verdict(&result.unwrap()),
                Some(crate::verification_panel::Verdict::Partial)
            );
        }
        assert_eq!(
            transport.calls.load(Ordering::SeqCst),
            usize::from(failed_write)
        );
        let records = RunStore::new(&owner.db).events_after("run", None).unwrap();
        assert_eq!(
            records.last().unwrap().data["outcome"],
            if failed_write {
                "accounting_failed"
            } else {
                "rejected"
            }
        );
        assert!(events.0.lock().unwrap().is_empty());
        assert!(CostStore::new(&owner.db)
            .session_costs_detailed(&owner.session_id)
            .unwrap()
            .is_empty());
        if failed_write {
            // A fresh invocation must see the durable failed receipt, not
            // treat its private sink's empty error slot as paid usage = zero.
            owner
                .db
                .with_conn(|conn| {
                    conn.execute_batch("DROP TRIGGER fail_cost")
                        .map_err(|e| CoreError::Persistence(e.to_string()))
                })
                .unwrap();
            owner
                .cost
                .as_ref()
                .unwrap()
                .set_budget(crate::cost_service::BudgetConfig {
                    daily_limit: Some(1.0),
                    monthly_limit: None,
                })
                .unwrap();
            assert_eq!(
                crate::verification_panel::parse_verdict(
                    &spawner.spawn(0, "offline").await.unwrap()
                ),
                Some(crate::verification_panel::Verdict::Partial)
            );
            assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        }
    }
}
