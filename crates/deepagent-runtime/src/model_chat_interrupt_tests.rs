// Included in model_agent::tests to reuse the real serializer/attempt fixtures.
fn h144_usage() -> serde_json::Value {
    serde_json::json!({"prompt_tokens":9,"completion_tokens":2,"total_tokens":11,
        "prompt_cache_hit_tokens":4,"prompt_cache_miss_tokens":5,
        "completion_tokens_details":{"reasoning_tokens":1}})
}

fn h144_events(reason: &str, with_tool: bool) -> Vec<String> {
    let delta = if with_tool {
        serde_json::json!({"content":"interrupted partial","tool_calls":[{"index":0,"id":"interrupted-call","type":"function","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}]})
    } else if reason == "stop" {
        serde_json::json!({"content":"normal answer"})
    } else {
        serde_json::json!({})
    };
    vec![serde_json::json!({"choices":[{"index":0,"delta":delta,"finish_reason":reason}],"usage":h144_usage()}).to_string(),"[DONE]".into()]
}

fn h144_agent(attempts: Vec<Vec<String>>) -> (ModelAgent, Arc<AttemptTransport>) {
    let transport = Arc::new(AttemptTransport {
        attempts: Mutex::new(attempts.into()),
        requests: Mutex::new(Vec::new()),
    });
    let client = Arc::new(ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek_chat("synthetic"),
    ));
    (
        ModelAgent::new(client, "primary", "rules", "synthetic", vec![]).with_max_model_attempts(3),
        transport,
    )
}

#[tokio::test]
async fn h144_provider_abort_preserves_cause_aborts_tools_and_never_retries_or_falls_back() {
    for with_tool in [false, true] {
        let (mut agent, transport) = h144_agent(vec![
            h144_events("aborted", with_tool),
            h144_events("stop", false),
        ]);
        let mut tools = RecordingToolAttempt::default();
        let error = agent
            .think_streaming_cancelled(0, &[], None, Some(&mut tools))
            .await
            .unwrap_err();
        let usage = agent.cumulative_usage().unwrap();
        eprintln!(
            "H144 baseline abort: {error}; billed input={} output={}",
            usage.billed_prompt_tokens, usage.completion_tokens
        );
        assert!(
            matches!(&error,CoreError::Provider{code:Some(code),message,..} if code=="aborted" && !message.contains("unsupported")),
            "{error}"
        );
        assert_eq!(format!("{:?}", classify_model_error(&error)), "Aborted");
        assert_eq!(transport.requests.lock().unwrap().len(), 1);
        assert_eq!(tools.events, ["begin:1", "abort:1"]);
        assert!(tools.calls.is_empty());
        assert_eq!(
            (
                usage.billed_prompt_tokens,
                usage.completion_tokens,
                usage.billed_cache_hit_tokens,
                usage.billed_cache_miss_tokens,
                usage.reasoning_tokens
            ),
            (9, 2, 4, 5, 1)
        );
        assert_eq!(agent.take_pending_raw_usage(), [h144_usage()]);
    }
}

#[tokio::test]
async fn h144_resource_failure_aborts_partial_tools_and_bills_once_at_every_attempt_budget() {
    for with_tool in [false, true] {
        for limit in [0, 1, 2, 3] {
            let (agent, transport) = h144_agent(vec![
                h144_events("insufficient_system_resource", with_tool),
                h144_events("stop", false),
            ]);
            let mut agent = agent.with_max_model_attempts(limit);
            let mut tools = RecordingToolAttempt::default();
            let error = agent
                .think_streaming_cancelled(0, &[], None, Some(&mut tools))
                .await
                .unwrap_err();
            assert_eq!(classify_model_error(&error), ModelFailureKind::Overloaded);
            assert!(
                matches!(&error, CoreError::Provider { code: Some(code), .. }
                if code == "insufficient_system_resource")
            );
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(), 1, "limit={limit}: {error}");
            assert_eq!(requests[0]["model"], "primary");
            assert_eq!(tools.events, ["begin:1", "abort:1"]);
            assert!(tools.calls.is_empty());
            let routes = agent.model_usage();
            assert_eq!(routes.len(), 1);
            let usage = agent.cumulative_usage().unwrap();
            assert_eq!(
                (
                    usage.billed_prompt_tokens,
                    usage.completion_tokens,
                    usage.billed_cache_hit_tokens,
                    usage.billed_cache_miss_tokens,
                    usage.reasoning_tokens
                ),
                (9, 2, 4, 5, 1)
            );
            assert_eq!(agent.take_pending_raw_usage(), [h144_usage()]);
        }
    }
}

#[tokio::test]
async fn h144_local_cancel_interrupts_transport_backoff_without_a_second_physical_call() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Ready {
        calls: AtomicUsize,
        ready: tokio::sync::Semaphore,
    }
    #[async_trait]
    impl HttpTransport for Ready {
        async fn stream(&self, _: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            assert_eq!(
                self.calls.fetch_add(1, Ordering::SeqCst),
                0,
                "no request after local cancel"
            );
            self.ready.add_permits(1);
            sink.on_event(&chat_usage_chunk(9, 2))?;
            Err(CoreError::other("synthetic connection interrupted"))
        }
    }
    let transport = Arc::new(Ready {
        calls: AtomicUsize::new(0),
        ready: tokio::sync::Semaphore::new(0),
    });
    let client = Arc::new(ModelClient::new(
        transport.clone(),
        ModelConfig::deepseek_chat("synthetic"),
    ));
    let mut agent =
        ModelAgent::new(client, "primary", "rules", "synthetic", vec![]).with_max_model_attempts(3);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut tools = RecordingToolAttempt::default();
    let signal = async {
        transport.ready.acquire().await.unwrap().forget();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.store(true, Ordering::Release);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(
            agent.think_streaming_cancelled(0, &[], Some(cancel.clone()), Some(&mut tools)),
            signal
        )
    })
    .await
    .unwrap();
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("cancelled by user"));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(tools.events, ["begin:1", "abort:1"]);
    assert_eq!(agent.cumulative_usage().unwrap().billed_prompt_tokens, 9);
}

#[tokio::test]
async fn h144_attempt_budget_is_shared_by_retry_including_zero_and_one() {
    for server_control in [false, true] {
        for limit in [0, 1, 2, 3] {
            let fail = if server_control {
                vec!["__ERROR_503__".into()]
            } else {
                vec!["__ERROR_EOF__".into()]
            };
            let (mut agent, transport) =
                h144_agent(vec![fail.clone(), fail, h144_events("stop", false)]);
            agent = agent.with_max_model_attempts(limit);
            let result = agent.think(0, &[]).await;
            let requests = transport.requests.lock().unwrap();
            assert_eq!(requests.len(),limit.max(1),"retry is not an extra attempt beyond the shared budget; limit={limit}, server={server_control}");
            if limit < 3 {
                assert_eq!(
                    classify_model_error(&result.unwrap_err()),
                    if server_control {
                        ModelFailureKind::Server
                    } else {
                        ModelFailureKind::Transport
                    }
                );
            } else {
                assert_eq!(complete_items(result.unwrap()).0.content, "normal answer");
            }
        }
    }
}

#[tokio::test]
async fn h144_local_cancel_blocks_the_retry_before_physical_dispatch() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct CancelOnReset(Arc<AtomicBool>);
    impl RuntimeEventSink for CancelOnReset {
        fn emit(&self, event: RuntimeEvent) {
            if matches!(event, RuntimeEvent::ModelAttemptReset { .. }) {
                self.0.store(true, Ordering::Release);
            }
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let (agent, transport) = h144_agent(vec![
        vec![chat_usage_chunk(9, 2), "__ERROR_EOF__".into()],
        h144_events("stop", false),
    ]);
    let mut agent = agent.with_events(Arc::new(CancelOnReset(cancel.clone())));
    let mut tools = RecordingToolAttempt::default();
    let error = agent
        .think_streaming_cancelled(0, &[], Some(cancel.clone()), Some(&mut tools))
        .await
        .unwrap_err();
    assert!(cancel.load(Ordering::Acquire));
    assert!(error.to_string().contains("cancelled by user"));
    assert_eq!(transport.requests.lock().unwrap().len(), 1);
    assert_eq!(tools.events, ["begin:1", "abort:1"]);
    let routes = agent.model_usage();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].model_id, "primary");
    assert_eq!(routes[0].usage.billed_prompt_tokens, 9);
}
