// Exercise pressure, UI and dispatch pricing through the real serializers.

fn measurement_agent() -> (ModelAgent, Arc<std::sync::atomic::AtomicUsize>) {
    let computations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = computations.clone();
    let agent = ModelAgent::new(
        client(vec![]),
        "third-party-model",
        "stable rules",
        "synthetic input",
        vec![ToolSchema::function(
            "inspect",
            "Inspect a file",
            serde_json::json!({"type":"object","properties":{"path":{"type":"string"}}}),
        )],
    )
    .with_context_snapshot(snapshot_estimating(1, 1_000_000))
    .with_preparation_observer(Arc::new(move |phase, _| {
        if phase == "model_agent_context_measurement_computed" {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }));
    (agent, computations)
}

#[test]
fn identical_pressure_usage_and_dispatch_share_request_measurement() {
    let (mut agent, computations) = measurement_agent();
    let pressure = agent.estimate_context_tokens();
    agent.publish_context_usage();
    let request = agent
        .request_for_current_history()
        .unwrap()
        .with_tools(agent.tools.clone());
    let (fingerprint, surface) = agent.measure_request(&request);
    assert_eq!(pressure, surface.total() as u64);
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Provider calibration is live state; only the local projection is reused.
    agent
        .context_meter
        .anchor(fingerprint, surface.total(), 1234);
    agent.publish_context_usage();
    assert_eq!(agent.estimate_context_tokens(), 1234);
    assert_eq!(
        agent
            .context_snapshot
            .as_ref()
            .unwrap()
            .projected_request_tokens,
        Some(1234)
    );
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn request_measurement_refreshes_after_each_request_dependency_changes() {
    let (agent, computations) = measurement_agent();
    let original = agent
        .response_history
        .request(agent.model.clone())
        .with_tools(agent.tools.clone());
    let mut request = original.clone();
    let mut expected_computations = 0;
    for change in 0..13 {
        match change {
            0 => {}
            1 => request.instructions = Some("changed rules".into()),
            2 => request
                .input
                .push(ResponseInputItem::from_message_content(&Message::user(
                    "next",
                ))),
            3 => {
                request.input = vec![ResponseInputItem::from_message_content(&Message::user(
                    "compacted",
                ))]
            }
            4 => request.tools[0].function.description = "Changed schema".into(),
            5 => request.model = "another-third-party-model".into(),
            6 => request.reasoning_effort = Some(ReasoningEffort::Low),
            7 => request.max_output_tokens = Some(1000),
            8 => request.temperature = Some(0.4),
            9 => request.top_p = Some(0.8),
            10 => request.tool_choice = Some(serde_json::json!("none")),
            11 => request.text = Some(serde_json::json!({"format":{"type":"json_object"}})),
            12 => request = original.clone(),
            _ => unreachable!(),
        }
        let (fresh, _) = measurement_agent();
        let expected = fresh.measure_request(&request);
        assert_eq!(agent.measure_request(&request), expected, "change {change}");
        assert_eq!(agent.measure_request(&request), expected, "repeat {change}");
        expected_computations += 1;
        assert_eq!(
            computations.load(std::sync::atomic::Ordering::SeqCst),
            expected_computations,
            "change {change}"
        );
    }
}

#[test]
fn request_measurement_rechecks_image_availability_before_reuse() {
    struct AvailableImage {
        available: AtomicBool,
        checks: std::sync::atomic::AtomicUsize,
    }
    impl AttachmentDataResolver for AvailableImage {
        fn data_url(&self, _: &deepagent_core::message::MessageAttachment) -> Option<String> {
            panic!("pricing must not materialize image bytes");
        }
        fn is_resolvable(&self, _: &deepagent_core::message::MessageAttachment) -> bool {
            self.checks
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.available.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    let (mut agent, computations) = measurement_agent();
    let model: deepagent_models::ModelInfo = serde_json::from_value(serde_json::json!({
        "id":"third-party-model", "input_modalities":["text","image"],
    }))
    .unwrap();
    let mut config = ModelConfig::deepseek("synthetic");
    config
        .capabilities
        .push(deepagent_models::ModelCapabilityResolver::new().resolve_model(&model));
    agent.client = Arc::new(ModelClient::new(
        Arc::new(deepagent_models::transport::MockTransport::default()),
        config,
    ));
    let resolver = Arc::new(AvailableImage {
        available: AtomicBool::new(true),
        checks: std::sync::atomic::AtomicUsize::new(0),
    });
    agent.attachment_resolver = Some(resolver.clone());
    let mut message = Message::user("image");
    message
        .attachments
        .push(deepagent_core::message::MessageAttachment {
            id: "synthetic-image".into(),
            kind: "image".into(),
            media_type: Some("image/png".into()),
            path: None,
            sha256: None,
        });
    agent.context_snapshot = None;
    agent.push_message(message);
    let first = agent.context_measurement();
    assert!(first.1.attachments > 0);
    let mut request = agent
        .response_history
        .request(agent.model.clone())
        .with_tools(agent.tools.clone());
    request.reasoning_effort = agent.requested_reasoning_effort;
    let expected = deepagent_context::measure_response_request(&agent.client, &request, |_| {
        agent.client.image_token_reserve(&request.model)
    });
    assert_eq!(first.1, expected);
    assert_eq!(agent.context_measurement(), first);
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(resolver.checks.load(std::sync::atomic::Ordering::SeqCst), 2);
    resolver
        .available
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(agent.context_measurement().1.attachments, 0);
    assert_eq!(agent.context_measurement().1.attachments, 0);
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 2);
    resolver
        .available
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(agent.context_measurement(), first);
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[test]
fn request_measurement_reuses_each_protocols_actual_local_projection() {
    for wire_mode in [
        WireMode::Responses,
        WireMode::ChatCompletions,
        WireMode::Messages,
    ] {
        let (mut agent, computations) = measurement_agent();
        let mut config = ModelConfig::deepseek("synthetic").with_wire_mode(wire_mode);
        config.defaults.max_output_tokens = Some(4096);
        agent.client = Arc::new(ModelClient::new(
            Arc::new(deepagent_models::transport::MockTransport::default()),
            config,
        ));
        let request = agent
            .response_history
            .request(agent.model.clone())
            .with_tools(agent.tools.clone());
        let expected = deepagent_context::measure_response_request(&agent.client, &request, |_| 0);
        assert_eq!(agent.measure_request(&request).1, expected, "{wire_mode:?}");
        assert_eq!(agent.measure_request(&request).1, expected, "{wire_mode:?}");
        assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

#[test]
fn request_measurement_excludes_transient_image_bytes_and_stream_flag() {
    let (agent, computations) = measurement_agent();
    let mut request = agent
        .response_history
        .request(agent.model.clone())
        .with_tools(agent.tools.clone());
    let first = agent.measure_request(&request);
    request.stream = !request.stream;
    request.attachment_data_urls.insert(
        "unreferenced".into(),
        "synthetic-sensitive-image-bytes".into(),
    );
    request
        .image_file_ids
        .insert("unreferenced".into(), "transient-file-id".into());
    assert_eq!(agent.measure_request(&request), first);
    let cached = agent.request_measurement.lock().unwrap();
    assert!(cached
        .as_ref()
        .unwrap()
        .request
        .attachment_data_urls
        .is_empty());
    assert!(cached.as_ref().unwrap().request.image_file_ids.is_empty());
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn concurrent_identical_request_measurement_computes_once() {
    let (agent, computations) = measurement_agent();
    let request = agent
        .response_history
        .request(agent.model.clone())
        .with_tools(agent.tools.clone());
    let agent = Arc::new(agent);
    let barrier = Arc::new(std::sync::Barrier::new(6));
    let handles = (0..6)
        .map(|_| {
            let agent = agent.clone();
            let barrier = barrier.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                barrier.wait();
                agent.measure_request(&request)
            })
        })
        .collect::<Vec<_>>();
    let measurements = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert!(measurements.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(computations.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
#[ignore = "explicit local latency benchmark; no network or timing assertion"]
fn request_measurement_latency_probe() {
    let (mut agent, computations) = measurement_agent();
    agent.tools = (0..48)
        .map(|index| {
            ToolSchema::function(
                format!("inspect_{index}"),
                "Synthetic tool description. ".repeat(20),
                serde_json::json!({"type":"object","properties":{
            "path":{"type":"string","description":"Workspace relative path"},
            "mode":{"type":"string","enum":["short","full"]}
        },"required":["path"],"additionalProperties":false}),
            )
        })
        .collect();
    agent.push_message(Message::user("synthetic history 中文 🧪 ".repeat(2000)));
    let started = std::time::Instant::now();
    for _ in 0..100 {
        std::hint::black_box(agent.context_measurement());
    }
    println!(
        "measurement_probe: calls=100 computations={} elapsed_us={}",
        computations.load(std::sync::atomic::Ordering::SeqCst),
        started.elapsed().as_micros()
    );
}
