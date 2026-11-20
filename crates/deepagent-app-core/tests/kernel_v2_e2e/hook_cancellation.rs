//! Compare hook cancellation at preparation and sequential tool boundaries.
//! Only provider transport is synthetic; configuration, tools and run owner are real.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::Notify;

const MODEL: &str = "deepseek-flash";
const MARKER: &str = "AUDIT_HOOK_CANCEL_GATE";
const RUN: &str = "audit-hook-cancellation";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Site {
    Input,
    Start,
    Before,
    After,
}

impl Site {
    fn event(self) -> &'static str {
        match self {
            Self::Input => "UserPromptSubmit",
            Self::Start => "SessionStart",
            Self::Before => "PreToolUse",
            Self::After => "PostToolUse",
        }
    }
}

struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GatePoint {
    BeforeDecision,
    BeforeToolCall,
    StreamingDecision,
}

struct GatedModel {
    site: Site,
    agent: bool,
    gate: GatePoint,
    entered: Notify,
    release: Notify,
    waiting: AtomicUsize,
    answered: AtomicUsize,
    hooks: Mutex<Vec<Value>>,
    parent: AtomicUsize,
}

#[async_trait]
impl HttpTransport for GatedModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let hook = body["input"].to_string().contains(MARKER);
        let response_id = format!(
            "cancel-response-{}",
            self.parent.load(Ordering::Acquire) + self.hooks.lock().unwrap().len()
        );
        let mut gated = false;
        let item = if hook {
            let index = {
                let mut hooks = self.hooks.lock().unwrap();
                hooks.push(body);
                hooks.len() - 1
            };
            assert!(index <= usize::from(self.agent));
            gated = index == usize::from(self.agent && self.gate != GatePoint::BeforeToolCall);
            if self.agent && index == 0 {
                call("hook-cancel-list", "task_list", json!({}))
            } else {
                text("hook-cancel-decision", r#"{"ok":true}"#)
            }
        } else {
            let index = self.parent.fetch_add(1, Ordering::AcqRel);
            if matches!(self.site, Site::Before | Site::After) && index == 0 {
                call(
                    "parent-cancel-read",
                    "read_file",
                    json!({"path":"fixture.txt"}),
                )
            } else {
                text(
                    "parent-cancel-answer",
                    "Offline hook cancellation audit complete.",
                )
            }
        };
        let mut events = completed(
            &response_id,
            MODEL,
            item,
            json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
        );
        let split = if gated && self.gate == GatePoint::StreamingDecision {
            let partial = [
                json!({"type":"response.content_part.added","item_id":"hook-cancel-decision",
                    "output_index":0,"content_index":0,
                    "part":{"type":"output_text","text":"","annotations":[]}}),
                json!({"type":"response.output_text.delta","item_id":"hook-cancel-decision",
                    "output_index":0,"content_index":0,"delta":"{\"ok\":"}),
                json!({"type":"response.output_text.delta","item_id":"hook-cancel-decision",
                    "output_index":0,"content_index":0,"delta":"true}"}),
                json!({"type":"response.output_text.done","item_id":"hook-cancel-decision",
                    "output_index":0,"content_index":0,"text":"{\"ok\":true}"}),
                json!({"type":"response.content_part.done","item_id":"hook-cancel-decision",
                    "output_index":0,"content_index":0,
                    "part":{"type":"output_text","text":"{\"ok\":true}","annotations":[]}}),
            ];
            events.splice(2..2, partial.into_iter().map(|event| event.to_string()));
            for (sequence, event) in events.iter_mut().enumerate() {
                let mut value: Value = serde_json::from_str(event)?;
                value["sequence_number"] = sequence.into();
                *event = value.to_string();
            }
            emit_responses_events(sink, &events[..4])?;
            4 // The actual parser has received a partial decision, but no terminal.
        } else {
            0
        };
        if gated {
            self.waiting.fetch_add(1, Ordering::AcqRel);
            let _waiting = Waiting(&self.waiting);
            self.entered.notify_one();
            self.release.notified().await;
            self.answered.fetch_add(1, Ordering::AcqRel);
        }
        emit_responses_events(sink, &events[split..])
    }
}

async fn settings(db: Arc<Database>, site: Site, agent: bool) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", MODEL)
        .unwrap();
    settings
        .set_hooks_json(
            &json!({"hooks":{site.event():[{
            "matcher":if matches!(site, Site::Before | Site::After) {"read_file"} else {"*"},
                    "hooks":[{"type":if agent {"agent"} else {"prompt"},
                        "prompt":MARKER,"timeout":5}]
                }]}})
            .to_string(),
        )
        .unwrap();
    settings
}

#[derive(Debug, Default)]
struct Observation {
    stopped_before_release: bool,
    waiting_before_release: usize,
    terminal_before_release: Option<String>,
    answered: usize,
    parent_requests: usize,
    hook_starts: usize,
    hook_ends: usize,
    hook_requests_before_release: usize,
    hook_requests: usize,
    parent_tool_starts: usize,
    parent_tool_completions: usize,
}

fn verify_agent_tool(model: &GatedModel, may_stop_before_tool: bool) {
    let hooks = model.hooks.lock().unwrap();
    if may_stop_before_tool && model.agent && hooks.len() == 1 {
        return; // A future fix may cancel before receiving the first tool call.
    }
    assert_eq!(hooks.len(), if model.agent { 2 } else { 1 });
    if model.agent {
        let outputs: Vec<_> = hooks[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "hook-cancel-list"
            })
            .collect();
        assert_eq!(outputs.len(), 1);
        let output: Value = serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();
        assert_eq!(output["status"], "ok");
    }
}

fn observe_events(
    model: &GatedModel,
    events: &[RuntimeEvent],
    mut observed: Observation,
) -> Observation {
    observed.answered = model.answered.load(Ordering::Acquire);
    observed.parent_requests = model.parent.load(Ordering::Acquire);
    observed.hook_requests = model.hooks.lock().unwrap().len();
    for event in events {
        match event {
            RuntimeEvent::HookStarted { event, .. } if event == model.site.event() => {
                observed.hook_starts += 1;
            }
            RuntimeEvent::HookCompleted { event, .. } if event == model.site.event() => {
                observed.hook_ends += 1;
            }
            RuntimeEvent::ToolStarted { call_id, .. } if call_id == "parent-cancel-read" => {
                observed.parent_tool_starts += 1;
            }
            RuntimeEvent::ToolCompleted { call_id, .. } if call_id == "parent-cancel-read" => {
                observed.parent_tool_completions += 1;
            }
            _ => {}
        }
    }
    observed
}

async fn observe(site: Site, agent: bool, cancel: bool) -> Observation {
    observe_wait(site, agent, cancel, false).await
}

async fn observe_wait(
    site: Site,
    agent: bool,
    cancel: bool,
    gate_before_tool: bool,
) -> Observation {
    observe_gate(
        site,
        agent,
        cancel,
        if gate_before_tool {
            GatePoint::BeforeToolCall
        } else {
            GatePoint::BeforeDecision
        },
    )
    .await
}

async fn observe_gate(site: Site, agent: bool, cancel: bool, gate: GatePoint) -> Observation {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("fixture.txt"), "AUDIT_CANCEL_FILE").unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = settings(db.clone(), site, agent).await;
    let model = Arc::new(GatedModel {
        site,
        agent,
        gate,
        entered: Notify::new(),
        release: Notify::new(),
        waiting: AtomicUsize::new(0),
        answered: AtomicUsize::new(0),
        hooks: Mutex::new(Vec::new()),
        parent: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path());
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let run_chat = chat.clone();
    let mut run = tokio::spawn(async move {
        run_chat
            .run_in_session(
                "Read fixture.txt and reply briefly.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(RUN),
                move |event| captured.lock().unwrap().push(event),
                |_| {},
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(4), model.entered.notified())
        .await
        .expect("the configured hook must really enter its model wait");
    if gate != GatePoint::BeforeToolCall {
        verify_agent_tool(&model, false);
    }
    assert_eq!(model.waiting.load(Ordering::Acquire), 1);
    assert!(RunStore::new(&db)
        .get(RUN)
        .unwrap()
        .unwrap()
        .terminal_kind
        .is_none());

    let stopped = if cancel {
        assert!(chat.request_cancel(RUN).unwrap().accepted);
        tokio::time::timeout(Duration::from_millis(200), &mut run)
            .await
            .ok()
    } else {
        None
    };
    let stopped_before_release = stopped.is_some();
    let waiting_before_release = model.waiting.load(Ordering::Acquire);
    let hook_requests_before_release = model.hooks.lock().unwrap().len();
    let terminal_before_release = RunStore::new(&db).get(RUN).unwrap().unwrap().terminal_kind;
    // Release and join even when the desired cancellation invariant failed.
    model.release.notify_one();
    let result = match stopped {
        Some(result) => result,
        None => tokio::time::timeout(Duration::from_secs(4), run)
            .await
            .expect("audit workflow must settle after releasing the provider"),
    }
    .expect("chat task must not panic");
    assert!(
        result.is_ok(),
        "formal cancellation/success must settle: {result:?}"
    );
    assert_eq!(model.waiting.load(Ordering::Acquire), 0);
    verify_agent_tool(&model, gate == GatePoint::BeforeToolCall && cancel);
    let terminal = RunStore::new(&db).get(RUN).unwrap().unwrap();
    assert_eq!(
        terminal.terminal_kind.as_deref(),
        Some(if cancel { "cancelled" } else { "succeeded" })
    );
    assert_eq!(
        chat.run_events(RUN, None)
            .unwrap()
            .iter()
            .filter(|event| event.event_type == "run_terminal")
            .count(),
        1
    );
    let events = events.lock().unwrap();
    let observation = observe_events(
        &model,
        &events,
        Observation {
            stopped_before_release,
            waiting_before_release,
            terminal_before_release,
            hook_requests_before_release,
            ..Observation::default()
        },
    );
    eprintln!("hook cancellation: site={site:?}, agent={agent}, cancel={cancel}, gate={gate:?}, {observation:?}");
    assert_eq!(observation.hook_starts, 1);
    observation
}

#[tokio::test]
async fn released_model_hooks_finish_at_each_boundary() {
    for site in [Site::Input, Site::Start, Site::Before, Site::After] {
        for agent in [false, true] {
            let observed = observe(site, agent, false).await;
            assert_eq!(observed.answered, 1);
            assert_eq!(observed.hook_ends, 1);
            assert_eq!(
                observed.parent_requests,
                if matches!(site, Site::Input | Site::Start) {
                    1
                } else {
                    2
                }
            );
            let tool_count = usize::from(matches!(site, Site::Before | Site::After));
            assert_eq!(observed.parent_tool_starts, tool_count);
            assert_eq!(observed.parent_tool_completions, tool_count);
        }
    }
}

#[tokio::test]
async fn input_and_runtime_hooks_drop_the_wait_when_run_is_cancelled() {
    for site in [Site::Input, Site::Start] {
        for agent in [false, true] {
            let observed = observe(site, agent, true).await;
            assert!(observed.stopped_before_release);
            assert_eq!(observed.waiting_before_release, 0);
            assert_eq!(
                observed.terminal_before_release.as_deref(),
                Some("cancelled")
            );
            assert_eq!(observed.answered, 0);
            assert_eq!(observed.parent_requests, 0);
            assert_eq!(observed.parent_tool_starts, 0);
        }
    }
}

async fn assert_tool_hook_cancellation(site: Site) {
    let mut observations = Vec::new();
    for (agent, gate_before_tool) in [(false, false), (true, false), (true, true)] {
        let observed = observe_wait(site, agent, true, gate_before_tool).await;
        assert_eq!(observed.parent_requests, 1);
        let tool_count = usize::from(site == Site::After);
        assert_eq!(observed.parent_tool_starts, tool_count);
        assert_eq!(observed.parent_tool_completions, tool_count);
        observations.push(observed);
    }
    assert!(
        observations
            .iter()
            .all(|observed| observed.stopped_before_release
                && observed.waiting_before_release == 0
                && observed.answered == 0
                && observed.hook_requests == observed.hook_requests_before_release),
        "stop must reach each sequential hook wait: {observations:?}"
    );
}

#[tokio::test]
async fn audit_pre_tool_model_hooks_observe_run_cancellation() {
    assert_tool_hook_cancellation(Site::Before).await;
}

#[tokio::test]
async fn audit_post_tool_model_hooks_observe_run_cancellation() {
    assert_tool_hook_cancellation(Site::After).await;
}

#[tokio::test]
async fn h236_streaming_hook_decisions_cancel_without_consuming_the_terminal() {
    for site in [Site::Before, Site::After] {
        for agent in [false, true] {
            for cancel in [false, true] {
                let observed =
                    observe_gate(site, agent, cancel, GatePoint::StreamingDecision).await;
                assert_eq!(observed.answered, usize::from(!cancel));
                assert_eq!(observed.parent_requests, if cancel { 1 } else { 2 });
                let tools = usize::from(!cancel || site == Site::After);
                assert_eq!(observed.parent_tool_completions, tools);
                if cancel {
                    assert!(observed.stopped_before_release);
                    assert_eq!(observed.waiting_before_release, 0);
                    assert_eq!(
                        observed.hook_requests,
                        observed.hook_requests_before_release
                    );
                }
            }
        }
    }
}
