mod h49_tests {
    use super::*;
    use deepagent_models::{CapabilitySource, ModelCapability};

    fn capability(model: &str, window: usize, limit: usize) -> ModelCapability {
        ModelCapability {
            model_id: model.into(),
            effort: None,
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
        fallback_limit: usize,
    ) -> (
        ModelAgent,
        Arc<AttemptTransport>,
        tokio::sync::mpsc::UnboundedReceiver<RuntimeEvent>,
    ) {
        let primary = capability("primary", 1_000_000, first_limit);
        let mut config = ModelConfig::deepseek("synthetic");
        config.defaults.max_output_tokens = Some(32_768.min(first_limit as u32));
        config.capabilities = vec![
            primary.clone(),
            capability("fallback", 128_000, fallback_limit),
        ];
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
        .with_fallback_model(Some("fallback".into()))
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
    async fn h49_fallback_and_next_step_share_wire_and_budget_ceiling() {
        let (mut agent, transport, mut rx) = configured_agent(
            vec![
                vec!["__ERROR_OVERLOADED__".into()],
                response_text_completed("fallback"),
                response_text_completed("next"),
            ],
            32_768,
            8_192,
        );
        agent.think(0, &[]).await.unwrap();
        agent.think(1, &[]).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0]["max_output_tokens"], 32_768);
        for body in &requests[1..] {
            assert_eq!(body["model"], "fallback");
            assert_eq!(body["max_output_tokens"], 8_192);
        }
        let snapshots: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|event| match event {
                RuntimeEvent::ContextUsage { snapshot } if snapshot.model_id == "fallback" => {
                    Some(snapshot)
                }
                _ => None,
            })
            .collect();
        assert!(!snapshots.is_empty());
        assert!(snapshots
            .iter()
            .all(|s| s.context_window == 128_000 && s.reserved_output_tokens == 8_192));
    }

    #[tokio::test]
    async fn h49_truncation_then_fallback_rebounds_escalation_without_committing_old_tools() {
        let (mut agent, transport, _) = configured_agent(
            vec![
                response_text_incomplete("partial"),
                vec!["__ERROR_OVERLOADED__".into()],
                response_text_incomplete("continuation"),
                response_text_completed("done"),
            ],
            96_000,
            8_192,
        );
        let mut tools = RecordingToolAttempt::default();
        agent
            .think_streaming_cancelled(0, &[], None, Some(&mut tools))
            .await
            .unwrap();
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[0]["max_output_tokens"], 32_768);
        assert_eq!(requests[1]["max_output_tokens"], 65_536);
        assert!(requests[2..]
            .iter()
            .all(|r| r["model"] == "fallback" && r["max_output_tokens"] == 8_192));
        assert_eq!(
            tools
                .events
                .iter()
                .filter(|e| e.starts_with("commit:"))
                .count(),
            1
        );
        assert_eq!(tools.events.last().unwrap(), "commit:4");
    }

    #[tokio::test]
    async fn h49_small_model_uses_continuation_instead_of_impossible_escalation() {
        let (mut agent, transport, _) = configured_agent(
            vec![
                response_text_incomplete("partial"),
                response_text_completed("done"),
            ],
            4_096,
            8_192,
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
        let (agent, _, _) = configured_agent(vec![], 8_192, 4_096);
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
        let (mut agent, _, _) = configured_agent(vec![], 96_000, 8_192);
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
