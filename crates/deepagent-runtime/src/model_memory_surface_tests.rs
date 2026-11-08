// H-53: use the real agent/serializer with only transport and retrieval offline.
async fn h53_wait_for_prefetch(agent: &ModelAgent) {
    let handle = agent
        .memory_prefetch_handle
        .as_ref()
        .expect("prefetch scheduled");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn h53_agent_with_memory() -> (ModelAgent, Arc<AttemptTransport>) {
    let transport = Arc::new(AttemptTransport {
        attempts: Mutex::new(
            (0..10)
                .map(|index| {
                    response_function_call_done(
                        &format!("h53-{index}"),
                        "read_file",
                        r#"{"path":"synthetic"}"#,
                    )
                })
                .collect(),
        ),
        requests: Mutex::new(Vec::new()),
    });
    let mut agent = ModelAgent::new(
        Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic"),
        )),
        "model",
        "rules",
        "review synthetic deployment settings",
        vec![],
    )
    .with_relevant_memory_provider(Arc::new(ScriptedMemoryProvider {
        mems: vec![RelevantMemory {
            id: "project:h53".into(),
            block: "H53_VISIBLE_EXCERPT".into(),
        }],
        calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }));
    agent.think(0, &[]).await.unwrap();
    h53_wait_for_prefetch(&agent).await;
    agent.think(1, &[]).await.unwrap();
    assert!(transport.requests.lock().unwrap()[1]["input"]
        .to_string()
        .contains("H53_VISIBLE_EXCERPT"));
    (agent, transport)
}

fn h53_adopt(agent: &mut ModelAgent, items: Vec<ResponseInputItem>, native: bool) {
    let messages = deepagent_models::messages_from_response_items(Some("rules"), &items);
    agent.adopt_compaction(
        ReactiveCompaction {
            messages,
            response_items: native.then_some(items),
            tokens_before: 5000,
            tokens_after: 100,
            summary: "synthetic summary".into(),
            usage: Default::default(),
        },
        deepagent_core::compaction_strategy::PROACTIVE_THRESHOLD,
    );
}

#[tokio::test]
async fn h53_removed_memory_retries_even_when_current_query_is_unchanged() {
    for native in [false, true] {
        let (mut agent, transport) = h53_agent_with_memory().await;
        h53_adopt(
            &mut agent,
            vec![ResponseInputItem::Message {
                role: "user".into(),
                content: "review synthetic deployment settings".into(),
            }],
            native,
        );
        agent.think(2, &[]).await.unwrap();
        h53_wait_for_prefetch(&agent).await;
        agent.think(3, &[]).await.unwrap();
        assert!(transport.requests.lock().unwrap()[3]["input"]
            .to_string()
            .contains("H53_VISIBLE_EXCERPT"));
    }
}

#[tokio::test]
async fn h53_retained_tail_keeps_memory_identity_and_repeated_adoption_is_idempotent() {
    for native in [false, true] {
        let (mut agent, _) = h53_agent_with_memory().await;
        let retained = agent.response_history.items.iter().find(|item| {
            matches!(item, ResponseInputItem::Message { content, .. } if content.contains("H53_VISIBLE_EXCERPT"))
        }).unwrap().clone();
        for _ in 0..2 {
            h53_adopt(&mut agent, vec![retained.clone()], native);
            assert_eq!(
                agent
                    .surfaced_memory_ids
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>(),
                vec!["project:h53"]
            );
        }
    }
}

#[tokio::test]
async fn h53_snip_releases_only_removed_memory_and_resets_query_gate() {
    let (mut agent, _) = h53_agent_with_memory().await;
    agent = agent.with_snip_tool("snip_history");
    agent.push_message(Message::user("protected fresh query [id:u2]"));
    for index in 0..10 {
        agent.push_message(Message::assistant(format!("protected filler {index}")));
    }
    agent.apply_snip_from_output(&serde_json::json!({"ids":["u1"]}));
    assert!(!agent
        .conversation()
        .iter()
        .any(|m| m.content.contains("H53_VISIBLE_EXCERPT")));
    assert!(agent.surfaced_memory_ids.is_empty());
    assert!(agent.last_prefetch_query.is_none());
}

#[tokio::test]
async fn h53_adoption_discards_prefetch_based_on_the_old_exclusion_snapshot() {
    let (mut agent, _) = h53_agent_with_memory().await;
    agent.push_message(Message::user("another synthetic deployment query"));
    agent.maybe_schedule_memory_prefetch();
    assert!(agent.memory_prefetch_handle.is_some());
    h53_adopt(
        &mut agent,
        vec![ResponseInputItem::Message {
            role: "user".into(),
            content: "another synthetic deployment query".into(),
        }],
        true,
    );
    assert!(
        agent.memory_prefetch_handle.is_none(),
        "old exclusions must not survive adoption"
    );
    agent.maybe_schedule_memory_prefetch();
    h53_wait_for_prefetch(&agent).await;
    agent.think(2, &[]).await.unwrap();
    assert!(agent.surfaced_memory_ids.contains("project:h53"));
}

#[tokio::test]
async fn h53_ids_without_visible_bodies_do_not_consume_quota() {
    let (mut agent, _) = h53_agent_with_memory().await;
    agent.seed_surfaced_memories(&["project:absent".into()]);
    assert!(!agent.surfaced_memory_ids.contains("project:absent"));
    assert!(agent.surfaced_memory_ids.contains("project:h53"));
}

#[tokio::test]
async fn h53_identity_and_body_reach_both_production_wire_adapters() {
    struct Recorder(Mutex<Vec<serde_json::Value>>);
    #[async_trait]
    impl HttpTransport for Recorder {
        async fn stream(&self, request: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(serde_json::from_str(&request.body)?);
            Err(CoreError::other("h53 synthetic wire recorded"))
        }
    }
    let item = deepagent_context::memory_attachment::memory_attachment(&[RelevantMemory {
        id: "project:wire".into(),
        block: "H53_TWO_PROTOCOLS 记忆".into(),
    }])
    .unwrap();
    for mode in [
        deepagent_models::WireMode::Responses,
        deepagent_models::WireMode::ChatCompletions,
    ] {
        let transport = Arc::new(Recorder(Mutex::new(Vec::new())));
        let client = ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        );
        let request =
            ResponseRequest::from_response_items("deepseek-v4-flash", None, vec![item.clone()]);
        assert_eq!(
            client
                .stream_response(request)
                .await
                .unwrap_err()
                .to_string(),
            "h53 synthetic wire recorded"
        );
        let bodies = transport.0.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        let messages = bodies[0]
            .get("input")
            .or_else(|| bodies[0].get("messages"))
            .unwrap();
        assert_eq!(messages[0]["role"], "user");
        let ResponseInputItem::Message { content, .. } = &item else {
            unreachable!()
        };
        assert_eq!(messages[0]["content"], *content);
    }
}

#[tokio::test]
async fn h53_partial_retention_keeps_one_identity_and_releases_remaining_budget() {
    use deepagent_context::memory_attachment::memory_attachment;
    let (mut agent, transport) = h53_agent_with_memory().await;
    let memories: Vec<_> = (0..MAX_SURFACED_MEMORIES_PER_RUN)
        .map(|index| RelevantMemory {
            id: format!("project:retained-{index}"),
            block: format!("H53_PARTIAL_EXCERPT_{index}"),
        })
        .collect();
    h53_adopt(
        &mut agent,
        vec![memory_attachment(&memories).unwrap()],
        true,
    );
    assert_eq!(
        agent.surfaced_memory_ids.len(),
        MAX_SURFACED_MEMORIES_PER_RUN
    );
    agent.maybe_schedule_memory_prefetch();
    assert!(agent.memory_prefetch_handle.is_none());
    h53_adopt(
        &mut agent,
        vec![
            memory_attachment(&memories[..1]).unwrap(),
            ResponseInputItem::Message {
                role: "user".into(),
                content: "review synthetic deployment settings".into(),
            },
        ],
        true,
    );
    assert_eq!(agent.surfaced_memory_ids.len(), 1);
    agent.think(2, &[]).await.unwrap();
    h53_wait_for_prefetch(&agent).await;
    agent.think(3, &[]).await.unwrap();
    assert_eq!(agent.surfaced_memory_ids.len(), 2);
    let wire = transport.requests.lock().unwrap()[3]["input"].to_string();
    assert!(wire.contains("H53_PARTIAL_EXCERPT_0"));
    assert!(!wire.contains("H53_PARTIAL_EXCERPT_1"));
    assert!(wire.contains("H53_VISIBLE_EXCERPT"));
}

struct H53UnsupportedPersistence;
impl ContextSurfacePersistence for H53UnsupportedPersistence {
    fn replace_tool_results(&self, _: &str, _: &[(String, String)]) -> Result<()> {
        Ok(())
    }
    fn record_snip(&self, _: &[String]) -> Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn h53_real_pressure_gate_compacts_then_allows_retrieval_on_the_next_surface() {
    let (mut agent, transport) = h53_agent_with_memory().await;
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    agent = agent
        .with_reactive_compactor(Arc::new(StaticReactiveCompactor {
            calls: calls.clone(),
        }))
        .with_proactive_compaction(1);
    agent.push_message(Message::user("review synthetic deployment settings again"));
    agent.think(2, &[]).await.unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::Acquire), 1);
    assert!(agent.surfaced_memory_ids.is_empty());
    assert!(!transport.requests.lock().unwrap()[2]["input"]
        .to_string()
        .contains("H53_VISIBLE_EXCERPT"));
    agent = agent.with_proactive_compaction(10_000_000);
    agent.think(3, &[]).await.unwrap();
    h53_wait_for_prefetch(&agent).await;
    agent.think(4, &[]).await.unwrap();
    assert!(transport.requests.lock().unwrap()[4]["input"]
        .to_string()
        .contains("H53_VISIBLE_EXCERPT"));
}

#[tokio::test]
async fn h53_attachment_persistence_failure_stops_adoption_and_model_dispatch() {
    let (mut agent, transport) = h53_agent_with_memory().await;
    agent.drain_memory_injections();
    agent = agent.with_surface_persistence(Arc::new(H53UnsupportedPersistence));
    h53_adopt(
        &mut agent,
        vec![ResponseInputItem::Message {
            role: "user".into(),
            content: "review synthetic deployment settings".into(),
        }],
        true,
    );
    agent.think(2, &[]).await.unwrap();
    h53_wait_for_prefetch(&agent).await;
    let before = transport.requests.lock().unwrap().len();
    let error = agent.think(3, &[]).await.unwrap_err();
    assert!(error
        .to_string()
        .contains("Surface persistence does not support memory attachments"));
    assert_eq!(transport.requests.lock().unwrap().len(), before);
    assert!(agent.surfaced_memory_ids.is_empty());
    assert!(agent.drain_memory_injections().is_empty());
    assert!(!agent
        .conversation()
        .iter()
        .any(|m| m.content.contains("H53_VISIBLE_EXCERPT")));
}
