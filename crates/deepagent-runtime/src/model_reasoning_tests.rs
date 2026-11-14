/// H-157: explicit effort is checked for every physical model attempt.
mod h157_tests {
    use super::*;
    use deepagent_models::discovery::ModelEffortMetadata;
    use deepagent_models::ModelCapabilityResolver;

    fn config(primary_levels: &[&str], fallback_levels: &[&str]) -> ModelConfig {
        let mut config = ModelConfig::deepseek("synthetic");
        config.capabilities = [("primary", primary_levels), ("fallback", fallback_levels)]
            .into_iter()
            .map(|(model, levels)| {
                let mut capability = ModelCapabilityResolver::new().resolve_model_id(model);
                capability.effort = Some(ModelEffortMetadata {
                    supported_levels: levels.iter().map(|level| (*level).to_owned()).collect(),
                    default_level: levels.first().map(|level| (*level).to_owned()),
                });
                capability
            })
            .collect();
        config
    }

    #[tokio::test]
    async fn explicit_unsupported_effort_never_retries_or_invokes_a_fallback() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("not dispatched")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            config(&["low"], &["high"]),
        ));
        let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
            .with_reasoning_effort(ReasoningEffort::High)
            .with_max_model_attempts(6)
            .with_fallback_model(Some("fallback".into()));

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
    async fn overload_fallback_rechecks_explicit_effort_without_silently_downgrading() {
        for supported in [false, true] {
            let transport = Arc::new(AttemptTransport {
                attempts: Mutex::new(VecDeque::from([
                    vec!["__ERROR_OVERLOADED__".into()],
                    response_text_completed("fallback"),
                ])),
                requests: Mutex::new(Vec::new()),
            });
            let fallback_levels: &[&str] = if supported {
                &["low", "high"]
            } else {
                &["low"]
            };
            let client = Arc::new(ModelClient::new(
                transport.clone(),
                config(&["high"], fallback_levels),
            ));
            let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
                .with_reasoning_effort(ReasoningEffort::High)
                .with_max_model_attempts(6)
                .with_fallback_model(Some("fallback".into()));
            let result = agent.think(0, &[]).await;
            if supported {
                let _ = complete_items(result.unwrap());
            } else {
                let error = result.unwrap_err();
                assert_eq!(
                    classify_model_error(&error),
                    ModelFailureKind::InvalidRequest
                );
                assert!(error.to_string().contains("fallback"));
                assert!(error
                    .to_string()
                    .contains("does not support reasoning effort"));
            }
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), if supported { 2 } else { 1 });
            assert_eq!(requests[0]["model"], "primary");
            assert!(requests
                .iter()
                .all(|request| request["reasoning"]["effort"] == "high"));
            if supported {
                assert_eq!(requests[1]["model"], "fallback");
            }
            assert_eq!(
                transport.attempts.lock().unwrap().len(),
                usize::from(!supported)
            );
        }
    }

    #[tokio::test]
    async fn explicit_none_remains_a_thinking_off_control_for_an_empty_level_list() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([response_text_completed("off")])),
            requests: Mutex::new(Vec::new()),
        });
        let client = Arc::new(ModelClient::new(transport.clone(), config(&[], &["high"])));
        let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
            .with_reasoning_effort(ReasoningEffort::None)
            .with_fallback_model(Some("fallback".into()));
        let _ = complete_items(agent.think(0, &[]).await.unwrap());
        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["model"], "primary");
        assert_eq!(requests[0]["reasoning"]["effort"], "none");
    }

    #[tokio::test]
    async fn model_default_selection_recomputes_on_fallback_and_next_step() {
        let transport = Arc::new(AttemptTransport {
            attempts: Mutex::new(VecDeque::from([
                vec!["__ERROR_OVERLOADED__".into()],
                response_text_completed("fallback"),
                response_text_completed("next"),
            ])),
            requests: Mutex::new(Vec::new()),
        });
        let mut config = config(&["high"], &["low"]).with_model_reasoning_defaults(true);
        config.defaults.reasoning_effort = Some(ReasoningEffort::High);
        let client = Arc::new(ModelClient::new(transport.clone(), config));
        let mut agent = ModelAgent::new(client, "primary", "synthetic", "synthetic", vec![])
            .with_fallback_model(Some("fallback".into()));
        let _ = complete_items(agent.think(0, &[]).await.unwrap());
        assert_eq!(agent.reasoning_effort, ReasoningEffort::Low);
        assert!(agent.requested_reasoning_effort.is_none());
        let _ = complete_items(agent.think(1, &[]).await.unwrap());
        let requests = transport.requests.lock().unwrap();
        let levels: Vec<_> = requests
            .iter()
            .map(|request| {
                (
                    request["model"].as_str().unwrap(),
                    request["reasoning"]["effort"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            levels,
            [
                ("primary", "high"),
                ("fallback", "low"),
                ("fallback", "low")
            ]
        );
        for request in requests.iter() {
            assert!(request.get("prefer_model_reasoning_default").is_none());
            assert!(request.get("requested_reasoning_effort").is_none());
        }
    }
}
