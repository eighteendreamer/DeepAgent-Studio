//! H-54 historical repair: resolved settings survive retry and a real tool turn.
use super::*;
use deepagent_app_core::{settings::ResponsesApiSettings, HarnessRunOverrides, RuntimeEvent};
use deepagent_models::ReasoningEffort;

#[derive(Default)]
struct Attempts(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for Attempts {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let attempt = {
            let mut requests = self.0.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len()
        };
        let events = match attempt {
            1 => {
                return Err(CoreError::provider(
                    Some(429),
                    Some("rate_limit_exceeded".into()),
                    "H54 synthetic rate limit",
                ));
            }
            2 => response_function_call(
                "h54-read",
                "read_file",
                serde_json::json!({"path":"control.txt"}),
            ),
            3 => response_text("H54 complete"),
            _ => panic!("unexpected model attempt {attempt}"),
        };
        let sink: &mut dyn EventSink = &mut crate::sequence_fixture::sink(sink);
        emit_responses_events(sink, &events)
    }
}

async fn configured_settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","context_window":1000000,"max_output_tokens":131072}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("h54-synthetic").await.unwrap();
    settings
        .set_reasoning_effort(ReasoningEffort::None)
        .unwrap();
    settings
        .set_responses_settings(ResponsesApiSettings {
            reasoning_effort: Some("high".into()),
            max_output_tokens: Some(512),
            ..Default::default()
        })
        .unwrap();
    settings
        .set_responses_developer_json(r#"{"reasoning":{"effort":"max"},"max_output_tokens":768}"#)
        .unwrap();
    settings
}

#[tokio::test]
async fn h54_effective_parameters_survive_project_run_precedence_retry_and_tool_turn() {
    for (project_key, run_effort, expected) in [
        (None, None, "max"),
        (Some("reasoningEffort"), None, "low"),
        (Some("reasoning_effort"), Some("none"), "none"),
    ] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("control.txt"), "H54 synthetic control").unwrap();
        if let Some(key) = project_key {
            std::fs::create_dir(root.path().join(".deepagent")).unwrap();
            std::fs::write(
                root.path().join(".deepagent/settings.json"),
                serde_json::json!({(key):"low"}).to_string(),
            )
            .unwrap();
        }
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = configured_settings(db.clone()).await;
        let transport = Arc::new(Attempts::default());
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let captured = snapshots.clone();
        let chat = ChatService::new(db, settings, transport.clone(), root.path());
        chat.run_in_session_with_overrides(
            "Read control.txt and acknowledge it.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some("h54-effective-parameters"),
            HarnessRunOverrides {
                reasoning_effort: run_effort.map(str::to_owned),
                ..Default::default()
            },
            move |event| {
                if let RuntimeEvent::ContextUsage { snapshot } = event {
                    captured
                        .lock()
                        .unwrap()
                        .push(snapshot.reserved_output_tokens);
                }
            },
            |_| panic!("a synthetic workspace read must not request approval"),
        )
        .await
        .unwrap();
        let requests = transport.0.lock().unwrap();
        assert_eq!(requests.len(), 3, "one retry and one tool continuation");
        for body in requests.iter() {
            assert_eq!(body["reasoning"]["effort"], expected);
            assert_eq!(body["max_output_tokens"], 768);
        }
        assert!(requests[2]["input"].as_array().unwrap().iter().any(|item| {
            item["type"] == "function_call_output"
                && item["call_id"] == "h54-read"
                && item["output"].to_string().contains("H54 synthetic control")
        }));
        let snapshots = snapshots.lock().unwrap();
        assert!(!snapshots.is_empty());
        assert!(snapshots.iter().all(|value| *value == 768));
        let events = chat.run_events("h54-effective-parameters", None).unwrap();
        let terminals: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .collect();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].data["kind"], "succeeded");
        assert_eq!(
            std::fs::read_to_string(root.path().join("control.txt")).unwrap(),
            "H54 synthetic control"
        );
    }
}

#[tokio::test]
async fn h54_invalid_run_effort_is_rejected_before_transport() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = configured_settings(db.clone()).await;
    let transport = Arc::new(Attempts::default());
    let chat = ChatService::new(db, settings, transport.clone(), root.path());
    for value in ["medium", "simple", "deep", "xhigh", "off"] {
        let error = chat
            .run_in_session_with_overrides(
                "This invalid run must not dispatch.",
                None,
                None,
                None,
                vec![],
                None,
                false,
                None,
                HarnessRunOverrides {
                    reasoning_effort: Some(value.into()),
                    ..Default::default()
                },
                |_| {},
                |_| panic!("invalid model configuration must fail before tools"),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("reasoning effort"), "{error}");
        assert!(transport.0.lock().unwrap().is_empty());
    }
}
