//! Explicit run model selection must not silently become a different catalog default.

use super::*;
use deepagent_app_core::HarnessRunOverrides;

const DEFAULT_MODEL: &str = "deepseek-v4-flash";
const KNOWN_OVERRIDE: &str = "deepseek-v4-pro";

#[derive(Default)]
struct SelectionModel(Mutex<Vec<String>>);

#[async_trait]
impl HttpTransport for SelectionModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        self.0
            .lock()
            .unwrap()
            .push(body["model"].as_str().expect("wire model").to_string());
        emit_responses_events(sink, &response_text("AUDIT_MODEL_SELECTION_COMPLETE"))
    }
}

struct SelectionOutcome {
    error: Option<String>,
    requested_models: Vec<String>,
    terminal_kind: String,
}

async fn observe_model_selection(overrides: HarnessRunOverrides) -> SelectionOutcome {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    assert_eq!(
        settings.load().unwrap().unwrap().catalog.chat_model,
        DEFAULT_MODEL
    );
    let model = Arc::new(SelectionModel::default());
    let chat = ChatService::new(db, settings, model.clone(), root.path());
    let result = chat
        .run_in_session_with_overrides(
            "Audit explicit model selection.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-explicit-model-selection"),
            overrides,
            |_| {},
            |_| {},
        )
        .await;
    let events = chat
        .run_events("audit-explicit-model-selection", None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1, "one durable terminal per run");
    let terminal_kind = terminals[0].data["kind"].as_str().unwrap().to_string();
    assert_eq!(
        terminal_kind,
        if result.is_ok() {
            "succeeded"
        } else {
            "failed"
        }
    );
    let requested_models = model.0.lock().unwrap().clone();
    SelectionOutcome {
        error: result.err().map(|error| error.to_string()),
        requested_models,
        terminal_kind,
    }
}

#[tokio::test]
async fn default_and_known_explicit_model_selection_reach_the_intended_wire_model() {
    for (selected, expected) in [
        (None, DEFAULT_MODEL),
        (Some(KNOWN_OVERRIDE), KNOWN_OVERRIDE),
    ] {
        let outcome = observe_model_selection(HarnessRunOverrides {
            model: selected.map(str::to_string),
            ..Default::default()
        })
        .await;
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        assert_eq!(outcome.requested_models, vec![expected]);
        assert_eq!(outcome.terminal_kind, "succeeded");
    }
}

#[tokio::test]
async fn unsupported_explicit_provider_fails_before_any_model_request() {
    let outcome = observe_model_selection(HarnessRunOverrides {
        provider: Some("audit-unsupported-provider".to_string()),
        ..Default::default()
    })
    .await;
    assert!(outcome
        .error
        .as_deref()
        .is_some_and(|error| error.contains("unsupported provider")));
    assert!(outcome.requested_models.is_empty());
    assert_eq!(outcome.terminal_kind, "failed");
}

#[tokio::test]
#[ignore = "audit H-89: an explicit unknown model is silently replaced by the catalog default"]
async fn audit_unknown_explicit_model_is_rejected_or_sent_without_substitution() {
    const REQUESTED: &str = "deepseek-audit-not-in-catalog";
    let outcome = observe_model_selection(HarnessRunOverrides {
        model: Some(REQUESTED.to_string()),
        ..Default::default()
    })
    .await;
    eprintln!(
        "requested={REQUESTED} wire_models={:?} error_present={} terminal={}",
        outcome.requested_models,
        outcome.error.is_some(),
        outcome.terminal_kind,
    );
    // A provider may accept a newly published model before discovery catches up.
    // Either explicit rejection or exact dispatch is valid; silent substitution is not.
    let rejected_before_dispatch = outcome.requested_models.is_empty()
        && outcome
            .error
            .as_deref()
            .is_some_and(|error| error.contains("model"));
    let dispatched_exact_model = outcome.requested_models == [REQUESTED];
    assert!(
        rejected_before_dispatch || dispatched_exact_model,
        "explicit model selection was silently replaced"
    );
}
