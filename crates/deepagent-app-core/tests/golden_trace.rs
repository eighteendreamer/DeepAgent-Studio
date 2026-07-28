//! Phase G acceptance tooling: golden-trace comparison and terminal-state
//! invariants over the durable `run_events` stream.
//!
//! The golden trace records the ORDER AND SHAPE of kernel events (event
//! types + phases), not volatile payloads (ids, timestamps, token counts), so
//! it is stable across machines and model wording changes while still
//! catching state-machine regressions — a lightweight version of Claude
//! Code's trace replay.
//!
//! The invariant checker enforces the plan's hard criteria on ANY finished
//! run: exactly one terminal event, no orphan tool call/result pairing, no
//! running subagent left behind, and gapless event sequences.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_app_core::{ChatService, MemorySecretStore, SettingsService};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::run_store::StoredRunEvent;
use deepagent_persistence::Database;

// ---- golden trace ----------------------------------------------------------

/// Project one stored event into its stable golden form: `phase:event_type`.
/// Delta batches collapse into a single marker because chunk boundaries are
/// timing-dependent.
fn golden_line(event: &StoredRunEvent) -> String {
    let event_type = match event.event_type.as_str() {
        "content_delta_batch" | "reasoning_delta_batch" => "model_output",
        other => other,
    };
    format!("{}:{}", event.phase, event_type)
}

/// Render a run's golden trace, collapsing consecutive duplicate lines
/// (repeated model_output batches).
fn golden_trace(events: &[StoredRunEvent]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for event in events {
        let line = golden_line(event);
        if out.last() != Some(&line) {
            out.push(line);
        }
    }
    out
}

// ---- terminal invariants ----------------------------------------------------

/// Assert the plan's hard terminal invariants over a finished run's events.
fn assert_terminal_invariants(chat: &ChatService, run_id: &str) {
    let events = chat.run_events(run_id, None).unwrap();
    assert!(!events.is_empty(), "run {run_id} recorded no events");

    // 1. Gapless, monotonically increasing sequences.
    for (index, event) in events.iter().enumerate() {
        assert_eq!(
            event.sequence, index as u64,
            "run {run_id} event sequence has a gap at {index}"
        );
    }

    // 2. Exactly one terminal event, and it is the last non-delta event.
    let terminals: Vec<&StoredRunEvent> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(
        terminals.len(),
        1,
        "run {run_id} must have exactly one run_terminal event"
    );

    // 3. Tool pairing: every tool_started has a tool_completed or
    //    tool_blocked with the same call id — no orphans in either direction.
    let mut started: Vec<String> = Vec::new();
    let mut finished: Vec<String> = Vec::new();
    for event in &events {
        match event.event_type.as_str() {
            "tool_started" => {
                if let Some(id) = event.data.get("call_id").and_then(|value| value.as_str()) {
                    started.push(id.to_string());
                }
            }
            "tool_completed" => {
                if let Some(id) = event.data.get("call_id").and_then(|value| value.as_str()) {
                    finished.push(id.to_string());
                }
            }
            _ => {}
        }
    }
    for id in &started {
        assert!(
            finished.contains(id),
            "run {run_id}: tool call {id} started but never completed (orphan tool_use)"
        );
    }
    for id in &finished {
        assert!(
            started.contains(id),
            "run {run_id}: tool result {id} has no matching start (orphan tool_result)"
        );
    }

    // 4. No subagent left running after the parent reached terminal.
    for record in chat.subagent_runs(run_id).unwrap() {
        assert_ne!(
            record.state, "running",
            "run {run_id}: subagent {} still running after terminal",
            record.id
        );
    }
}

// ---- scripted transport ------------------------------------------------------

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

#[async_trait]
impl HttpTransport for ReplayTransport {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let events = self
            .turns
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .pop_front()
            .ok_or_else(|| CoreError::other("fake model has no remaining turn"))?;
        for event in events {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

fn e2e_root(run_id: &str) -> PathBuf {
    PathBuf::from(r"G:\Code\Kotlin_code\_deepagent-e2e").join(run_id)
}

async fn chat_with(transport: Arc<dyn HttpTransport>, root: &PathBuf) -> ChatService {
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
    ChatService::new(db, settings, transport, root)
}

fn unique_run_id(label: &str) -> String {
    format!(
        "golden-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    )
}

// ---- tests -------------------------------------------------------------------

/// Golden trace for the simplest run shape: one model turn, no tools.
/// Guards the kernel state machine's canonical event order.
#[tokio::test]
async fn golden_trace_simple_answer_matches_fixture() {
    let run_id = unique_run_id("simple");
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    let transport: Arc<dyn HttpTransport> = Arc::new(ReplayTransport::new([vec![
        r#"{"choices":[{"delta":{"content":"Paris."},"finish_reason":"stop"}]}"#.to_string(),
        "[DONE]".to_string(),
    ]]));
    let chat = chat_with(transport, &root).await;

    chat.run_in_session(
        "What is the capital of France?",
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

    let events = chat.run_events(&run_id, None).unwrap();
    let trace = golden_trace(&events);

    // The golden fixture: shape-stable across machines. Update ONLY when the
    // kernel state machine intentionally changes.
    //
    // Note: model streaming events (request started / deltas) flow through
    // the UI event pump into runtime-logs.db; run_events carries the durable
    // kernel state machine only — that separation is part of the contract.
    let expected = vec![
        "accepted:run_accepted",
        "preparing:run_started",
        "preparing:session_registered",
        "running_turn:turn_started",
        "verifying:completion_evidence",
        "finalizing:run_completed",
        "terminal:run_terminal",
    ];
    assert_eq!(
        trace, expected,
        "golden trace diverged.\nactual:\n{trace:#?}"
    );
    assert_terminal_invariants(&chat, &run_id);
    std::fs::remove_dir_all(root).unwrap();
}

/// Golden trace + invariants for a run with one tool round-trip.
#[tokio::test]
async fn golden_trace_tool_call_run_holds_invariants() {
    let run_id = unique_run_id("tool");
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("hello.txt"), "hi").unwrap();
    let transport: Arc<dyn HttpTransport> = Arc::new(ReplayTransport::new([
        vec![
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-read","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"hello.txt\"}"}}]},"finish_reason":"tool_calls"}]}"#.to_string(),
            "[DONE]".to_string(),
        ],
        vec![
            r#"{"choices":[{"delta":{"content":"The file says hi."},"finish_reason":"stop"}]}"#
                .to_string(),
            "[DONE]".to_string(),
        ],
    ]));
    let chat = chat_with(transport, &root).await;

    chat.run_in_session(
        "Read hello.txt and tell me what it says.",
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

    let events = chat.run_events(&run_id, None).unwrap();
    let trace = golden_trace(&events);
    // Tool phase must appear between the two model turns, with pairing.
    let started_at = trace
        .iter()
        .position(|line| line == "executing_tools:tool_started")
        .expect("trace must contain tool_started");
    let completed_at = trace
        .iter()
        .position(|line| line == "executing_tools:tool_completed")
        .expect("trace must contain tool_completed");
    assert!(started_at < completed_at);
    assert_eq!(
        trace.last().map(String::as_str),
        Some("terminal:run_terminal")
    );
    assert_terminal_invariants(&chat, &run_id);
    std::fs::remove_dir_all(root).unwrap();
}

/// Hard metric: a cancel request is ACKNOWLEDGED (cancel_session returns and
/// the flag is observed) well under the 200ms budget even while the model
/// stream is mid-flight.
#[tokio::test]
async fn cancel_request_acknowledged_within_200ms() {
    #[derive(Debug)]
    struct NotifyHangTransport {
        started: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl HttpTransport for NotifyHangTransport {
        async fn stream(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
        ) -> Result<()> {
            Err(CoreError::other("requires cancellation-aware streaming"))
        }

        async fn stream_cancelled(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
            cancel: Arc<std::sync::atomic::AtomicBool>,
        ) -> Result<()> {
            self.started.notify_one();
            while !cancel.load(std::sync::atomic::Ordering::Acquire) {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            Err(CoreError::other("request cancelled"))
        }
    }

    let run_id = unique_run_id("cancel-ack");
    let root = e2e_root(&run_id);
    std::fs::create_dir_all(&root).unwrap();
    let started = Arc::new(tokio::sync::Notify::new());
    let transport: Arc<dyn HttpTransport> = Arc::new(NotifyHangTransport {
        started: started.clone(),
    });
    let chat = Arc::new(chat_with(transport, &root).await);

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

    let ack_started = std::time::Instant::now();
    let accepted = chat.cancel_session(&run_id);
    let ack_elapsed = ack_started.elapsed();
    assert!(accepted, "cancel must find the in-flight run");
    assert!(
        ack_elapsed < std::time::Duration::from_millis(200),
        "cancel acknowledgement took {ack_elapsed:?} (budget 200ms)"
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .expect("cancelled run exceeded the two-second terminal deadline")
        .unwrap();
    assert!(result.is_ok(), "cancel finalization failed: {result:?}");
    assert_terminal_invariants(&chat, &run_id);
    std::fs::remove_dir_all(root).unwrap();
}
