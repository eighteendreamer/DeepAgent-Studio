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
let pending;
let timer;
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
    pending = req;
    mark(process.env.AUDIT_STARTED, req);
    timer = setInterval(() => {
      if (!fs.existsSync(process.env.AUDIT_RELEASE)) return;
      clearInterval(timer);
      mark(process.env.AUDIT_RESPONDED, {requestId:req.id});
      reply(req.id, {content:[{type:'text',text:'{"ok":true}'}],isError:false});
      pending = undefined;
    }, 5);
  } else if (req.method === 'notifications/cancelled') {
    if (pending && req.params.requestId === pending.id) {
      clearInterval(timer);
      pending = undefined;
      mark(process.env.AUDIT_CANCELLED, req);
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
    settings
        .initialize("offline-audit-placeholder")
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
                        "command":"mcp__hook_cancel_audit__review", "timeout":5}]
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
        tokio::time::timeout(Duration::from_millis(250), &mut run)
            .await
            .ok()
    } else {
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
#[ignore = "audit H-236: every MCP hook must receive parent cancellation, including peer notification"]
async fn audit_mcp_hooks_propagate_cancellation_to_the_stdio_peer() {
    let mut observations = Vec::new();
    for event in EVENTS {
        let observed = observe(event, true).await;
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
