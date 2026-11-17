// H-297: use the existing real attempt fixtures, serializer and provider anchor.

#[tokio::test]
async fn h297_calibration_tracks_unicode_deltas_and_invalidates_changed_headers() {
    for model in ["deepseek-v4-pro", "deepseek-flash"] {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed_with_usage(
                "合成回答 🧪",
                20_000,
                12,
                15_000,
            )])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic"),
        ));
        let mut agent = ModelAgent::new(
            client.clone(),
            model,
            "stable rules",
            "请核对原文。",
            vec![],
        )
        .with_reasoning_effort(ReasoningEffort::None);
        let (key, initial) = agent.context_measurement();
        assert!(initial.source.starts_with("offline_prompt_estimate:"));
        agent.think(0, &[]).await.unwrap();
        let (same_key, answered) = agent.context_measurement();
        assert_eq!(key, same_key);
        assert_eq!(
            agent.estimate_context_tokens(),
            (20_000 + answered.total() - initial.total()) as u64
        );
        agent.push_message(Message::user(
            "中文工具结果 🧪 {\"编号\":1234567890}".repeat(500),
        ));
        let (_, appended) = agent.context_measurement();
        assert!(appended.total() > answered.total());
        assert_eq!(
            agent.estimate_context_tokens(),
            (20_000 + appended.total() - initial.total()) as u64
        );
        agent
            .response_history
            .replace_items(vec![ResponseInputItem::Message {
                role: "user".into(),
                content: "replacement".into(),
            }]);
        let (replacement_key, replaced) = agent.context_measurement();
        assert_eq!(
            replacement_key, key,
            "history replacement uses a signed delta, not a new header"
        );
        assert_eq!(
            agent.estimate_context_tokens(),
            (20_000i128 + replaced.total() as i128 - initial.total() as i128).max(0) as u64
        );

        for change in 0..4 {
            match change {
                0 => agent.response_history.instructions = Some("changed stable rules".into()),
                1 => agent.request_max_output_override = Some(4096),
                2 => agent.tools.push(ToolSchema::function(
                    "check",
                    "检查",
                    serde_json::json!({"type":"object"}),
                )),
                3 => {
                    agent.model = if model == "deepseek-flash" {
                        "deepseek-v4-pro"
                    } else {
                        "deepseek-flash"
                    }
                    .into()
                }
                _ => unreachable!(),
            }
            let (changed, surface) = agent.context_measurement();
            assert_ne!(changed, key);
            assert_eq!(
                agent.estimate_context_tokens(),
                surface.total() as u64,
                "changed request cannot reuse old calibration"
            );
        }
        let mut cold = ModelAgent::new(
            client,
            &agent.model,
            "changed stable rules",
            "replacement",
            agent.tools.clone(),
        )
        .with_reasoning_effort(ReasoningEffort::None);
        cold.request_max_output_override = Some(4096);
        assert_eq!(
            cold.estimate_context_tokens(),
            cold.context_measurement().1.total() as u64
        );
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn h297_selected_model_prices_the_actual_tokenizer_before_dispatch() {
    struct Pressure(std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl ReactiveContextCompactor for Pressure {
        async fn compact(
            &self,
            _: &[Message],
            trigger: CompactionTrigger,
        ) -> Result<Option<ReactiveCompaction>> {
            assert_eq!(trigger, CompactionTrigger::AutoCompactThreshold);
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }
    }
    let pressure = Arc::new(Pressure(std::sync::atomic::AtomicUsize::new(0)));
    let transport = Arc::new(AttemptTransport {
        attempts: Mutex::new(VecDeque::from([response_text_completed("done")])),
        requests: Mutex::new(Vec::new()),
    });
    let mut config = ModelConfig::deepseek("synthetic");
    config.defaults.max_output_tokens = Some(8192);
    let mut selected =
        deepagent_models::ModelCapabilityResolver::new().resolve_model_id("deepseek-flash");
    selected.context_window = 128_000;
    selected.max_output_tokens = 8192;
    config.capabilities = vec![selected.clone()];
    let client = Arc::new(ModelClient::new(transport.clone(), config));
    let observed = pressure.clone();
    let mut agent = ModelAgent::new(
        client,
        "deepseek-flash",
        "rules",
        "🧪🔧📁✅".repeat(8000),
        vec![],
    )
    .with_reasoning_effort(ReasoningEffort::None)
    .with_context_policy(
        deepagent_context::ContextPolicy::for_capability(&selected, ReasoningEffort::None),
        None,
        None,
    )
    .with_reactive_compactor(pressure.clone())
    .with_tool_schema_observer(Arc::new(move |_, schemas| {
        assert_eq!(schemas.model, "deepseek-flash");
        assert_eq!(
            observed.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "pressure must be handled before dispatch"
        );
        Ok(())
    }));
    agent.think(0, &[]).await.unwrap();
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["model"], "deepseek-flash");
    assert!(agent.context_measurement().1.source.contains(":v41:"));
}
