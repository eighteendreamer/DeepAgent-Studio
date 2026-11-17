//! Audit the structured-summary adoption boundary, not model summarization quality.

use super::*;
use crate::session_memory::testing::{scripted_client_with_transport, ScriptedTransport};
use deepagent_runtime::{Agent, ModelAgent};

const FACT: &str = "AUDIT_SUMMARY_FACT_268";

async fn check_summary_adoption(label: &str, model_summary: &str) {
    check_summary_adoption_for_fact(label, model_summary, FACT).await;
}

async fn check_summary_adoption_for_fact(label: &str, model_summary: &str, fact: &str) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("summary.sqlite3");
    let db = Arc::new(Database::open(&path).unwrap());
    let mut session =
        deepagent_session::Session::create(&db, &deepagent_core::clock::SystemClock, None).unwrap();
    let sid = session.id();
    let items: Vec<_> = (0..24)
        .map(|index| ResponseInputItem::Message {
            role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
            content: if index == 1 {
                // A fact that the existing heuristic fallback also retains.
                format!("Created {fact}\n{}", "historical details ".repeat(600))
            } else {
                format!("unique turn {index}: {}", "historical details ".repeat(600))
            },
        })
        .collect();
    for item in &items {
        session.append_response_item(item.clone()).unwrap();
    }
    let current = ResponseInputItem::Message {
        role: "user".into(),
        content: "current".into(),
    };
    session.append_response_item(current.clone()).unwrap();
    let mut expected = items.clone();
    expected.push(current);
    let split = pairing_safe_response_item_compaction_split(&expected).unwrap();
    assert!(split > 1, "the fact must actually be in the summary zone");
    assert!(!serde_json::to_string(&expected[split..])
        .unwrap()
        .contains(fact));

    let (summary_client, summary_transport) =
        scripted_client_with_transport(vec![ScriptedTransport::text(model_summary)]);
    let (main_client, main_transport) =
        scripted_client_with_transport(vec![ScriptedTransport::text("done")]);
    let compactor = Arc::new(HookedReactiveContextCompactor::new(
        summary_client,
        "deepseek-flash".into(),
        Arc::new(HookRegistry::new()),
        sid,
        db.clone(),
    ));
    let mut agent = ModelAgent::new(
        main_client,
        "deepseek-flash",
        "protected rules",
        "current",
        vec![],
    )
    .with_response_history(items)
    .with_snip_tool(deepagent_builtins::SNIP_HISTORY_TOOL_NAME)
    .with_reactive_compactor(compactor.clone())
    .with_proactive_compaction(2_000);
    tokio::time::timeout(std::time::Duration::from_secs(5), agent.think(0, &[]))
        .await
        .expect("audit model/compaction did not finish")
        .unwrap();
    assert!(summary_transport.last_prompt().contains(fact));
    let request: serde_json::Value = serde_json::from_str(&main_transport.last_prompt()).unwrap();
    let sent: Vec<ResponseInputItem> = serde_json::from_value(request["input"].clone()).unwrap();
    assert_eq!(request["instructions"], "protected rules");
    let side_usage = agent.side_usage().unwrap();
    assert_eq!(side_usage.billed_prompt_tokens, 120);
    assert_eq!(side_usage.completion_tokens, 18);
    drop(agent);
    drop(compactor);
    drop(session);
    drop(db);

    // Close every database owner before testing the cold recovery projection.
    let reopened = Database::open(&path).unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&reopened)
        .load_session(sid)
        .unwrap();
    let boundaries: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ContextCompacted {
                tokens_before,
                tokens_after,
                summary,
                ..
            } => Some((*tokens_before, *tokens_after, summary.clone().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(boundaries.len(), 1);
    let (before, after, summary) = &boundaries[0];
    assert!(after < before);
    let replay = crate::input_runtime::response_items_from_events(&events);
    assert!(
        replay == sent,
        "live and cold-recovered surfaces must agree"
    );
    let raw_fact = events.iter().any(|event| {
        matches!(
            &event.payload,
            EventPayload::ResponseItemAppended {
                item: ResponseInputItem::Message { content, .. }
            , .. } if content.starts_with(&format!("Created {fact}"))
        )
    });
    let wire_fact = request.to_string().contains(fact);
    let replay_fact = serde_json::to_string(&replay).unwrap().contains(fact);
    let summary_fact = summary.contains(fact);
    eprintln!(
        "{label}: split={split}, tokens={before}->{after}, boundaries={}, raw_fact={raw_fact}, summary_fact={summary_fact}, wire_fact={wire_fact}, replay_fact={replay_fact}, side_usage=120+18",
        boundaries.len(),
    );
    assert!(
        raw_fact,
        "the append-only source evidence must remain intact"
    );
    assert!(
        summary.contains("完整记录（未压缩的原始事件）"),
        "the recovery pointer must remain"
    );
    assert!(
        wire_fact && replay_fact && summary_fact,
        "summary adoption must not silently remove the supplied checkpoint fact"
    );
}

#[tokio::test]
async fn valid_structured_summary_preserves_fact() {
    let output = serde_json::json!({
        "goal": "continue audit",
        "completed": [format!("Created {FACT}")],
        "pending": [], "decisions": [], "failures": [],
    });
    check_summary_adoption("valid", &output.to_string()).await;
}

#[tokio::test]
async fn non_json_summary_uses_existing_heuristic_fallback() {
    check_summary_adoption("non-json-fallback", "not a JSON summary").await;
}

#[tokio::test]
async fn audit_empty_object_does_not_shadow_history() {
    check_summary_adoption("empty-object", "{}").await;
}

#[tokio::test]
async fn audit_unrelated_object_does_not_shadow_history() {
    check_summary_adoption("unrelated-object", r#"{"unrelated":"not a checkpoint"}"#).await;
}

#[tokio::test]
async fn audit_empty_sections_do_not_shadow_history() {
    check_summary_adoption(
        "empty-sections",
        r#"{"goal":"","completed":[],"pending":[],"decisions":[],"failures":[]}"#,
    )
    .await;
}

#[tokio::test]
async fn h268_goal_only_whitespace_missing_state_and_wrong_types_use_fallback() {
    for (label, body) in [
        ("goal-only", r#"{"goal":"continue audit"}"#),
        (
            "whitespace",
            r#"{"goal":"continue audit","completed":["　"],"pending":[" \n"],"decisions":[""],"failures":[]}"#,
        ),
        (
            "wrong-type",
            r#"{"goal":"continue audit","completed":"invalid shape"}"#,
        ),
        ("null", r#"{"goal":"continue audit","pending":null}"#),
        ("fenced-empty", "```json\n{}\n```"),
    ] {
        check_summary_adoption(label, body).await;
    }
}

fn h268_seed_history(
    with_tool_tail: bool,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Arc<Database>,
    SessionId,
    Vec<ResponseInputItem>,
) {
    seed_history_for_fact(with_tool_tail, FACT)
}

fn seed_history_for_fact(
    with_tool_tail: bool,
    fact: &str,
) -> (
    tempfile::TempDir,
    std::path::PathBuf,
    Arc<Database>,
    SessionId,
    Vec<ResponseInputItem>,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("h268.sqlite3");
    let db = Arc::new(Database::open(&path).unwrap());
    let mut session =
        deepagent_session::Session::create(&db, &deepagent_core::clock::SystemClock, None).unwrap();
    let sid = session.id();
    for index in 0..24 {
        session
            .append_response_item(ResponseInputItem::Message {
                role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                content: if index == 1 {
                    format!("Created {fact}\n{}", "history detail ".repeat(600))
                } else {
                    format!("turn {index}: {}", "history detail ".repeat(600))
                },
            })
            .unwrap();
    }
    if with_tool_tail {
        session
            .append_response_item(ResponseInputItem::FunctionCall {
                call_id: "h268-tail".into(),
                name: "read_file".into(),
                arguments: " { \"path\" : \"synthetic.txt\" } ".into(),
            })
            .unwrap();
        session
            .append_response_item(ResponseInputItem::FunctionCallOutput {
                call_id: "h268-tail".into(),
                output: "synthetic retained output".into(),
            })
            .unwrap();
    }
    session
        .append_response_item(ResponseInputItem::Message {
            role: "user".into(),
            content: "current".into(),
        })
        .unwrap();
    drop(session);
    let events = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(sid)
        .unwrap();
    let items = crate::input_runtime::response_items_from_events(&events);
    (dir, path, db, sid, items)
}

#[tokio::test]
async fn h268_fallback_is_shared_by_native_compatibility_and_prefire_with_cold_reopen() {
    for mode in [
        "native",
        "compatibility",
        "prefire-native",
        "prefire-compatibility",
        "compatibility-valid",
    ] {
        let body = if mode == "compatibility-valid" {
            serde_json::json!({"completed": [format!("Created {FACT}; H268_MODEL_ONLY")]})
                .to_string()
        } else {
            "{}".into()
        };
        check_compaction_mode(
            mode,
            FACT,
            &body,
            (mode == "compatibility-valid").then_some("H268_MODEL_ONLY"),
            None,
        )
        .await;
    }
}

async fn check_compaction_mode(
    mode: &str,
    fact: &str,
    body: &str,
    model_marker: Option<&str>,
    forbidden: Option<&str>,
) {
    let native = mode.ends_with("native");
    let (_dir, path, db, sid, items) = seed_history_for_fact(native, fact);
    let messages = deepagent_models::messages_from_response_items(Some("protected rules"), &items);
    let (client, summary_wire) =
        scripted_client_with_transport(vec![ScriptedTransport::text(body)]);
    let compactor = HookedReactiveContextCompactor::new(
        client,
        "deepseek-flash".into(),
        Arc::new(HookRegistry::new()),
        sid,
        db.clone(),
    );
    let result = match mode {
        "native" => compactor
            .compact_response_items(
                Some("protected rules"),
                &items,
                CompactionTrigger::ContextOverflow,
            )
            .await
            .unwrap()
            .unwrap(),
        "compatibility" | "compatibility-valid" => compactor
            .compact(&messages, CompactionTrigger::ContextOverflow)
            .await
            .unwrap()
            .unwrap(),
        _ => {
            let note = if native {
                compactor
                    .prefire_pass1_response_items(Some("protected rules"), &items)
                    .await
                    .unwrap()
                    .unwrap()
            } else {
                compactor.prefire_pass1(&messages).await.unwrap().unwrap()
            };
            assert!(
                note.note.contains(fact),
                "{mode}: prefire cache must retain the supplied source fact"
            );
            let before = deepagent_persistence::event_store::EventStore::new(&db)
                .load_session(sid)
                .unwrap();
            assert!(!before
                .iter()
                .any(|event| matches!(event.payload, EventPayload::ContextCompacted { .. })));
            let mut stale = note.clone();
            stale.fingerprint ^= 1;
            let refused = if native {
                compactor
                    .apply_prefire_response_items(&stale, Some("protected rules"), &items)
                    .await
                    .unwrap()
            } else {
                compactor.apply_prefire(&stale, &messages).await.unwrap()
            };
            assert!(refused.is_none());
            if native {
                compactor
                    .apply_prefire_response_items(&note, Some("protected rules"), &items)
                    .await
                    .unwrap()
                    .unwrap()
            } else {
                compactor
                    .apply_prefire(&note, &messages)
                    .await
                    .unwrap()
                    .unwrap()
            }
        }
    };
    assert!(summary_wire.last_prompt().contains(fact));
    assert_eq!(result.usage.prompt_tokens, 120);
    assert_eq!(result.usage.completion_tokens, 18);
    assert!(result.tokens_after < result.tokens_before);
    assert!(
        result.summary.contains(fact) && result.summary.contains("完整记录（未压缩的原始事件）")
    );
    if let Some(marker) = model_marker {
        assert!(
            result.summary.contains(marker),
            "valid sparse model state is actually adopted"
        );
    }
    if let Some(forbidden) = forbidden {
        assert!(!result.summary.contains(forbidden));
    }
    let live = result
        .response_items
        .unwrap_or_else(|| deepagent_models::response_items_from_messages(&result.messages).1);
    if native {
        for item in &items {
            if matches!(
                item,
                ResponseInputItem::FunctionCall { .. }
                    | ResponseInputItem::FunctionCallOutput { .. }
            ) {
                assert!(
                    live.contains(item),
                    "{mode}: retained tool pair must be exact"
                );
            }
        }
    }
    // Exercise the real provider serializer on the adopted surface, not a
    // hand-built JSON expectation. The transport stays entirely offline.
    let (next_client, next_wire) =
        scripted_client_with_transport(vec![ScriptedTransport::text("done")]);
    next_client
        .stream_response(deepagent_models::ResponseRequest::from_response_items(
            "deepseek-flash",
            Some("protected rules".into()),
            live.clone(),
        ))
        .await
        .unwrap();
    let request: serde_json::Value = serde_json::from_str(&next_wire.last_prompt()).unwrap();
    let sent: Vec<ResponseInputItem> = serde_json::from_value(request["input"].clone()).unwrap();
    assert_eq!(sent, live);
    assert_eq!(request["instructions"], "protected rules");
    if let Some(marker) = model_marker {
        assert!(request.to_string().contains(marker));
    }
    if let Some(forbidden) = forbidden {
        assert!(!request.to_string().contains(forbidden));
    }
    drop(compactor);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&reopened)
        .load_session(sid)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.payload, EventPayload::ContextCompacted { .. }))
            .count(),
        1
    );
    let replay = crate::input_runtime::response_items_from_events(&events);
    if native {
        assert!(
            replay == sent,
            "{mode}: native cold replay must equal live wire"
        );
    } else {
        // Existing compatibility record_durable_boundary unconditionally
        // adds one extra transcript pointer. Check that exact discrepancy,
        // including the valid model control, rather than hiding differences
        // with trim/field omission or mixing this independent formatter bug.
        let mut expected = sent.clone();
        let Some(ResponseInputItem::Message { content, .. }) = expected.first_mut() else {
            panic!("summary message required")
        };
        *content = crate::input_runtime::with_transcript_pointer(
            content.clone(),
            &reopened.session_transcript_dir(sid),
        );
        assert!(
            replay == expected,
            "{mode}: only the known extra compatibility pointer may differ"
        );
    }
    let replay_text = serde_json::to_string(&replay).unwrap();
    assert!(replay_text.contains(fact));
    if let Some(marker) = model_marker {
        assert!(replay_text.contains(marker));
    }
    if let Some(forbidden) = forbidden {
        assert!(!replay_text.contains(forbidden));
    }
    eprintln!("checkpoint {mode}: fact, tool tail where applicable, 120+18 usage, one boundary; native exact replay / compatibility known extra pointer verified");
}

#[tokio::test]
#[cfg(windows)]
async fn h268_native_boundary_storage_failure_does_not_adopt_or_erase_usage() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let (_dir, path, db, sid, items) = h268_seed_history(true);
    let (client, _) = scripted_client_with_transport(vec![ScriptedTransport::text("{}")]);
    let compactor = HookedReactiveContextCompactor::new(
        client,
        "deepseek-flash".into(),
        Arc::new(HookRegistry::new()),
        sid,
        db.clone(),
    );
    // The event file, not its SQLite projection, is authoritative. Hold a
    // read-only share handle to deny the writer access used by the boundary.
    // Canonical reads also request writer access; this fails during boundary
    // materialization, before append, and is not an append-only fault injection.
    let files: Vec<_> = std::fs::read_dir(db.session_transcript_dir(sid))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.to_string_lossy().ends_with(".v1.jsonl.zst"))
        .collect();
    assert_eq!(files.len(), 1);
    let bytes_before = std::fs::read(&files[0]).unwrap();
    let clock = deepagent_core::clock::SystemClock;
    let mut recovered = deepagent_session::Session::recover(&db, &clock, sid).unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&files[0])
        .unwrap();
    let failure = compactor
        .compact_response_items_accounted(
            Some("protected rules"),
            &items,
            CompactionTrigger::ContextOverflow,
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&failure.error, CoreError::Persistence(_)),
        "actual storage failure: {}",
        failure.error
    );
    assert_eq!(failure.usage.prompt_tokens, 120);
    assert_eq!(failure.usage.completion_tokens, 18);
    // Separately reach the real Session -> EventStore -> append_projection
    // writer with recovery already complete. This proves append rejection, not
    // an injected mock or a SQLite projection failure. The compactor case above
    // stops earlier at materialization; do not conflate these two boundaries.
    let append_error = recovered
        .append(EventPayload::ContextCompacted {
            tokens_before: 100,
            tokens_after: 10,
            strategy: deepagent_core::compaction_strategy::REACTIVE_MODEL.into(),
            summary: Some("synthetic rejected boundary".into()),
            first_kept_sequence: Some(1),
        })
        .unwrap_err();
    assert!(
        matches!(&append_error, CoreError::Persistence(message) if message.starts_with("append session file:")),
        "actual append failure: {append_error}"
    );
    drop(held);
    assert_eq!(std::fs::read(&files[0]).unwrap(), bytes_before);
    drop(recovered);
    drop(compactor);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&reopened)
        .load_session(sid)
        .unwrap();
    assert!(!events
        .iter()
        .any(|event| matches!(event.payload, EventPayload::ContextCompacted { .. })));
    assert_eq!(
        crate::input_runtime::response_items_from_events(&events),
        items
    );
}

const LITERAL_FACT: &str = "<analysis>AUDIT_SUMMARY_LITERAL_269</analysis>";

#[tokio::test]
async fn h269_cancelled_partial_literal_checkpoint_does_not_replace_durable_history() {
    use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
    use std::sync::atomic::{AtomicBool, Ordering};
    struct PartialSummary(tokio::sync::Semaphore);
    #[async_trait::async_trait]
    impl HttpTransport for PartialSummary {
        async fn stream(
            &self,
            _: TransportRequest,
            sink: &mut dyn EventSink,
        ) -> deepagent_core::error::Result<()> {
            let sink: &mut dyn EventSink = &mut crate::test_sequence::sink(sink);
            sink.on_event(&serde_json::json!({"type":"response.output_text.delta","delta":format!("{{\"completed\":[\"Created {LITERAL_FACT}")}).to_string())?;
            self.0.add_permits(1);
            std::future::pending().await
        }
    }
    let (_dir, path, db, sid, items) = seed_history_for_fact(true, LITERAL_FACT);
    let original = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(sid)
        .unwrap();
    let transport = Arc::new(PartialSummary(tokio::sync::Semaphore::new(0)));
    let client = Arc::new(deepagent_models::ModelClient::new(
        transport.clone(),
        deepagent_models::ModelConfig::deepseek("synthetic"),
    ));
    let compactor = HookedReactiveContextCompactor::new(
        client,
        "deepseek-flash".into(),
        Arc::new(HookRegistry::new()),
        sid,
        db.clone(),
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let operation = compactor.compact_response_items_accounted(
        Some("rules"),
        &items,
        CompactionTrigger::ContextOverflow,
        Some(cancel.clone()),
    );
    let signal = async {
        transport.0.acquire().await.unwrap().forget();
        cancel.store(true, Ordering::Release);
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(operation, signal)
    })
    .await
    .unwrap();
    let error = result.unwrap_err();
    assert!(error.error.to_string().contains("cancelled"));
    assert_eq!(
        error.usage,
        deepagent_models::Usage::default(),
        "partial stream did not report provider usage"
    );
    assert_eq!(
        deepagent_persistence::event_store::EventStore::new(&db)
            .load_session(sid)
            .unwrap(),
        original
    );
    drop(compactor);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&reopened)
        .load_session(sid)
        .unwrap();
    assert_eq!(events, original);
    assert_eq!(
        crate::input_runtime::response_items_from_events(&events),
        items
    );
}

#[tokio::test]
async fn h269_literal_data_survives_native_compatibility_and_prefire_on_wire_and_cold_reopen() {
    for (index, fact) in [
        LITERAL_FACT,
        "中文🦀<analysis>未闭合但合法JSON字符串",
        "<analysis>外层<analysis>内层</analysis>末</analysis><analysis>再次</analysis>",
    ]
    .iter()
    .enumerate()
    {
        let mut checkpoint: serde_json::Value =
            serde_json::from_str(&literal_checkpoint(fact)).unwrap();
        checkpoint["completed"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!("H269_MODEL_ONLY"));
        let checkpoint = checkpoint.to_string();
        let body = match index {
            0 => checkpoint,
            1 => format!("Here is the summary:\n```json\n{checkpoint}\n```"),
            _ => format!("<analysis>H269_PRIVATE_DRAFT {{\"completed\":[\"draft\"]}}</analysis>\n<analysis>nested<analysis>scratch</analysis></analysis>\n{checkpoint}"),
        };
        for mode in [
            "native",
            "compatibility",
            "prefire-native",
            "prefire-compatibility",
        ] {
            check_compaction_mode(
                mode,
                fact,
                &body,
                Some("H269_MODEL_ONLY"),
                Some("H269_PRIVATE_DRAFT"),
            )
            .await;
        }
    }
}

fn literal_checkpoint(fact: &str) -> String {
    serde_json::json!({
        "goal": "continue audit",
        "completed": [format!("Created {fact}")],
        "pending": ["next audit"],
        "decisions": ["preserve the literal template text"],
        "failures": [],
    })
    .to_string()
}

#[tokio::test]
async fn literal_summary_escaped_analysis_tag_survives() {
    let literal = literal_checkpoint(LITERAL_FACT);
    let escaped = literal.replace('<', "\\u003c");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&literal).unwrap(),
        serde_json::from_str::<serde_json::Value>(&escaped).unwrap(),
        "the two summary bodies must describe exactly the same JSON value"
    );
    check_summary_adoption_for_fact("unicode-escaped-analysis", &escaped, LITERAL_FACT).await;
}

#[tokio::test]
async fn literal_summary_other_tag_survives() {
    let fact = "<summary>AUDIT_SUMMARY_LITERAL_269</summary>";
    check_summary_adoption_for_fact("literal-summary-tag", &literal_checkpoint(fact), fact).await;
}

#[tokio::test]
async fn literal_summary_leading_scratchpad_still_allows_checkpoint() {
    let body = format!(
        "<analysis>AUDIT_DRAFT_ONLY_269</analysis>\n{}",
        literal_checkpoint(FACT)
    );
    check_summary_adoption("leading-scratchpad", &body).await;
}

#[tokio::test]
async fn audit_literal_summary_analysis_tag_does_not_erase_fact() {
    check_summary_adoption_for_fact(
        "literal-analysis-tag",
        &literal_checkpoint(LITERAL_FACT),
        LITERAL_FACT,
    )
    .await;
}
