//! H-242: persisted native context, never the bounded status preview, drives resume.
use super::tests::audit_child_runner_with_db;
use super::*;
use deepagent_builtins::{SubagentRequest, SubagentRunner};
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_persistence::subagent_store::SubagentRunStore;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Mutex};

#[derive(Default)]
struct Model {
    requests: Mutex<Vec<Value>>,
    read: bool,
    fail: AtomicBool,
    wait: AtomicBool,
    entered: tokio::sync::Notify,
    hold: AtomicBool,
    release: tokio::sync::Notify,
    break_transcript: Mutex<Option<PathBuf>>,
}

#[async_trait::async_trait]
impl HttpTransport for Model {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(serde_json::from_str(&request.body)?);
            requests.len() - 1
        };
        if index == 1 && self.hold.load(Ordering::Acquire) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if let Some(path) = self.break_transcript.lock().unwrap().take() {
            std::fs::rename(&path, path.with_extension("old")).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
        if self.read && index == 1 {
            self.entered.notify_one();
            if self.wait.load(Ordering::Acquire) {
                std::future::pending::<()>().await;
            }
            if self.fail.load(Ordering::Acquire) {
                return Err(CoreError::invalid("synthetic child model failure"));
            }
        }
        let item = if self.read && index == 0 {
            json!({"type":"function_call","id":"item-read","status":"completed",
                "call_id":"read-once","name":"read_file","arguments":r#"{"path":"fact.txt"}"#})
        } else {
            json!({"type":"message","id":format!("answer-{index}"),"role":"assistant",
                "status":"completed","content":[{"type":"output_text","text":format!("ANSWER_{index}_{}", "研".repeat(2100)),"annotations":[]}]})
        };
        sink.on_event(
            &json!({"type":"response.output_item.done","output_index":0,"item":item}).to_string(),
        )?;
        sink.on_event(
            &json!({"type":"response.completed","response":{"id":format!("response-{index}"),
            "object":"response","created_at":1,"model":"deepseek-flash","status":"completed",
            "output":[item],"usage":{"input_tokens":10,"output_tokens":10,"total_tokens":20}}})
            .to_string(),
        )?;
        Ok(())
    }
}

fn request() -> SubagentRequest {
    serde_json::from_value(
        json!({"description":"context fixture","prompt":"ORIGINAL_NATIVE_TASK",
        "isolation":"shared"}),
    )
    .unwrap()
}

fn record(runner: &ChatSubagentRunner) -> deepagent_persistence::subagent_store::SubagentRunRecord {
    SubagentRunStore::new(&runner.db)
        .list_for_parent("audit-parent")
        .unwrap()
        .remove(0)
}

fn transcript(record: &deepagent_persistence::subagent_store::SubagentRunRecord) -> Value {
    serde_json::from_slice(&std::fs::read(record.transcript_path.as_ref().unwrap()).unwrap())
        .unwrap()
}

struct WaitBeforeSession(Arc<tokio::sync::Notify>);

#[async_trait::async_trait]
impl deepagent_hooks::Hook for WaitBeforeSession {
    fn name(&self) -> &str {
        "h242-prestart-wait"
    }
    async fn run(&self, _context: &HookContext) -> Result<HookOutcome> {
        self.0.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn h242_drop_before_new_session_preserves_the_loaded_prior_surface() {
    let root = tempfile::tempdir().unwrap();
    let model = Arc::new(Model::default());
    let mut runner = audit_child_runner_with_db(
        root.path(),
        model.clone(),
        Arc::new(Database::open_in_memory().unwrap()),
    );
    runner.run_inner(request(), None).await.unwrap();
    let saved = record(&runner);
    let entered = Arc::new(tokio::sync::Notify::new());
    let mut hooks = HookRegistry::new();
    hooks.register(
        HookPoint::SubagentStart,
        Arc::new(WaitBeforeSession(entered.clone())),
    );
    runner.hooks.set(Arc::new(hooks)).ok().unwrap();
    let mut future =
        Box::pin(runner.resume(&saved.id, "CANCEL_BEFORE_SESSION", Default::default()));
    tokio::select! {
        _ = entered.notified() => {},
        result = &mut future => panic!("start hook did not wait: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("start hook not reached"),
    }
    drop(future);
    let cancelled = record(&runner);
    assert_eq!(cancelled.state, "cancelled");
    let value = transcript(&cancelled);
    assert!(
        value["context"]["items"].to_string().contains("ANSWER_0_"),
        "a pre-session drop must not overwrite the prior native history with null"
    );
    assert_eq!(
        value["attempts"][1]["request"]["prompt"],
        "CANCEL_BEFORE_SESSION"
    );
    runner.hooks = Default::default();
    runner
        .resume(&saved.id, "CONTINUE_AFTER_PRESTART", Default::default())
        .await
        .unwrap();
    assert_eq!(record(&runner).resume_count, 2);
    let requests = model.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "pre-session drop never dispatched a model/tool"
    );
    assert!(requests[1]["input"].to_string().contains("ANSWER_0_"));
}

#[tokio::test]
async fn h242_native_context_survives_database_close_and_two_resumes() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("state.sqlite");
    let model = Arc::new(Model::default());
    let id = {
        let runner = audit_child_runner_with_db(
            root.path(),
            model.clone(),
            Arc::new(Database::open(&db_path).unwrap()),
        );
        runner.run_inner(request(), None).await.unwrap();
        let saved = record(&runner);
        assert_eq!(saved.summary.as_ref().unwrap().chars().count(), 2000);
        let context = transcript(&saved)["context"].clone();
        assert_eq!(context["version"], 1);
        assert!(context["items"].to_string().contains("ANSWER_0_"));
        saved.id
    };
    // All database/session/host owners above have been dropped, not just the UI service.
    for (attempt, continuation) in [(1, "CONTINUE_ONE"), (2, "CONTINUE_TWO")] {
        let runner = audit_child_runner_with_db(
            root.path(),
            model.clone(),
            Arc::new(Database::open(&db_path).unwrap()),
        );
        runner
            .resume(&id, continuation, Default::default())
            .await
            .unwrap();
        let saved = record(&runner);
        assert_eq!(saved.resume_count, attempt);
        assert_eq!(transcript(&saved)["context"]["attempt"], attempt);
        assert_eq!(
            transcript(&saved)["attempts"].as_array().unwrap().len(),
            attempt as usize + 1
        );
    }
    let requests = model.requests.lock().unwrap();
    for (index, expected) in [
        (1, vec!["ORIGINAL_NATIVE_TASK", "ANSWER_0_", "CONTINUE_ONE"]),
        (
            2,
            vec![
                "ORIGINAL_NATIVE_TASK",
                "ANSWER_0_",
                "CONTINUE_ONE",
                "ANSWER_1_",
                "CONTINUE_TWO",
            ],
        ),
    ] {
        let input = requests[index]["input"].to_string();
        for fragment in expected {
            assert_eq!(
                input.matches(fragment).count(),
                1,
                "native history must not duplicate or omit {fragment}"
            );
        }
    }
}

#[tokio::test]
async fn h242_unrecoverable_transcripts_fail_before_claim_or_dispatch() {
    for damage in [
        "legacy",
        "version",
        "items",
        "empty",
        "ambiguous",
        "attempt",
        "identity",
        "json",
    ] {
        let root = tempfile::tempdir().unwrap();
        let model = Arc::new(Model::default());
        let runner = audit_child_runner_with_db(
            root.path(),
            model.clone(),
            Arc::new(Database::open_in_memory().unwrap()),
        );
        runner.run_inner(request(), None).await.unwrap();
        let saved = record(&runner);
        let mut value = transcript(&saved);
        match damage {
            "legacy" => {
                value.as_object_mut().unwrap().remove("context");
            }
            "version" => value["context"]["version"] = json!(999),
            "items" => value["context"]["items"] = json!([{"type":"unknown"}]),
            "empty" => value["context"]["items"] = json!([]),
            "ambiguous" => {
                value["context"]["items"] = json!([
                {"type":"function_call","call_id":"duplicate","name":"read_file","arguments":"{}"},
                {"type":"function_call","call_id":"duplicate","name":"read_file","arguments":"{}"}])
            }
            "attempt" => value["context"]["attempt"] = json!(99),
            "identity" => value["subagent_id"] = json!("different-child"),
            "json" => {}
            _ => unreachable!(),
        }
        std::fs::write(
            saved.transcript_path.as_ref().unwrap(),
            if damage == "json" {
                b"{".to_vec()
            } else {
                serde_json::to_vec(&value).unwrap()
            },
        )
        .unwrap();
        let error = runner
            .resume(&saved.id, "CONTINUE", Default::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("NOT_RESUMABLE"),
            "{damage}: {error}"
        );
        assert_eq!(
            record(&runner),
            saved,
            "rejected recovery must not consume an attempt"
        );
        assert_eq!(model.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn h242_concurrent_resume_claims_only_one_context_owner() {
    let root = tempfile::tempdir().unwrap();
    let model = Arc::new(Model::default());
    let runner = audit_child_runner_with_db(
        root.path(),
        model.clone(),
        Arc::new(Database::open_in_memory().unwrap()),
    );
    runner.run_inner(request(), None).await.unwrap();
    let saved = record(&runner);
    model.hold.store(true, Ordering::Release);
    let mut first = Box::pin(runner.resume(&saved.id, "FIRST_OWNER", Default::default()));
    tokio::select! {
        _ = model.entered.notified() => {},
        result = &mut first => panic!("must hold first resume: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("resume did not start"),
    }
    assert!(runner
        .resume(&saved.id, "SECOND_OWNER", Default::default())
        .await
        .is_err());
    model.release.notify_one();
    first.await.unwrap();
    assert_eq!(record(&runner).resume_count, 1);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn h242_write_failure_is_reported_and_old_snapshot_cannot_resume_new_attempt() {
    let root = tempfile::tempdir().unwrap();
    let model = Arc::new(Model::default());
    let runner = audit_child_runner_with_db(
        root.path(),
        model.clone(),
        Arc::new(Database::open_in_memory().unwrap()),
    );
    runner.run_inner(request(), None).await.unwrap();
    let saved = record(&runner);
    let path = PathBuf::from(saved.transcript_path.as_ref().unwrap());
    *model.break_transcript.lock().unwrap() = Some(path.clone());
    assert!(runner
        .resume(&saved.id, "WRITE_MUST_FAIL", Default::default())
        .await
        .is_err());
    let failed_write = record(&runner);
    assert_eq!(failed_write.state, "failed");
    assert!(
        failed_write.finished_at.is_some(),
        "filesystem failure must not leave the child running"
    );
    assert_eq!(failed_write.resume_count, 1);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(path.with_extension("old"), &path).unwrap();
    let error = runner
        .resume(&saved.id, "STALE_CONTEXT", Default::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("NOT_RESUMABLE"));
    assert_eq!(record(&runner), failed_write);
    assert_eq!(model.requests.lock().unwrap().len(), 2);
}

#[test]
fn h242_saved_surface_uses_existing_compaction_replacement_and_native_item_projection() {
    use deepagent_core::event::{EventPayload, ToolResultReplacement};
    use deepagent_core::message::MessageAttachment;
    let root = tempfile::tempdir().unwrap();
    let runner = audit_child_runner_with_db(
        root.path(),
        Arc::new(Model::default()),
        Arc::new(Database::open_in_memory().unwrap()),
    );
    let request = request();
    assert!(runner
        .begin_tracking("native", &request, false, None)
        .unwrap());
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut session = Session::create(&db, &SystemClock, None).unwrap();
    session
        .append_response_item(ResponseInputItem::Message {
            role: "user".into(),
            content: "SHADOWED_PREFIX".into(),
        })
        .unwrap();
    let first_kept = session
        .append_response_item(ResponseInputItem::Reasoning {
            id: Some("reasoning-242".into()),
            content: "NATIVE_REASONING".into(),
            signature: None,
        })
        .unwrap()
        .sequence;
    session
        .append_response_item(ResponseInputItem::FunctionCall {
            call_id: "function-242".into(),
            name: "read_file".into(),
            arguments: "{}".into(),
        })
        .unwrap();
    let output = session
        .append_response_item(ResponseInputItem::FunctionCallOutput {
            call_id: "function-242".into(),
            output: "OLD_RESULT_MUST_NOT_REAPPEAR".repeat(1000).into(),
        })
        .unwrap();
    session
        .append_response_item(ResponseInputItem::CustomToolCall {
            call_id: "custom-242".into(),
            name: "apply_patch".into(),
            input: "SYNTHETIC_PATCH".into(),
        })
        .unwrap();
    session
        .append_response_item(ResponseInputItem::CustomToolCallOutput {
            call_id: "custom-242".into(),
            output: "NATIVE_CUSTOM_RESULT".repeat(1000).into(),
        })
        .unwrap();
    session
        .append_response_item(ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "NATIVE_ATTACHMENT_REF".into(),
            attachments: vec![MessageAttachment {
                sha256: None,
                id: "file-242".into(),
                kind: "file".into(),
                media_type: Some("text/plain".into()),
                path: Some(
                    root.path()
                        .join("synthetic.txt")
                        .to_string_lossy()
                        .into_owned(),
                ),
            }],
        })
        .unwrap();
    session
        .append(EventPayload::ContextSurfacePatched {
            version: 1,
            strategy: "micro_compact".into(),
            replacements: vec![ToolResultReplacement {
                source_sequence: output.sequence,
                call_id: "function-242".into(),
                replacement_output: "PERSISTED_REPLACEMENT".into(),
            }],
        })
        .unwrap();
    session
        .append(EventPayload::ContextCompacted {
            tokens_before: 1000,
            tokens_after: 100,
            strategy: "reactive_model".into(),
            summary: Some("PERSISTED_COMPACTION".into()),
            first_kept_sequence: Some(first_kept),
        })
        .unwrap();
    session
        .append(EventPayload::ContextSnipped {
            tags: vec!["u999".into()],
        })
        .unwrap();
    let events = deepagent_persistence::event_store::EventStore::new(&db)
        .load_session(session.id())
        .unwrap();
    let expected = crate::input_runtime::response_items_from_events(&events);
    let mut lifecycle = TrackedSubagent {
        runner: &runner,
        id: "native".into(),
        request,
        created_at: 1,
        persist: true,
        background: false,
        armed: true,
        surface: Some(crate::context_runtime::SessionSurfaceStore {
            db: db.clone(),
            session_id: session.id(),
        }),
        seed_history: Vec::new(),
        origin_activity: None,
        stop_transcript_ready: false,
        stop_transcript_state: None,
        stop_transcript_view: None,
    };
    lifecycle.finish("succeeded", "short preview").unwrap();
    let saved = record(&runner);
    assert_eq!(
        transcript(&saved)["context"]["items"],
        serde_json::to_value(&expected).unwrap()
    );
    let input = runner.resume_request(&saved, "CONTINUE_NATIVE").unwrap();
    assert_eq!(
        input.history,
        deepagent_models::ResponseRequest::from_response_items("deepseek-flash", None, expected)
            .input
    );
    let rendered = serde_json::to_string(&input.history).unwrap();
    for text in [
        "PERSISTED_COMPACTION",
        "PERSISTED_REPLACEMENT",
        "NATIVE_CUSTOM_RESULT",
        "NATIVE_REASONING",
        "NATIVE_ATTACHMENT_REF",
    ] {
        assert!(rendered.contains(text));
    }
    assert!(
        !rendered.contains("SHADOWED_PREFIX") && !rendered.contains("OLD_RESULT_MUST_NOT_REAPPEAR")
    );
}

#[tokio::test]
async fn h242_interrupted_call_gets_existing_explicit_missing_result_without_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let model = Arc::new(Model::default());
    let runner = audit_child_runner_with_db(
        root.path(),
        model.clone(),
        Arc::new(Database::open_in_memory().unwrap()),
    );
    runner.run_inner(request(), None).await.unwrap();
    let saved = record(&runner);
    let mut value = transcript(&saved);
    value["context"]["items"].as_array_mut().unwrap().push(json!({"type":"function_call","call_id":"unfinished","name":"read_file","arguments":r#"{"path":"not-read.txt"}"#}));
    std::fs::write(
        saved.transcript_path.as_ref().unwrap(),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    runner
        .resume(&saved.id, "CONTINUE_INTERRUPTED", Default::default())
        .await
        .unwrap();
    let requests = model.requests.lock().unwrap();
    let input = requests.last().unwrap()["input"].as_array().unwrap();
    let outputs: Vec<_> = input
        .iter()
        .filter(|i| i["type"] == "function_call_output" && i["call_id"] == "unfinished")
        .collect();
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0]["output"]
        .as_str()
        .unwrap()
        .contains("tool result missing"));
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn h242_failure_cancel_and_forced_drop_preserve_observed_tools_without_reexecution() {
    for ending in ["failed", "cancelled", "dropped"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("fact.txt"), "OBSERVED_ONCE_242").unwrap();
        let model = Arc::new(Model {
            read: true,
            ..Default::default()
        });
        model.fail.store(ending == "failed", Ordering::Release);
        model.wait.store(ending != "failed", Ordering::Release);
        let runner = audit_child_runner_with_db(
            root.path(),
            model.clone(),
            Arc::new(Database::open_in_memory().unwrap()),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let mut future = Box::pin(runner.run_inner(request(), Some(cancel.clone())));
        if ending == "failed" {
            assert!(future.await.is_err());
        } else {
            tokio::select! {
                _ = model.entered.notified() => {},
                result = &mut future => { panic!("must reach model after actual read: {result:?}"); },
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => panic!("child did not read"),
            }
            if ending == "cancelled" {
                cancel.store(true, Ordering::Release);
                assert!(
                    tokio::time::timeout(std::time::Duration::from_secs(5), future)
                        .await
                        .unwrap()
                        .is_err()
                );
            } else {
                drop(future);
            }
        }
        let saved = record(&runner);
        assert_eq!(
            saved.state,
            if ending == "failed" {
                "failed"
            } else {
                "cancelled"
            }
        );
        assert!(
            transcript(&saved)["context"]["items"]
                .to_string()
                .contains("OBSERVED_ONCE_242"),
            "{ending}: preserve actual tool result"
        );
        std::fs::write(root.path().join("fact.txt"), "MUST_NOT_BE_REREAD").unwrap();
        runner
            .resume(&saved.id, "CONTINUE_AFTER_END", Default::default())
            .await
            .unwrap();
        let requests = model.requests.lock().unwrap();
        let items = requests.last().unwrap()["input"].as_array().unwrap();
        for kind in ["function_call", "function_call_output"] {
            assert_eq!(
                items
                    .iter()
                    .filter(|item| item["type"] == kind && item["call_id"] == "read-once")
                    .count(),
                1
            );
        }
        let input = items.iter().map(Value::to_string).collect::<String>();
        assert!(input.contains("OBSERVED_ONCE_242") && !input.contains("MUST_NOT_BE_REREAD"));
        assert_eq!(
            requests.len(),
            3,
            "resume must not execute the prior read again"
        );
    }
}
