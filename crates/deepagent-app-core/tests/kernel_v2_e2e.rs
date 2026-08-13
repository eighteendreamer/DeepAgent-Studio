use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_app_core::{ChatService, MemorySecretStore, PermissionPreset, SettingsService};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::Database;
use deepagent_subagents::WorktreeProvider;

#[derive(Debug)]
struct ReplayTransport {
    turns: Mutex<VecDeque<Vec<String>>>,
}

impl ReplayTransport {
    fn new(turns: impl IntoIterator<Item = Vec<String>>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
        }
    }
}

fn response_text(text: &str) -> Vec<String> {
    vec![
        serde_json::json!({"type":"response.output_text.delta","delta":text}).to_string(),
        serde_json::json!({"type":"response.completed","response":{"status":"completed"}})
            .to_string(),
    ]
}

fn response_function_call(call_id: &str, name: &str, arguments: serde_json::Value) -> Vec<String> {
    let item = serde_json::json!({
        "type":"function_call",
        "id":format!("item_{call_id}"),
        "call_id":call_id,
        "name":name,
        "arguments":serde_json::to_string(&arguments).unwrap()
    });
    vec![
        serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
        serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
        serde_json::json!({"type":"response.completed","response":{"status":"completed"}})
            .to_string(),
    ]
}

fn emit_responses_events(sink: &mut dyn EventSink, events: &[String]) -> Result<()> {
    for event in events {
        if sink.on_event(event)? {
            break;
        }
    }
    Ok(())
}

fn request_contains_user_input(body: &serde_json::Value, expected: &str) -> bool {
    body.get("input")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("role").and_then(serde_json::Value::as_str) == Some("user")
                    && item.get("content").and_then(serde_json::Value::as_str) == Some(expected)
            })
        })
}

#[async_trait]
impl HttpTransport for ReplayTransport {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let events = self
            .turns
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop_front()
            .ok_or_else(|| CoreError::other("fake model has no remaining turn"))?;
        emit_responses_events(sink, &events)
    }
}

#[derive(Debug)]
struct HangingTransport {
    started: Arc<tokio::sync::Notify>,
}

#[derive(Debug)]
struct BackgroundRoutingTransport {
    parent_calls: AtomicUsize,
    child_started: Arc<tokio::sync::Notify>,
}

#[derive(Debug, Default)]
struct ResumeRoutingTransport {
    calls: AtomicUsize,
    resume_id: Mutex<Option<String>>,
}

#[async_trait]
impl HttpTransport for ResumeRoutingTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        let events = match call {
            0 => response_function_call(
                "call-initial-child",
                "task",
                serde_json::json!({
                    "description": "initial child",
                    "prompt": "produce-initial-result"
                }),
            ),
            1 => response_text("initial-result"),
            2 => response_text("Initial child completed."),
            3 => {
                let id = self
                    .resume_id
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .clone()
                    .ok_or_else(|| CoreError::other("resume id was not installed"))?;
                response_function_call(
                    "call-resume-child",
                    "task",
                    serde_json::json!({
                        "operation": "resume",
                        "subagent_id": id,
                        "prompt": "add-more"
                    }),
                )
            }
            4 => {
                let body: serde_json::Value = serde_json::from_str(&request.body)
                    .map_err(|error| CoreError::other(error.to_string()))?;
                let prompt = body["input"]
                    .as_array()
                    .and_then(|messages| messages.last())
                    .and_then(|message| message["content"].as_str())
                    .unwrap_or_default();
                if !prompt.contains("Previous result:\ninitial-result")
                    || !prompt.contains("Continuation request:\nadd-more")
                {
                    return Err(CoreError::other(format!(
                        "resume context was not reconstructed: {prompt}"
                    )));
                }
                response_text("resumed-result")
            }
            5 => response_text("Resumed child completed."),
            _ => return Err(CoreError::other("unexpected resume test model call")),
        };
        emit_responses_events(sink, &events)
    }
}

#[async_trait]
impl HttpTransport for BackgroundRoutingTransport {
    async fn stream(&self, _request: TransportRequest, _sink: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other(
            "background routing transport requires cancellation-aware streaming",
        ))
    }

    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        let is_child = serde_json::from_str::<serde_json::Value>(&request.body)
            .ok()
            .is_some_and(|body| request_contains_user_input(&body, "wait-child-until-cancelled"));
        if is_child {
            self.child_started.notify_one();
            while !cancel.load(Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            return Err(CoreError::other("request cancelled"));
        }
        let call = self.parent_calls.fetch_add(1, Ordering::AcqRel);
        let events = if call == 0 {
            response_function_call(
                "call-background",
                "task",
                serde_json::json!({
                    "description": "background wait",
                    "prompt": "wait-child-until-cancelled",
                    "background": true
                }),
            )
        } else {
            response_text("Background child started.")
        };
        emit_responses_events(sink, &events)
    }
}

#[async_trait]
impl HttpTransport for HangingTransport {
    async fn stream(&self, _request: TransportRequest, _sink: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other("hanging transport requires cancellation"))
    }

    async fn stream_cancelled(
        &self,
        _request: TransportRequest,
        _sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        self.started.notify_one();
        while !cancel.load(Ordering::Acquire) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Err(CoreError::other("request cancelled"))
    }
}

fn e2e_root(run_id: &str) -> PathBuf {
    PathBuf::from(r"G:\Code\Kotlin_code\_deepagent-e2e").join(run_id)
}

#[tokio::test]
async fn write_turn_reaches_terminal_with_checkpoint_and_real_side_effect() {
    let run_id = format!(
        "kernel-v2-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("cleanup-manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "run_id": run_id,
            "remove": [root]
        }))
        .unwrap(),
    )
    .unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();

    let tool_turn = response_function_call(
        "call-write",
        "write_file",
        serde_json::json!({"path":"result.txt","content":"kernel-v2-ok"}),
    );
    let final_turn = response_text("Created and verified result.txt.");
    let transport: Arc<dyn HttpTransport> = Arc::new(ReplayTransport::new([tool_turn, final_turn]));
    let chat = ChatService::new(db.clone(), settings, transport, &root);

    let session_id = chat
        .run_in_session(
            "Create result.txt containing kernel-v2-ok.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(&run_id),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();

    assert!(!session_id.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("result.txt")).unwrap(),
        "kernel-v2-ok"
    );
    let events = chat.run_events(&run_id, None).unwrap();
    assert!(events
        .iter()
        .any(|event| { event.event_type == "run_terminal" && event.data["kind"] == "succeeded" }));
    let checkpoint_count: i64 = db
        .with_conn(|conn| {
            conn.query_row(
                "SELECT count(*) FROM checkpoints WHERE run_id=?1",
                [&run_id],
                |row| row.get(0),
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert_eq!(checkpoint_count, 1);

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn cancel_by_run_id_stops_model_stream_and_reaches_terminal_under_two_seconds() {
    let run_id = format!(
        "kernel-v2-cancel-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let transport: Arc<dyn HttpTransport> = Arc::new(HangingTransport {
        started: started.clone(),
    });
    let chat = Arc::new(ChatService::new(db, settings, transport, &root));
    let runner = chat.clone();
    let task_run_id = run_id.clone();
    let task = tokio::spawn(async move {
        runner
            .run_in_session(
                "Wait until cancelled.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(&task_run_id),
                |_| {},
                |_| {},
            )
            .await
    });
    started.notified().await;
    let cancel_started = std::time::Instant::now();
    assert!(chat.cancel_session(&run_id));
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .expect("cancelled run exceeded two-second deadline")
        .unwrap();
    assert!(result.is_ok(), "cancel finalization failed: {result:?}");
    assert!(cancel_started.elapsed() < std::time::Duration::from_secs(2));
    let events = chat.run_events(&run_id, None).unwrap();
    assert!(events
        .iter()
        .any(|event| { event.event_type == "run_terminal" && event.data["kind"] == "cancelled" }));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn native_move_and_delete_are_verified_and_recorded_as_completion_evidence() {
    let run_id = format!(
        "kernel-v2-mutate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(root.join("remove-me/nested")).unwrap();
    std::fs::write(root.join("before.txt"), "move-content").unwrap();
    std::fs::write(root.join("remove-me/nested/file.txt"), "delete-content").unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();

    let move_turn = response_function_call(
        "call-move",
        "move_path",
        serde_json::json!({"source":"before.txt","destination":"after.txt"}),
    );
    let delete_turn = response_function_call(
        "call-delete",
        "delete_path",
        serde_json::json!({"path":"remove-me","recursive":true}),
    );
    let final_turn =
        response_text("Moved before.txt and deleted remove-me; filesystem checks passed.");
    let transport: Arc<dyn HttpTransport> =
        Arc::new(ReplayTransport::new([move_turn, delete_turn, final_turn]));
    let chat = ChatService::new(db, settings, transport, &root);

    chat.run_in_session(
        "Rename before.txt to after.txt, then delete remove-me recursively.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(&run_id),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    assert!(!root.join("before.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("after.txt")).unwrap(),
        "move-content"
    );
    assert!(!root.join("remove-me").exists());
    let events = chat.run_events(&run_id, None).unwrap();
    let evidence = events
        .iter()
        .find(|event| event.event_type == "completion_evidence")
        .expect("completion evidence event");
    let kinds = evidence.data["mutations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"created"));
    assert!(kinds.contains(&"deleted"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn completion_is_not_gated_by_prompt_keyword_extraction() {
    // Intent-layer cleanup 2026-07-28: completion is NEVER gated on
    // requirements guessed from the prompt. Upstream (Claude Code / codex /
    // grok) trust the model's self-report + Stop hooks + optional LLM
    // verification. A "delete X" prompt whose run does not actually delete X
    // therefore still completes (the model owns the honesty contract); the
    // kernel does not resurrect the removed keyword gate. A single turn is
    // enough — there is no feedback-retry loop keyed off prompt keywords.
    let run_id = format!(
        "kernel-v2-no-prompt-gate-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(root.join("target-dir")).unwrap();
    std::fs::write(root.join("target-dir/keep.txt"), "still-here").unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    let transport: Arc<dyn HttpTransport> = Arc::new(ReplayTransport::new([response_text(
        "Deleted target-dir successfully.",
    )]));
    let chat = ChatService::new(db, settings, transport, &root);

    let result = chat
        .run_in_session(
            "Delete target-dir recursively.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(&run_id),
            |_| {},
            |_| {},
        )
        .await;

    // No prompt gate: the run completes in one turn.
    assert!(result.is_ok(), "run must complete: {result:?}");
    let events = chat.run_events(&run_id, None).unwrap();
    // The keyword gate is gone: no completion-gate failure verification event
    // referencing "required filesystem effect(s)".
    assert!(!events.iter().any(|event| {
        event.event_type == "verification"
            && event.data["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("required filesystem effect"))
    }));
    assert!(events
        .iter()
        .any(|event| { event.event_type == "run_terminal" && event.data["kind"] == "succeeded" }));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn subagent_run_persists_terminal_state_and_independent_transcript() {
    let run_id = format!(
        "kernel-v2-subagent-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();

    let parent_task_turn = response_function_call(
        "call-subagent",
        "task",
        serde_json::json!({
            "description":"inspect fixture",
            "prompt":"Return exactly: subagent-ok"
        }),
    );
    let subagent_turn = response_text("subagent-ok");
    let parent_final_turn = response_text("Sub-agent returned subagent-ok.");
    let transport: Arc<dyn HttpTransport> = Arc::new(ReplayTransport::new([
        parent_task_turn,
        subagent_turn,
        parent_final_turn,
    ]));
    let chat = ChatService::new(db.clone(), settings, transport, &root);

    chat.run_in_session(
        "Ask a sub-agent for the fixture result.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(&run_id),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let records = deepagent_persistence::subagent_store::SubagentRunStore::new(&db)
        .list_for_parent(&run_id)
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, "succeeded");
    assert_eq!(records[0].summary.as_deref(), Some("subagent-ok"));
    let transcript_path = records[0]
        .transcript_path
        .as_ref()
        .map(PathBuf::from)
        .expect("subagent transcript path");
    let transcript: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transcript_path).unwrap()).unwrap();
    assert_eq!(transcript["summary"], "subagent-ok");
    assert_eq!(transcript["parent_run_id"], run_id);
    let parent_events = chat.run_events(&run_id, None).unwrap();
    assert!(parent_events
        .iter()
        .any(|event| event.event_type == "subagent_started"));
    assert!(parent_events
        .iter()
        .any(|event| event.event_type == "subagent_completed"));
    assert!(!parent_events
        .iter()
        .any(|event| event.event_type == "subagent_notification"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn background_subagent_outlives_parent_and_can_be_cancelled_independently() {
    let run_id = format!(
        "kernel-v2-background-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let child_started = Arc::new(tokio::sync::Notify::new());
    let transport: Arc<dyn HttpTransport> = Arc::new(BackgroundRoutingTransport {
        parent_calls: AtomicUsize::new(0),
        child_started: child_started.clone(),
    });
    let chat = Arc::new(ChatService::new(db, settings, transport, &root));

    chat.run_in_session(
        "Start one background child and return immediately.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(&run_id),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), child_started.notified())
        .await
        .expect("background child did not start");

    let records = chat.subagent_runs(&run_id).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, "running");
    let subagent_id = records[0].id.clone();
    assert!(chat.cancel_subagent(&subagent_id));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let record = chat.subagent_runs(&run_id).unwrap().remove(0);
        if record.state == "cancelled" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background child did not reach cancelled state: {record:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let parent_events = chat.run_events(&run_id, None).unwrap();
    assert!(parent_events
        .iter()
        .any(|event| { event.event_type == "run_terminal" && event.data["kind"] == "succeeded" }));
    let lifecycle: Vec<&str> = parent_events
        .iter()
        .filter_map(|event| match event.event_type.as_str() {
            "subagent_started" | "subagent_cancelled" | "subagent_notification" => {
                Some(event.event_type.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle,
        vec![
            "subagent_started",
            "subagent_cancelled",
            "subagent_notification"
        ]
    );
    let notification = parent_events
        .iter()
        .find(|event| event.event_type == "subagent_notification")
        .unwrap();
    assert_eq!(notification.data["state"], "cancelled");
    assert_eq!(notification.data["id"], subagent_id);

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn terminal_subagent_resumes_across_parent_runs_in_the_same_session() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let first_run = format!("kernel-v2-resume-a-{}-{stamp}", std::process::id());
    let second_run = format!("kernel-v2-resume-b-{}-{stamp}", std::process::id());
    let root = e2e_root(&first_run);
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let routing = Arc::new(ResumeRoutingTransport::default());
    let transport: Arc<dyn HttpTransport> = routing.clone();
    let chat = ChatService::new(db, settings.clone(), transport, &root);

    let session_id = chat
        .run_in_session(
            "Run one child.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(&first_run),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let first_record = chat.subagent_runs(&first_run).unwrap().remove(0);
    *routing
        .resume_id
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some(first_record.id.clone());

    chat.run_in_session(
        "Resume the prior child.",
        Some(&session_id),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(&second_run),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let resumed = chat.subagent_runs(&second_run).unwrap().remove(0);
    assert_eq!(resumed.id, first_record.id);
    assert_eq!(resumed.origin_parent_run_id, first_run);
    assert_eq!(resumed.parent_run_id, second_run);
    assert_eq!(resumed.resume_count, 1);
    assert_eq!(resumed.state, "succeeded");
    assert_eq!(resumed.summary.as_deref(), Some("resumed-result"));
    assert_eq!(chat.subagent_runs(&first_run).unwrap().len(), 1);
    let transcript: serde_json::Value =
        serde_json::from_slice(&std::fs::read(resumed.transcript_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(transcript["attempts"].as_array().unwrap().len(), 2);
    let events = chat.run_events(&second_run, None).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event_type == "subagent_started"));
    assert!(events
        .iter()
        .any(|event| event.event_type == "subagent_completed"));

    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn worktree_subagent_rebinds_file_tools_without_touching_main_checkout() {
    let run_id = format!(
        "kernel-v2-worktree-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init"]);
    run_git(
        &root,
        &["config", "user.email", "deepagent@example.invalid"],
    );
    run_git(&root, &["config", "user.name", "DeepAgent E2E"]);
    std::fs::write(root.join("README.md"), "fixture").unwrap();
    run_git(&root, &["add", "README.md"]);
    run_git(&root, &["commit", "-m", "fixture"]);

    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let turns = [
        response_function_call(
            "call-worktree-child",
            "task",
            serde_json::json!({
                "description":"isolated write",
                "prompt":"Create isolated.txt containing worktree-ok.",
                "isolation":"worktree"
            }),
        ),
        response_function_call(
            "call-isolated-write",
            "write_file",
            serde_json::json!({"path":"isolated.txt","content":"worktree-ok"}),
        ),
        response_text("Created isolated.txt in the worktree."),
        response_text("The isolated child completed successfully."),
    ];
    let transport: Arc<dyn HttpTransport> = Arc::new(ReplayTransport::new(turns));
    let chat = ChatService::new(db, settings, transport, &root);
    chat.run_in_session(
        "Create isolated.txt in an isolated worktree child and report its result.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(&run_id),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();

    let record = chat.subagent_runs(&run_id).unwrap().remove(0);
    let worktree = PathBuf::from(record.worktree_path.as_ref().unwrap());
    assert_eq!(
        std::fs::read_to_string(worktree.join("isolated.txt")).unwrap(),
        "worktree-ok"
    );
    assert!(!root.join("isolated.txt").exists());
    let events = chat.run_events(&run_id, None).unwrap();
    assert!(events
        .iter()
        .any(|event| event.event_type == "worktree_created"));
    let evidence = events
        .iter()
        .find(|event| event.event_type == "completion_evidence")
        .expect("parent completion evidence");
    assert!(evidence.data["mutations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|mutation| {
            mutation["kind"] == "created"
                && mutation["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("isolated.txt"))
        }));

    let worktree_base = worktree.parent().unwrap().to_path_buf();
    deepagent_subagents::GitWorktrees::new(&root, &worktree_base)
        .remove(&record.id)
        .await
        .unwrap();
    std::fs::remove_dir_all(worktree_base.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn run_git(repo: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Fault-injection (Phase G): the CHILD model fails terminally (503 on every
/// attempt) while the parent keeps working. Routes by the last request
/// message: the child's goal prompt errors, parent turns follow a script.
#[derive(Debug)]
struct FailingChildTransport {
    parent_calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for FailingChildTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let is_child = serde_json::from_str::<serde_json::Value>(&request.body)
            .ok()
            .is_some_and(|body| request_contains_user_input(&body, "child-must-fail"));
        if is_child {
            return Err(CoreError::provider(
                Some(503),
                Some("service_unavailable".into()),
                "child model upstream unavailable",
            ));
        }
        let call = self.parent_calls.fetch_add(1, Ordering::AcqRel);
        let events = if call == 0 {
            response_function_call(
                "call-failing-child",
                "task",
                serde_json::json!({
                    "description":"doomed child",
                    "prompt":"child-must-fail"
                }),
            )
        } else {
            response_text("The delegated child failed; reporting the error.")
        };
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn failed_subagent_propagates_paired_failure_and_parent_still_terminates() {
    let run_id = format!(
        "kernel-v2-childfail-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-e2e-test").await.unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    let transport: Arc<dyn HttpTransport> = Arc::new(FailingChildTransport {
        parent_calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db, settings, transport, &root);

    // The parent must complete despite the child's terminal failure.
    let result = chat
        .run_in_session(
            "Delegate one task to a child.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some(&run_id),
            |_| {},
            |_| {},
        )
        .await;
    assert!(
        result.is_ok(),
        "a failed child must not fail the parent run: {result:?}"
    );

    let events = chat.run_events(&run_id, None).unwrap();
    // Child lifecycle: started, then completed with state=failed.
    assert!(events
        .iter()
        .any(|event| event.event_type == "subagent_started"));
    let completed = events
        .iter()
        .find(|event| event.event_type == "subagent_completed")
        .expect("parent must record the child's terminal state");
    assert_eq!(completed.data["state"], "failed");
    // The `task` tool call is PAIRED with a failure result (no orphan).
    let tool_completed = events
        .iter()
        .find(|event| {
            event.event_type == "tool_completed" && event.data["call_id"] == "call-failing-child"
        })
        .expect("task tool call must have a paired completion");
    assert_eq!(tool_completed.data["ok"], false);
    // The parent itself still reached a clean succeeded terminal.
    assert!(events
        .iter()
        .any(|event| event.event_type == "run_terminal" && event.data["kind"] == "succeeded"));
    // No child left running.
    for record in chat.subagent_runs(&run_id).unwrap() {
        assert_eq!(record.state, "failed");
    }

    std::fs::remove_dir_all(root).unwrap();
}
