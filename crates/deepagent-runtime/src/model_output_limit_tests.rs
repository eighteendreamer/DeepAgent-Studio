mod h49_tests {
    use super::*;
    use deepagent_models::{CapabilitySource, ModelCapability};

    fn capability(model: &str, window: usize, limit: usize) -> ModelCapability {
        ModelCapability {
            model_id: model.into(),
            effort: None,
            thinking: None,
            context_window: window,
            max_output_tokens: limit,
            supports_tools: true,
            supports_thinking: true,
            supports_json_output: true,
            input_modalities: None,
            capability_source: CapabilitySource::ProviderMetadata,
            fallback_reason: None,
        }
    }

    fn configured_agent(
        events: Vec<Vec<String>>,
        first_limit: usize,
    ) -> (
        ModelAgent,
        Arc<AttemptTransport>,
        tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    ) {
        let primary = capability("primary", 1_000_000, first_limit);
        let mut config = ModelConfig::deepseek("synthetic");
        config.defaults.max_output_tokens = Some(32_768.min(first_limit as u32));
        config.capabilities = vec![primary.clone()];
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(events.into()),
            requests: Mutex::new(Vec::new()),
        });
        let (sink, rx) = crate::events::ChannelSink::new();
        let agent = ModelAgent::new(
            Arc::new(ModelClient::new(transport.clone(), config)),
            "primary",
            "synthetic",
            "synthetic",
            vec![],
        )
        .with_max_model_attempts(6)
        .with_context_policy(
            deepagent_context::ContextPolicy::for_capability(&primary, ReasoningEffort::Max),
            None,
            None,
        )
        .with_context_snapshot(snapshot_estimating(1, primary.context_window))
        .with_events(Arc::new(sink));
        (agent, transport, rx)
    }

    #[tokio::test]
    async fn h49_selected_model_retries_and_next_steps_share_wire_and_budget_ceiling() {
        let (mut agent, transport, mut rx) = configured_agent(
            vec![
                vec!["__ERROR_503__".into()],
                response_text_completed("recovered"),
                response_text_completed("next"),
            ],
            8_192,
        );
        agent.think(0, &[]).await.unwrap();
        agent.think(1, &[]).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests
            .iter()
            .all(|body| body["model"] == "primary" && body["max_output_tokens"] == 8_192));
        let snapshots: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                RuntimeEvent::ContextUsage { snapshot } => Some(snapshot),
                _ => None,
            })
            .collect();
        assert!(!snapshots.is_empty());
        assert!(snapshots.iter().all(|s| s.model_id == "primary"
            && s.context_window == 1_000_000
            && s.reserved_output_tokens == 8_192));
    }

    #[tokio::test]
    async fn h49_overload_during_truncation_recovery_aborts_without_another_attempt() {
        let (mut agent, transport, _) = configured_agent(
            vec![
                response_text_incomplete("partial"),
                vec!["__ERROR_OVERLOADED__".into()],
                response_text_completed("must not dispatch"),
            ],
            96_000,
        );
        let mut tools = RecordingToolAttempt::default();
        let error = agent
            .think_streaming_cancelled(0, &[], None, Some(&mut tools))
            .await
            .unwrap_err();
        assert_eq!(classify_model_error(&error), ModelFailureKind::Overloaded);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["max_output_tokens"], 32_768);
        assert_eq!(requests[1]["max_output_tokens"], 65_536);
        assert!(tools
            .events
            .iter()
            .all(|event| !event.starts_with("commit:")));
        assert_eq!(tools.events.last().unwrap(), "abort:2");
        assert_eq!(transport.attempts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn h49_small_model_uses_continuation_instead_of_impossible_escalation() {
        let (mut agent, transport, _) = configured_agent(
            vec![
                response_text_incomplete("partial"),
                response_text_completed("done"),
            ],
            4_096,
        );
        agent.think(0, &[]).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|r| r["max_output_tokens"] == 4_096));
        assert!(requests[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["content"] == MAX_OUTPUT_RECOVERY_PROMPT));
    }

    #[test]
    fn h49_policy_cannot_replace_client_capability_with_a_second_model_limit() {
        let (agent, _, _) = configured_agent(vec![], 8_192);
        let stale = capability("primary", 2_000_000, 384_000);
        let agent = agent.with_context_policy(
            deepagent_context::ContextPolicy::for_capability(&stale, ReasoningEffort::Max),
            None,
            None,
        );
        let actual = agent.current_context_policy().unwrap();
        assert_eq!(actual.context_window, 1_000_000);
        assert_eq!(actual.max_output_tokens, 8_192);
        assert_eq!(actual.reserved_output_tokens, 8_192);
    }

    #[test]
    fn h49_meter_fingerprint_uses_the_attempts_escalated_output_ceiling() {
        let (mut agent, _, _) = configured_agent(vec![], 96_000);
        agent.request_max_output_override = Some(65_536);
        let request = agent
            .response_history
            .request(agent.model.clone())
            .with_reasoning_effort(agent.reasoning_effort)
            .with_tools(agent.tools.clone())
            .with_max_output_tokens(65_536);
        assert_eq!(
            agent.context_measurement().0,
            agent.measure_request(&request).0
        );
        assert_eq!(
            agent
                .current_context_policy()
                .unwrap()
                .reserved_output_tokens,
            65_536
        );
    }
}

mod h296_tests {
    use super::*;
    use deepagent_models::discovery::{ModelCatalog, ModelInfo, ModelRole, DEEPSEEK_BASE_URL};

    #[tokio::test]
    async fn h296_selected_partial_metadata_reaches_wire_budget_and_next_step() {
        let models: Vec<ModelInfo> = serde_json::from_value(serde_json::json!([
            {"id":"audit-capacity-model","max_output_tokens":8_192}
        ]))
        .unwrap();
        let catalog = ModelCatalog::auto_select(DEEPSEEK_BASE_URL, models).unwrap();
        let mut config = ModelConfig::from_catalog("synthetic-h296", &catalog, ModelRole::Chat);
        config.defaults.max_output_tokens = Some(32_768);
        let selected = config.capability("audit-capacity-model");
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                response_text_completed("selected"),
                response_text_completed("next"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let (sink, mut rx) = crate::events::ChannelSink::new();
        let mut agent = ModelAgent::new(
            Arc::new(ModelClient::new(transport.clone(), config)),
            "audit-capacity-model",
            "synthetic",
            "synthetic",
            vec![],
        )
        .with_context_policy(
            deepagent_context::ContextPolicy::for_capability(&selected, ReasoningEffort::High),
            None,
            None,
        )
        .with_context_snapshot(snapshot_estimating(1, selected.context_window))
        .with_events(Arc::new(sink));
        agent.think(0, &[]).await.unwrap();
        agent.think(1, &[]).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests
            .iter()
            .all(|body| body["model"] == "audit-capacity-model"
                && body["max_output_tokens"] == 8_192));
        let snapshots: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                RuntimeEvent::ContextUsage { snapshot } => Some(snapshot),
                _ => None,
            })
            .collect();
        assert!(!snapshots.is_empty());
        assert!(snapshots
            .iter()
            .all(|s| s.model_id == "audit-capacity-model"
                && s.context_window == 128_000
                && s.reserved_output_tokens == 8_192));
    }
}
