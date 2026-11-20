//! H-49: saved settings/catalog -> RunAssembler -> real wire and run reopen.
use super::*;
use deepagent_app_core::settings::ResponsesApiSettings;
use deepagent_persistence::run_store::RunStore;

const PRIMARY: &str = "deepseek-v4-flash";
const FALLBACK: &str = "deepseek-v4-pro";

struct CapabilityGate {
    fallback_limit: u32,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait]
impl HttpTransport for CapabilityGate {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        self.requests.lock().unwrap().push(body.clone());
        assert_eq!(body["model"], FALLBACK);
        if body["max_output_tokens"].as_u64().unwrap() > u64::from(self.fallback_limit) {
            return Err(CoreError::provider(
                Some(400),
                Some("invalid_request_error".into()),
                "synthetic capability rejection",
            ));
        }
        emit_responses_events(sink, &[serde_json::json!({"type":"response.completed","response":{
            "id":"h49-fixture", "object":"response", "created_at":1, "model":FALLBACK,
            "status":"completed", "output":[{"type":"message","id":"h49-answer","role":"assistant",
                "content":[{"type":"output_text","text":"H49 synthetic complete","annotations":[]}]}]
        }}).to_string()])
    }
}

#[tokio::test]
async fn h49_selected_model_matches_wire_context_and_reopened_terminals() {
    for limit in [8_192, 32_768] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("synthetic.sqlite");
        let db = Arc::new(Database::open(&path).unwrap());
        let catalog = serde_json::json!({"data":[
            {"id":PRIMARY,"context_window":1_000_000,"max_output_tokens":32_768},
            {"id":FALLBACK,"context_window":128_000,"max_output_tokens":limit}
        ]});
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(catalog.to_string())),
            Arc::new(MemorySecretStore::new()),
        ));
        crate::registry_fixture::configure(&settings, "h49-synthetic-control")
            .await
            .unwrap();
        model_selection::activate_selection_model(&db, &settings, FALLBACK);
        settings
            .harness_models()
            .select_model("deepseek", FALLBACK)
            .unwrap();
        settings
            .set_responses_settings(ResponsesApiSettings {
                max_output_tokens: Some(32_768),
                ..Default::default()
            })
            .unwrap();
        let transport = Arc::new(CapabilityGate {
            fallback_limit: limit,
            requests: Mutex::new(Vec::new()),
        });
        let chat = ChatService::new(db.clone(), settings.clone(), transport.clone(), root.path());
        let session = chat.create_session(None).unwrap();
        for turn in 0..2 {
            let snapshots = Arc::new(Mutex::new(Vec::new()));
            let observed = snapshots.clone();
            chat.run_in_session(
                "Acknowledge the synthetic control.",
                Some(&session),
                None,
                None,
                vec![],
                None,
                false,
                Some(&format!("h49-{turn}")),
                move |event| {
                    if let deepagent_runtime::RuntimeEvent::ContextUsage { snapshot } = event {
                        observed.lock().unwrap().push(snapshot);
                    }
                },
                |_| {},
            )
            .await
            .unwrap();
            let snapshots = snapshots.lock().unwrap();
            let fallback = snapshots
                .iter()
                .rev()
                .find(|s| s.model_id == FALLBACK)
                .unwrap();
            assert_eq!(fallback.context_window, 128_000);
            assert_eq!(fallback.reserved_output_tokens, limit as usize);
        }
        let calls = transport.requests.lock().unwrap().clone();
        assert_eq!(calls.len(), 2);
        for call in calls {
            assert_eq!(call["model"], FALLBACK);
            assert_eq!(call["max_output_tokens"], limit);
        }
        drop(chat);
        drop(settings);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        let store = RunStore::new(&reopened);
        for turn in 0..2 {
            let run = format!("h49-{turn}");
            assert_eq!(
                store.get(&run).unwrap().unwrap().terminal_kind.as_deref(),
                Some("succeeded")
            );
            assert_eq!(
                store
                    .events_after(&run, None)
                    .unwrap()
                    .iter()
                    .filter(|e| e.event_type == "run_terminal")
                    .count(),
                1
            );
        }
        assert_eq!(
            transport.requests.lock().unwrap().len(),
            2,
            "reopen/read must not dispatch again"
        );
    }
}
