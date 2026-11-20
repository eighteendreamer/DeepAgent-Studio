//! H166: real Kernel retry/tool continuation and durable sampling evidence.
use super::*;
use deepagent_app_core::{settings::ResponsesApiSettings, HarnessRunOverrides, RuntimeEvent};
use deepagent_persistence::run_store::RunStore;

#[derive(Default)]
struct SamplingAttempts(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for SamplingAttempts {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let attempt = {
            let mut requests = self.0.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len()
        };
        let events = match attempt {
            1 => {
                return Err(CoreError::provider(
                    Some(503),
                    Some("service_unavailable".into()),
                    "H166 synthetic transient failure",
                ))
            }
            2 => response_function_call(
                "h166-read",
                "read_file",
                serde_json::json!({"path":"control.txt"}),
            ),
            3 => response_text("H166 complete"),
            _ => panic!("unexpected H166 attempt {attempt}"),
        };
        let sink: &mut dyn EventSink = &mut crate::sequence_fixture::sink(sink);
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn h166_kernel_sampling_matches_wire_and_survives_sqlite_cold_reopen() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("run.db");
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("control.txt"), "H166 local fixture").unwrap();
    let transport = Arc::new(SamplingAttempts::default());
    let before = {
        let db = Arc::new(Database::open(&db_path).unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        crate::registry_fixture::configure(&settings, "h166-synthetic-placeholder")
            .await
            .unwrap();
        settings
            .harness_models()
            .select_model("deepseek", "deepseek-flash")
            .unwrap();
        db.seed_responses_settings(ResponsesApiSettings {
            top_p: Some(0.2),
            reasoning_effort: Some("none".into()),
            ..Default::default()
        })
        .unwrap();
        db.seed_responses_developer_json(r#"{"top_p":0.96,"reasoning":{"effort":"high"}}"#)
            .unwrap();
        let live = Arc::new(Mutex::new(Vec::new()));
        let captured = live.clone();
        let chat = ChatService::new(db, settings.clone(), transport.clone(), &workspace);
        chat.run_in_session_with_overrides(
            "Read control.txt then finish.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some("h166-sampling"),
            HarnessRunOverrides {
                model: Some("deepseek-flash".into()),
                ..Default::default()
            },
            move |event| {
                captured.lock().unwrap().push(event);
            },
            |_| panic!("a local read must not request approval"),
        )
        .await
        .unwrap();
        assert_eq!(settings.load().unwrap().unwrap().responses.top_p, Some(0.2));
        let events = chat.run_events("h166-sampling", None).unwrap();
        let resolved: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "model_sampling_resolved")
            .collect();
        let live_count = live
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ModelSamplingResolved { .. }))
            .count();
        assert_eq!(resolved.len(), 3);
        assert_eq!(live_count, 3);
        let requests = transport.0.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for (index, expected) in [0.96_f32; 3].into_iter().enumerate() {
            let data = &resolved[index].data;
            assert_eq!(
                data["sampling"]["configured_top_p"].as_f64().unwrap() as f32,
                0.96
            );
            assert_eq!(
                data["sampling"]["effective_top_p"].as_f64().unwrap() as f32,
                expected
            );
            assert_eq!(requests[index]["top_p"].as_f64().unwrap() as f32, expected);
            assert_eq!(data["model"], requests[index]["model"]);
            let typed: RuntimeEvent = serde_json::from_value(data.clone()).unwrap();
            assert!(matches!(typed, RuntimeEvent::ModelSamplingResolved { .. }));
            assert!(!data.to_string().contains("h166-synthetic-placeholder"));
        }
        assert_eq!(requests[0]["model"], "deepseek-flash");
        assert!(requests
            .iter()
            .all(|request| request["model"] == "deepseek-flash"));
        assert!(requests[2]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "function_call_output" && item["call_id"] == "h166-read"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.event_type == "run_terminal")
                .count(),
            1
        );
        events
    };
    let reopened = Database::open(&db_path).unwrap();
    let after = RunStore::new(&reopened)
        .events_after("h166-sampling", None)
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        std::fs::read_to_string(workspace.join("control.txt")).unwrap(),
        "H166 local fixture"
    );
}
