/// H-157: explicit effort is checked for every physical model attempt.
mod h157_tests {
    use super::*;
    use deepagent_models::discovery::ModelEffortMetadata;
    use deepagent_models::ModelCapabilityResolver;

    fn config(primary_levels: &[&str]) -> ModelConfig {
        let mut config = ModelConfig::deepseek("synthetic");
        let mut capability = ModelCapabilityResolver::new().resolve_model_id("primary");
        capability.effort = Some(ModelEffortMetadata {
            supported_levels: primary_levels
                .iter()
                .map(|level| (*level).to_owned())
                .collect(),
            default_level: primary_levels.first().map(|level| (*level).to_owned()),
        });
        config.capabilities = vec![capability];
        config
    }

    #[tokio::test]
    async fn provider_default_is_omitted_on_wire_and_reported_as_default() {
        #[derive(Default)]
        struct Events(Mutex<Vec<RuntimeEvent>>);
        impl RuntimeEventSink for Events {
            fn emit(&self, event: RuntimeEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("default")])),
            requests: Mutex::new(Vec::new()),
        });
        let mut config = ModelConfig::deepseek("synthetic").with_model_reasoning_defaults(true);
        let mut capability = ModelCapabilityResolver::new().resolve_model_id("unknown");
        capability.thinking = Some(deepagent_models::thinking::ThinkingCapabilities::default());
        config.capabilities = vec![capability];
        let events = Arc::new(Events::default());
        let mut agent = ModelAgent::new(
            Arc::new(ModelClient::new(transport.clone(), config)),
            "unknown",
            "synthetic",
            "synthetic",
            vec![],
        )
        .with_events(events.clone());
        let _ = complete_items(agent.think(0, &[]).await.unwrap());
        assert!(transport.requests.lock().unwrap()[0]
            .get("reasoning")
            .is_none());
        assert!(events.0.lock().unwrap().iter().any(|event| matches!(event,
            RuntimeEvent::ModelRequestStarted { reasoning_effort, .. } if reasoning_effort == "default")));
    }

    #[tokio::test]
    async fn explicit_unsupported_effort_never_retries() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("not dispatched")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(transport.clone(), config(&["low"])));
        let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
            .with_reasoning_effort(ReasoningEffort::High)
            .with_max_model_attempts(6);

        let error = agent.think(0, &[]).await.unwrap_err();
        assert_eq!(
            classify_model_error(&error),
            ModelFailureKind::InvalidRequest
        );
        assert!(!classify_model_error(&error).retryable());
        assert!(error
            .to_string()
            .contains("does not support reasoning effort"));
        assert!(transport.requests.lock().unwrap().is_empty());
        assert_eq!(transport.attempts.lock().unwrap().len(), 1);
        assert_eq!(agent.model, "primary");
    }

    #[tokio::test]
    async fn model_default_selection_survives_retry_and_next_step() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_503__".into()],
                response_text_completed("recovered"),
                response_text_completed("next"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let mut config = config(&["low"]).with_model_reasoning_defaults(true);
        config.defaults.reasoning_effort = Some(ReasoningEffort::High);
        let client = Arc::new(ModelClient::new(transport.clone(), config));
        let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![]);
        let _ = complete_items(agent.think(0, &[]).await.unwrap());
        assert_eq!(agent.reasoning_effort, ReasoningEffort::Low);
        assert!(agent.requested_reasoning_effort.is_none());
        let _ = complete_items(agent.think(1, &[]).await.unwrap());
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        for request in requests.iter() {
            assert_eq!(request["model"], "primary");
            assert_eq!(request["reasoning"]["effort"], "low");
            assert!(request.get("prefer_model_reasoning_default").is_none());
            assert!(request.get("requested_reasoning_effort").is_none());
        }
    }

    #[tokio::test]
    async fn explicit_none_remains_a_thinking_off_control_for_an_empty_level_list() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("off")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(transport.clone(), config(&[])));
        let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
            .with_reasoning_effort(ReasoningEffort::None);
        let _ = complete_items(agent.think(0, &[]).await.unwrap());
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["model"], "primary");
        assert_eq!(requests[0]["reasoning"]["effort"], "none");
    }
}
