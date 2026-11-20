//! H-236 extension: parent cancellation must reach a real MCP hook request,
//! not merely abandon the local hook future at selected lifecycle boundaries.

use super::responses_fixture::{call, completed, text};
use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use deepagent_persistence::run_store::RunStore;
use deepagent_runtime::RuntimeEvent;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const RUN: &str = "audit-mcp-hook-cancel";
const MODEL: &str = "deepseek-flash";
const EVENTS: [&str; 4] = [
    "UserPromptSubmit",
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
];

const SERVER: &str = r#"
const fs = require('node:fs');
const readline = require('node:readline');
const pending = new Map();
function reply(id, result) {
  process.stdout.write(JSON.stringify({jsonrpc:'2.0', id, result}) + '\n');
}
function mark(path, value) {
  fs.writeFileSync(path + '.tmp', JSON.stringify(value));
  fs.renameSync(path + '.tmp', path);
}
readline.createInterface({input:process.stdin}).on('line', line => {
  const req = JSON.parse(line);
  if (req.method === 'initialize') reply(req.id, {
    protocolVersion:req.params.protocolVersion, capabilities:{tools:{}},
    serverInfo:{name:'mcp-hook-cancel-audit', version:'1'}
  });
  else if (req.method === 'tools/list') reply(req.id, {tools:[{
    name:'review', description:'Wait for the audit gate then allow the hook',
    inputSchema:{type:'object'}
  }]});
  else if (req.method === 'tools/call') {
    const slot = req.params.arguments.slot || '';
    mark(process.env.AUDIT_STARTED + slot, req);
    const timer = setInterval(() => {
      if (!fs.existsSync(process.env.AUDIT_RELEASE + slot)) return;
      clearInterval(timer);
      mark(process.env.AUDIT_RESPONDED + slot, {requestId:req.id});
      reply(req.id, {content:[{type:'text',text:'{"ok":true}'}],isError:false});
      pending.delete(req.id);
    }, 5);
    pending.set(req.id, {timer, slot});
  } else if (req.method === 'notifications/cancelled') {
    const active = pending.get(req.params.requestId);
    if (active) {
      clearInterval(active.timer);
      pending.delete(req.params.requestId);
      mark(process.env.AUDIT_CANCELLED + active.slot, req);
    }
  } else if (req.id !== undefined) reply(req.id, {});
});
process.stdin.on('end', () => process.exit(0));
setTimeout(() => process.exit(2), 15000).unref();
"#;

struct ParentModel {
    tool_site: bool,
    ordinary_mcp: bool,
    calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for ParentModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        assert_eq!(body["model"], MODEL);
        let index = self.calls.fetch_add(1, Ordering::AcqRel);
        let item = if self.ordinary_mcp && index == 0 {
            call(
                "mcp-hook-parent-read",
                "mcp__hook_cancel_audit__review",
                json!({"hook_event_name":"ordinary-tool-control"}),
            )
        } else if self.tool_site && index == 0 {
            call(
                "mcp-hook-parent-read",
                "read_file",
                json!({"path":"fixture.txt"}),
            )
        } else {
            text("mcp-hook-parent-answer", "Offline MCP hook audit complete.")
        };
        emit_responses_events(
            sink,
            &completed(
                &format!("mcp-hook-response-{index}"),
                MODEL,
                item,
                json!({"input_tokens":12,"output_tokens":6,"total_tokens":18}),
            ),
        )
    }
}

async fn wait_for_file(path: &Path) -> std::result::Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(4), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
}

#[derive(Debug)]
struct Observation {
    event: &'static str,
    stopped_before_release: bool,
    terminal_before_release: Option<String>,
    cancellation_received: bool,
    server_completed_after_release: bool,
    hook_completions: usize,
    parent_requests: usize,
    parent_tool_starts: usize,
    parent_tool_completions: usize,
}

struct Fixture {
    root: tempfile::TempDir,
    db: Arc<Database>,
    ready: Arc<deepagent_mcp::McpRegistry>,
    model: Arc<ParentModel>,
    chat: ChatService,
}

async fn fixture(event: &'static str) -> Fixture {
    fixture_with_timeout(event, 5).await
}

async fn fixture_with_timeout(event: &'static str, timeout: u64) -> Fixture {
    // This marker selects a normal model-issued tool call, not a hook event.
    let ordinary_mcp = event == "ordinary-tool-control";
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("hook-mcp.cjs");
    let started = root.path().join("started.json");
    let cancelled = root.path().join("cancelled.json");
    let release = root.path().join("release");
    let responded = root.path().join("responded.json");
    std::fs::write(&script, SERVER).unwrap();
    std::fs::write(root.path().join("fixture.txt"), "SYNTHETIC_MCP_HOOK_FILE").unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    settings
        .set_active_permission_preset(PermissionPreset::FullAccess)
        .unwrap();
    settings
        .set_tool_search_mode(ToolSearchMode::Disabled)
        .unwrap();
    if !ordinary_mcp {
        settings
            .set_hooks_json(
                &json!({"hooks":{event:[{
                    "matcher":"*", "hooks":[{"type":"mcp_tool",
                        "command":"mcp__hook_cancel_audit__review", "timeout":timeout}]
                }]}})
                .to_string(),
            )
            .unwrap();
    }
    let mcp = Arc::new(McpService::new(db.clone()));
    let dto: McpServerDto = serde_json::from_value(json!({
        "name":"hook_cancel_audit", "transport":"stdio", "enabled":true,
        "command":"node", "args":[script], "env":{
            "AUDIT_STARTED":started, "AUDIT_CANCELLED":cancelled,
            "AUDIT_RELEASE":release, "AUDIT_RESPONDED":responded
        }
    }))
    .unwrap();
    mcp.upsert(dto).unwrap();
    mcp.start_preparation();
    let prepared = tokio::time::timeout(Duration::from_secs(4), async {
        while mcp.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    let ready = mcp.ready_registry();
    if prepared.is_err() {
        ready.close_all().await.unwrap();
        panic!("MCP fixture must be ready before starting the run");
    }

    let tool_site = matches!(event, "PreToolUse" | "PostToolUse");
    let model = Arc::new(ParentModel {
        tool_site,
        ordinary_mcp,
        calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
    Fixture {
        root,
        db,
        ready,
        model,
        chat,
    }
}

async fn observe(event: &'static str, cancel: bool) -> Observation {
    observe_with_deferred_poll(event, cancel, false).await
}

async fn observe_with_deferred_poll(
    event: &'static str,
    cancel: bool,
    defer_hook_poll: bool,
) -> Observation {
    use std::future::Future;

    let Fixture {
        root,
        db,
        ready,
        model,
        chat,
    } = fixture(event).await;
    let started = root.path().join("started.json");
    let cancelled = root.path().join("cancelled.json");
    let release = root.path().join("release");
    let responded = root.path().join("responded.json");
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let hook_started = Arc::new(AtomicBool::new(false));
    let started_hook = hook_started.clone();
    let resume_poll = Arc::new(tokio::sync::Notify::new());
    let resumed = resume_poll.clone();
    let run_chat = chat.clone();
    let mut run = tokio::spawn(async move {
        let mut operation = Box::pin(run_chat
            .run_in_session(
                "Read fixture.txt and reply briefly.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some(RUN),
                move |runtime_event| {
                    if matches!(&runtime_event, RuntimeEvent::HookStarted { event: name, .. } if name == event) {
                        started_hook.store(true, Ordering::Release);
                    }
                    captured.lock().unwrap().push(runtime_event);
                },
                |_| {},
            ));
        if defer_hook_poll {
            // Park the owning future after its first hook poll, but let the
            // actual peer consume the frame. Cancellation then wins before
            // the Windows pipe's flush completion is polled by the owner.
            loop {
                let completed =
                    std::future::poll_fn(|cx| std::task::Poll::Ready(operation.as_mut().poll(cx)))
                        .await;
                if let std::task::Poll::Ready(result) = completed {
                    return result;
                }
                if hook_started.load(Ordering::Acquire) {
                    break;
                }
                tokio::task::yield_now().await;
            }
            resumed.notified().await;
        }
        operation.await
    });
    if wait_for_file(&started).await.is_err() {
        let _ = chat.request_cancel(RUN);
        std::fs::write(&release, "release").unwrap();
        run.abort();
        let _ = run.await;
        ready.close_all().await.unwrap();
        panic!("the formal hook must enter tools/call at {event}");
    }
    let request: Value = serde_json::from_str(&std::fs::read_to_string(&started).unwrap()).unwrap();
    assert_eq!(request["method"], "tools/call");
    assert_eq!(request["params"]["name"], "review");
    assert_eq!(request["params"]["arguments"]["hook_event_name"], event);
    assert!(!responded.exists());
    assert!(RunStore::new(&db)
        .get(RUN)
        .unwrap()
        .unwrap()
        .terminal_kind
        .is_none());
    let stop_result = if cancel {
        assert!(chat.request_cancel(RUN).unwrap().accepted);
        resume_poll.notify_one();
        tokio::time::timeout(Duration::from_millis(250), &mut run)
            .await
            .ok()
    } else {
        resume_poll.notify_one();
        None
    };
    let stopped_before_release = stop_result.is_some();
    let terminal_before_release = RunStore::new(&db).get(RUN).unwrap().unwrap().terminal_kind;
    // Give a fast local finalization the same bounded peer-notification window.
    if stopped_before_release {
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let cancellation_received = cancelled.exists();
    let responded_before_release = responded.exists();

    // Always release, join and close the real server before checking the target invariant.
    std::fs::write(&release, "release").unwrap();
    let settled = match stop_result {
        Some(result) => Some(result),
        None => tokio::time::timeout(Duration::from_secs(4), &mut run)
            .await
            .ok(),
    };
    if settled.is_none() {
        run.abort();
        let _ = run.await;
    }
    if !cancellation_received {
        let _ = wait_for_file(&responded).await;
    }
    let server_completed_after_release = responded.exists();
    ready.close_all().await.unwrap();
    let result = settled
        .expect("run must settle after gate release")
        .expect("run task must not panic");
    assert!(result.is_ok(), "run did not finalize cleanly: {result:?}");
    assert!(!responded_before_release);
    if server_completed_after_release {
        let response: Value =
            serde_json::from_str(&std::fs::read_to_string(responded).unwrap()).unwrap();
        assert_eq!(response["requestId"], request["id"]);
    }
    if cancellation_received {
        let notification: Value =
            serde_json::from_str(&std::fs::read_to_string(cancelled).unwrap()).unwrap();
        assert_eq!(notification["params"]["requestId"], request["id"]);
    }
    let terminal = RunStore::new(&db).get(RUN).unwrap().unwrap();
    assert_eq!(
        terminal.terminal_kind.as_deref(),
        Some(if cancel { "cancelled" } else { "succeeded" })
    );
    assert_eq!(
        chat.run_events(RUN, None)
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    let events = events.lock().unwrap();
    let mut hook_completions = 0;
    let mut parent_tool_starts = 0;
    let mut parent_tool_completions = 0;
    for runtime_event in events.iter() {
        match runtime_event {
            RuntimeEvent::HookCompleted { event: name, .. } if name == event => {
                hook_completions += 1
            }
            RuntimeEvent::ToolStarted { call_id, .. } if call_id == "mcp-hook-parent-read" => {
                parent_tool_starts += 1
            }
            RuntimeEvent::ToolCompleted { call_id, .. } if call_id == "mcp-hook-parent-read" => {
                parent_tool_completions += 1
            }
            _ => {}
        }
    }
    let observed = Observation {
        event,
        stopped_before_release,
        terminal_before_release,
        cancellation_received,
        server_completed_after_release,
        hook_completions,
        parent_requests: model.calls.load(Ordering::Acquire),
        parent_tool_starts,
        parent_tool_completions,
    };
    eprintln!("H-236 MCP hook cancel={cancel}: {observed:?}");
    observed
}

#[tokio::test]
async fn ordinary_mcp_tool_notifies_the_same_peer_on_cancellation() {
    let observed = observe("ordinary-tool-control", true).await;
    assert!(observed.stopped_before_release);
    assert_eq!(
        observed.terminal_before_release.as_deref(),
        Some("cancelled")
    );
    assert!(observed.cancellation_received);
    assert!(!observed.server_completed_after_release);
    assert_eq!(observed.hook_completions, 0);
    assert_eq!(observed.parent_requests, 1);
    assert_eq!(observed.parent_tool_starts, 1);
    assert_eq!(observed.parent_tool_completions, 1);
}

#[tokio::test]
async fn released_mcp_hooks_finish_at_all_four_boundaries() {
    for event in EVENTS {
        let observed = observe(event, false).await;
        assert_eq!(observed.event, event);
        assert!(!observed.cancellation_received);
        assert!(observed.server_completed_after_release);
        assert_eq!(observed.hook_completions, 1);
        let tool_site = matches!(event, "PreToolUse" | "PostToolUse");
        assert_eq!(observed.parent_requests, if tool_site { 2 } else { 1 });
        assert_eq!(observed.parent_tool_starts, usize::from(tool_site));
        assert_eq!(observed.parent_tool_completions, usize::from(tool_site));
    }
}

#[tokio::test]
async fn audit_mcp_hooks_propagate_cancellation_to_the_stdio_peer() {
    assert_hook_cancellation(false).await;
}

#[cfg(windows)]
#[tokio::test]
async fn h236_mcp_hook_cancel_after_peer_receipt_before_owner_repoll() {
    assert_hook_cancellation(true).await;
    // A parked owner without cancellation must still finish normally.
    let normal = observe_with_deferred_poll("UserPromptSubmit", false, true).await;
    assert!(!normal.cancellation_received);
    assert!(normal.server_completed_after_release);
    assert_eq!(normal.parent_requests, 1);
}

async fn assert_hook_cancellation(defer_hook_poll: bool) {
    let mut observations = Vec::new();
    for event in EVENTS {
        let observed = observe_with_deferred_poll(event, true, defer_hook_poll).await;
        let tool_site = matches!(event, "PreToolUse" | "PostToolUse");
        assert_eq!(observed.parent_requests, usize::from(tool_site));
        let read_count = usize::from(event == "PostToolUse");
        assert_eq!(observed.parent_tool_starts, read_count);
        assert_eq!(observed.parent_tool_completions, read_count);
        observations.push(observed);
    }
    assert!(
        observations.iter().all(|o| o.stopped_before_release
            && o.terminal_before_release.as_deref() == Some("cancelled")
            && o.cancellation_received
            && !o.server_completed_after_release),
        "MCP hook stop must end the local wait and notify the cooperative peer: {observations:?}"
    );
}

#[tokio::test]
async fn h236_hook_cancel_and_deadline_preserve_a_shared_peer_request() {
    use deepagent_tools::ToolExecutionContext;
    for deadline in [false, true] {
        let Fixture {
            root,
            db,
            ready,
            chat,
            ..
        } = fixture_with_timeout("PostToolUse", if deadline { 1 } else { 5 }).await;
        let run_chat = chat.clone();
        let mut run = tokio::spawn(async move {
            run_chat
                .run_in_session(
                    "Read fixture.txt.",
                    None,
                    None,
                    None,
                    Vec::new(),
                    None,
                    false,
                    Some(RUN),
                    |_| {},
                    |_| {},
                )
                .await
        });
        let peer = ready.clone();
        let mut sibling = None;
        let observation = tokio::time::timeout(Duration::from_secs(6), async {
            wait_for_file(&root.path().join("started.json")).await?;
            sibling = Some(tokio::spawn(async move {
                peer.invoke_with_context(
                    "mcp__hook_cancel_audit__review",
                    json!({"slot":"sibling"}),
                    ToolExecutionContext::default(),
                )
                .await
            }));
            wait_for_file(&root.path().join("started.jsonsibling")).await?;
            if !deadline {
                assert!(chat.request_cancel(RUN).unwrap().accepted);
            }
            let run_result = (&mut run).await.unwrap();
            wait_for_file(&root.path().join("cancelled.json")).await?;
            Ok::<_, tokio::time::error::Elapsed>(run_result)
        })
        .await;
        // Release/close all owned fixtures before any target assertion, even
        // when a regression leaves either foreground request waiting.
        std::fs::write(root.path().join("release"), "release").unwrap();
        std::fs::write(root.path().join("releasesibling"), "release").unwrap();
        let sibling_result = if let Some(mut sibling) = sibling {
            let result = tokio::time::timeout(Duration::from_secs(3), &mut sibling).await;
            if result.is_err() {
                sibling.abort();
                let _ = sibling.await;
            }
            Some(result)
        } else {
            None
        };
        if !matches!(&observation, Ok(Ok(_))) {
            let _ = chat.request_cancel(RUN);
            run.abort();
            let _ = run.await;
        }
        ready.close_all().await.unwrap();
        let run_result = observation.unwrap().unwrap();
        if !deadline {
            assert!(run_result.is_ok());
        }
        let request: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("started.json")).unwrap(),
        )
        .unwrap();
        let notification: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("cancelled.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(request["id"], notification["params"]["requestId"]);
        assert!(!root.path().join("responded.json").exists());
        assert!(!root.path().join("cancelled.jsonsibling").exists());
        assert!(!sibling_result.unwrap().unwrap().unwrap().unwrap().is_error);
        let terminal = RunStore::new(&db).get(RUN).unwrap().unwrap();
        assert_eq!(
            terminal.terminal_kind.as_deref(),
            Some(if deadline { "failed" } else { "cancelled" })
        );
        let events = chat.run_events(RUN, None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
        // The parent read completed before the post-tool hook timed out/stopped.
        let action = deepagent_persistence::run_control::RunControlStore::new(&db)
            .list_actions(RUN)
            .unwrap()
            .into_iter()
            .find(|action| action.call_id == "mcp-hook-parent-read")
            .unwrap();
        assert_eq!(
            action.state,
            deepagent_persistence::run_control::RunActionState::Completed
        );
    }
}
