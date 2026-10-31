//! Exercise the shipped auxiliary consumers rather than synthetic accounting
//! reports. These tests use the real ModelClient observer boundary.

use super::*;
use crate::responses_fixture;
use crate::stall_classifier::ModelStallClassifier;
use crate::verification_panel::{ModelSkepticSpawner, SkepticSpawner};
use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{ModelClient, ModelConfig};
use deepagent_persistence::cost_store::CostStore;
use deepagent_runtime::stall_detector::StallClassifier;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    Completed,
    FailedMeasured,
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
        if self.reply != Reply::FailedUnmeasured {
            let frames = responses_fixture::completed(
                "side-response",
                "side-model",
                responses_fixture::text("side-text", self.text),
                serde_json::json!({"input_tokens":1000,"output_tokens":100,"total_tokens":1100,
                    "input_tokens_details":{"cached_tokens":800},"secret":"secret-canary"}),
            );
            for frame in &frames {
                sink.on_event(frame)?;
            }
            // Repeated terminal usage is one physical call, not a second charge.
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
    assert_eq!(last.event_type, "model_side_call_finished");
    assert_eq!(last.data["outcome"], outcome);
    assert_eq!(first.data["invocation_id"], last.data["invocation_id"]);
    assert_eq!(first.data["purpose"], purpose);
    assert_eq!(first.data["model_id"], "side-model");
    assert!(!serde_json::to_string(&records)
        .unwrap()
        .contains("secret-canary"));

    let receipts: Vec<_> = records
        .iter()
        .filter(|event| event.event_type == "model_call_usage")
        .collect();
    assert_eq!(receipts.len(), usize::from(measured));
    if measured {
        let report = &receipts[0].data["report"];
        assert_eq!(report["origin"], "side_call");
        assert_eq!(report["model_id"], "side-model");
        assert_eq!(report["usage"]["prompt_cache_hit_tokens"], 800);
        assert_eq!(report["invocation_id"], first.data["invocation_id"]);
    }
    let usage = CostStore::new(&owner.db)
        .session_usage(&owner.session_id)
        .unwrap();
    assert_eq!(usage.input_tokens, if measured { 1000 } else { 0 });
    assert_eq!(usage.output_tokens, if measured { 100 } else { 0 });
}

#[tokio::test]
async fn skeptic_output_parsing_does_not_control_usage_booking() {
    for text in ["VERDICT: PASS", "VERDICT: FAIL", "not a verdict"] {
        let (owner, _events) = setup();
        let (client, _) = client(Reply::Completed, text);
        let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
        assert_eq!(spawner.spawn(0, "offline").await.unwrap(), text);
        assert_settled(&owner, "adversarial_verification", "completed", true);
    }
}

#[tokio::test]
async fn classifier_parse_and_provider_failures_keep_measured_usage() {
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
async fn cancellation_after_a_usage_frame_keeps_the_receipt() {
    let (owner, _events) = setup();
    let (client, transport) = client(Reply::WaitMeasured, "VERDICT: PASS");
    let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
    let cancellation = CancellationToken::new();
    let call = spawner.spawn_cancelled(0, "offline", cancellation.clone());
    let cancel = async {
        transport.observed.notified().await;
        cancellation.cancel();
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
async fn classifier_timeout_keeps_the_last_measured_snapshot() {
    let (owner, _events) = setup();
    let (client, _) = client(Reply::WaitMeasured, "not JSON");
    let classifier = ModelStallClassifier::new(client, "side-model").with_call_usage(owner.clone());
    assert!(classifier.classify("offline").await.is_none());
    assert_settled(&owner, "stall_classification", "timed_out", true);
}

#[tokio::test]
async fn missing_cost_service_keeps_auditable_usage_without_a_ledger_row() {
    let (mut owner, events) = setup();
    owner.cost = None;
    let (client, _) = client(Reply::Completed, "VERDICT: PASS");
    let spawner = ModelSkepticSpawner::new(client, "side-model").with_call_usage(owner.clone());
    spawner.spawn(0, "offline").await.unwrap();
    let receipts: Vec<_> = RunStore::new(&owner.db)
        .events_after("run", None)
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type == "model_call_usage")
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].data["report"]["usage"]["prompt_tokens"], 1000);
    assert_eq!(receipts[0].data["report"]["ledger_available"], false);
    assert!(receipts[0].data["ledger_id"].is_null());
    assert!(CostStore::new(&owner.db)
        .session_costs_detailed(&owner.session_id)
        .unwrap()
        .is_empty());
    assert!(events.0.lock().unwrap().is_empty());
}
