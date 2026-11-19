#[test]
fn admitted_model_schema_does_not_rehash_identical_tool_definitions() {
    let (db, registry, tools) = fixture();
    let recorder = recorder(&db, &registry, &tools);
    HASH_CALLS.with(|calls| calls.set(0));
    for _ in 0..3 {
        recorder.record_model_tools(&tools).unwrap();
    }
    assert_eq!(
        HASH_CALLS.with(|calls| calls.get()),
        0,
        "admission already computed the exact initial schema projection"
    );
    assert_eq!(
        RunStore::new(&db)
            .events_after("h219-unit", None)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn identical_wire_audit_reuses_projection_but_keeps_live_durability_barrier() {
    for mode in [
        deepagent_models::WireMode::Responses,
        deepagent_models::WireMode::ChatCompletions,
        deepagent_models::WireMode::Messages,
    ] {
        let (db, registry, tools) = fixture();
        let recorder = recorder(&db, &registry, &tools);
        let client = deepagent_models::ModelClient::new(
            Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
            deepagent_models::ModelConfig::deepseek("synthetic").with_wire_mode(mode),
        );
        let wire = client.project_tool_schemas("primary", &tools);
        recorder.record_wire_tools(&tools, &wire).unwrap();
        HASH_CALLS.with(|calls| calls.set(0));
        recorder.record_wire_tools(&tools, &wire).unwrap();
        assert_eq!(HASH_CALLS.with(|calls| calls.get()), 0, "{mode:?}");
        let before = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        RunStore::new(&db)
            .finish_with_event(
                "h219-unit",
                "cancelled",
                None,
                &json!({"kind":"cancelled"}),
                None,
                3,
            )
            .unwrap();
        assert!(
            recorder.record_wire_tools(&tools, &wire).is_err(),
            "cached {mode:?} projection must not bypass terminal checks"
        );
        assert_eq!(
            RunStore::new(&db)
                .events_after("h219-unit", None)
                .unwrap()
                .len(),
            before.len() + 1
        );
    }
}

#[test]
fn concurrent_wire_audit_computes_one_projection_and_publishes_one_generation() {
    let (db, registry, tools) = fixture();
    let recorder = Arc::new(recorder(&db, &registry, &tools));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads = (0..8)
        .map(|_| {
            let recorder = recorder.clone();
            let tools = tools.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let client = deepagent_models::ModelClient::new(
                    Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
                    deepagent_models::ModelConfig::deepseek("synthetic"),
                );
                let wire = client.project_tool_schemas("primary", &tools);
                HASH_CALLS.with(|calls| calls.set(0));
                barrier.wait();
                recorder.record_wire_tools(&tools, &wire).unwrap();
                HASH_CALLS.with(|calls| calls.get() > 0)
            })
        })
        .collect::<Vec<_>>();
    let computed = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .filter(|computed| *computed)
        .count();
    assert_eq!(
        computed, 1,
        "concurrent retries must share one local projection"
    );
    let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "capability_wire_snapshot")
            .count(),
        1
    );
}

#[test]
fn projection_reuse_refreshes_tool_order_route_and_exact_wire_schema() {
    let (db, registry, tools) = fixture();
    let recorder = recorder(&db, &registry, &tools[..1]);
    recorder.record_model_tools(&tools).unwrap();
    let mut reversed = tools.clone();
    reversed.reverse();
    recorder.record_model_tools(&reversed).unwrap();
    recorder.record_model_tools(&tools).unwrap();
    let client = deepagent_models::ModelClient::new(
        Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
        deepagent_models::ModelConfig::deepseek("synthetic"),
    );
    let mut wire = client.project_tool_schemas("primary", &tools);
    recorder.record_wire_tools(&tools, &wire).unwrap();
    wire.tools[0]["description"] = json!("provider projection changed");
    recorder.record_wire_tools(&tools, &wire).unwrap();
    wire.tools.reverse();
    recorder.record_wire_tools(&tools, &wire).unwrap();
    wire.model = "other".into();
    recorder.record_wire_tools(&tools, &wire).unwrap();
    let chat = deepagent_models::ModelClient::new(
        Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
        deepagent_models::ModelConfig::deepseek_chat("synthetic"),
    );
    recorder
        .record_wire_tools(&reversed, &chat.project_tool_schemas("other", &reversed))
        .unwrap();
    let events = RunStore::new(&db).events_after("h219-unit", None).unwrap();
    let model = events
        .iter()
        .filter(|event| event.event_type == "capability_schema_snapshot")
        .map(|event| &event.data["modelSchema"])
        .collect::<Vec<_>>();
    assert_eq!(model.len(), 3);
    assert_eq!(model[0]["schemaHash"], model[2]["schemaHash"]);
    assert_ne!(model[0]["schemaHash"], model[1]["schemaHash"]);
    let provider = events
        .iter()
        .filter(|event| event.event_type == "capability_wire_snapshot")
        .map(|event| &event.data["wireSchema"])
        .collect::<Vec<_>>();
    assert_eq!(provider.len(), 5);
    for (number, projection) in provider.iter().enumerate() {
        assert_eq!(projection["generation"], number);
    }
    assert_ne!(provider[0]["schemaHash"], provider[1]["schemaHash"]);
    assert_ne!(provider[1]["schemaHash"], provider[2]["schemaHash"]);
    assert_eq!(provider[2]["schemaHash"], provider[3]["schemaHash"]);
    assert_eq!(provider[3]["model"], "other");
    assert_eq!(provider[4]["wireMode"], "chat_completions");
    assert_eq!(provider[4]["sourceSchemaHash"], model[1]["schemaHash"]);
}

#[test]
fn cached_projection_rejects_independent_writer_and_retries_after_read_failure() {
    for wire_audit in [false, true] {
        let (db, registry, tools) = fixture();
        let recorder = recorder(&db, &registry, &tools);
        let client = deepagent_models::ModelClient::new(
            Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
            deepagent_models::ModelConfig::deepseek("synthetic"),
        );
        let wire = client.project_tool_schemas("primary", &tools);
        let record = || {
            if wire_audit {
                recorder.record_wire_tools(&tools, &wire)
            } else {
                recorder.record_model_tools(&tools)
            }
        };
        record().unwrap();
        // A cached projection must still observe a missing admission and may
        // retry successfully when the durable evidence is restored.
        db.with_conn(|conn| {
            conn.execute_batch("ALTER TABLE run_events RENAME TO hidden_run_events;")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
        assert!(record().is_err());
        db.with_conn(|conn| {
            conn.execute_batch("ALTER TABLE hidden_run_events RENAME TO run_events;")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
        record().unwrap();
        let mut competing = if wire_audit {
            recorder
                .wire_state
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .projection
                .clone()
        } else {
            recorder.state.lock().unwrap().serialized.clone()
        };
        competing["generation"] = json!(1);
        competing["schemaHash"] = json!("independent-writer");
        RunStore::new(&db)
            .record_capability_schema(
                "h219-unit",
                &recorder.snapshot_id,
                if wire_audit {
                    CapabilitySchema::Wire(&competing)
                } else {
                    CapabilitySchema::Model(&competing)
                },
                3,
            )
            .unwrap();
        let before = RunStore::new(&db).events_after("h219-unit", None).unwrap();
        assert!(record().is_err(), "wire_audit={wire_audit}");
        assert_eq!(
            RunStore::new(&db).events_after("h219-unit", None).unwrap(),
            before
        );
    }
}

#[test]
fn cached_schema_still_rejects_mutated_and_duplicate_source_definitions() {
    let (db, registry, tools) = fixture();
    let recorder = recorder(&db, &registry, &tools);
    let client = deepagent_models::ModelClient::new(
        Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
        deepagent_models::ModelConfig::deepseek("synthetic"),
    );
    let wire = client.project_tool_schemas("primary", &tools);
    recorder.record_wire_tools(&tools, &wire).unwrap();
    let before = RunStore::new(&db).events_after("h219-unit", None).unwrap();
    for field in ["kind", "name", "description", "parameters", "duplicate"] {
        let mut invalid = tools.clone();
        match field {
            "kind" => invalid[0].kind = "custom".into(),
            "name" => invalid[0].function.name = "unknown".into(),
            "description" => invalid[0].function.description.push('!'),
            "parameters" => invalid[0].function.parameters["additionalProperties"] = json!(false),
            "duplicate" => invalid[1] = invalid[0].clone(),
            _ => unreachable!(),
        }
        assert!(recorder.record_model_tools(&invalid).is_err(), "{field}");
        assert!(
            recorder.record_wire_tools(&invalid, &wire).is_err(),
            "{field}"
        );
    }
    assert_eq!(
        RunStore::new(&db).events_after("h219-unit", None).unwrap(),
        before
    );
}

#[test]
#[ignore = "explicit local audit benchmark; no provider calls or timing assertion"]
fn capability_projection_latency_probe() {
    struct LargeTool(String);
    #[async_trait::async_trait]
    impl Tool for LargeTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: self.0.clone(),
                description: "Synthetic description. ".repeat(40),
                parameters: json!({"type":"object","properties":{
                    "path":{"type":"string","description":"Workspace relative path"},
                    "mode":{"type":"string","enum":["summary","full"]}
                },"required":["path"],"additionalProperties":false}),
                risk: RiskLevel::Safe,
                required_permissions: PermissionSet::read_only(),
            }
        }
        async fn invoke(&self, _: Value) -> Result<ToolOutput> {
            panic!("benchmark must not invoke tools")
        }
    }
    let (db, mut registry, _) = fixture();
    for index in 0..48 {
        registry
            .register(Arc::new(LargeTool(format!("tool_{index}"))))
            .unwrap();
    }
    let tools = registry
        .visible_to(&PermissionSet::developer())
        .into_iter()
        .map(|descriptor| {
            ToolSchema::function(
                descriptor.name,
                descriptor.description,
                descriptor.parameters,
            )
        })
        .collect::<Vec<_>>();
    let recorder = recorder(&db, &registry, &tools);
    let client = deepagent_models::ModelClient::new(
        Arc::new(deepagent_models::MockTransport::new(Vec::<String>::new())),
        deepagent_models::ModelConfig::deepseek("synthetic"),
    );
    let wire = client.project_tool_schemas("primary", &tools);
    for (name, wire_audit) in [("model", false), ("wire", true)] {
        HASH_CALLS.with(|calls| calls.set(0));
        let started = std::time::Instant::now();
        for _ in 0..100 {
            if wire_audit {
                recorder.record_wire_tools(&tools, &wire).unwrap();
            } else {
                recorder.record_model_tools(&tools).unwrap();
            }
        }
        println!(
            "capability_probe: domain={name} calls=100 hashes={} elapsed_us={}",
            HASH_CALLS.with(|calls| calls.get()),
            started.elapsed().as_micros()
        );
    }
}
