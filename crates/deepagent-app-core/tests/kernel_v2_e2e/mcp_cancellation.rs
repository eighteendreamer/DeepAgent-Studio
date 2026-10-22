//! A stopped run should interrupt its active MCP request and notify stdio servers.

use super::*;
use deepagent_app_core::{McpServerDto, McpService};
use deepagent_builtins::ToolSearchMode;
use std::time::Duration;

struct McpCancellationModel {
    calls: AtomicUsize,
}

#[async_trait]
impl HttpTransport for McpCancellationModel {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let events = match self.calls.fetch_add(1, Ordering::AcqRel) {
            0 => response_function_call(
                "audit-cancel-mcp-call",
                "mcp__cancel_audit__wait_for_release",
                serde_json::json!({}),
            ),
            _ => {
                return Err(CoreError::other(
                    "unexpected model request after cancellation",
                ))
            }
        };
        emit_responses_events(sink, &events)
    }
}

#[tokio::test]
async fn stopping_run_interrupts_inflight_stdio_mcp_call() {
    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("held-tool-mcp.cjs");
    let started = root.path().join("tool-call-started");
    let cancelled = root.path().join("tool-call-cancelled");
    let release = root.path().join("release-tool-call");
    std::fs::write(
        &script,
        r#"
const fs = require('node:fs');
const readline = require('node:readline');
function reply(id, result) {
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id, result }) + '\n');
}
readline.createInterface({ input: process.stdin }).on('line', line => {
  const request = JSON.parse(line);
  if (request.method === 'initialize') reply(request.id, {
    protocolVersion: request.params.protocolVersion,
    capabilities: { tools: {} },
    serverInfo: { name: 'cancel-audit', version: '1' }
  });
  else if (request.method === 'tools/list') reply(request.id, { tools: [{
    name: 'wait_for_release', description: 'Wait for test release', inputSchema: { type: 'object' }
  }] });
  else if (request.method === 'tools/call') {
    fs.writeFileSync(process.env.AUDIT_TOOL_STARTED, 'started');
    const timer = setInterval(() => {
      if (!fs.existsSync(process.env.AUDIT_TOOL_RELEASE)) return;
      clearInterval(timer);
      reply(request.id, { content: [{ type: 'text', text: 'released' }], isError: false });
    }, 10);
  } else if (request.method === 'notifications/cancelled') {
    fs.appendFileSync(process.env.AUDIT_CANCELLED, JSON.stringify(request) + '\n');
  } else if (request.id !== undefined) reply(request.id, {});
});
"#,
    )
    .unwrap();

    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
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

    let mcp = Arc::new(McpService::new(db.clone()));
    let dto: McpServerDto = serde_json::from_value(serde_json::json!({
        "name": "cancel_audit", "transport": "stdio", "enabled": true,
        "command": "node", "args": [script],
        "env": {
            "AUDIT_TOOL_STARTED": started,
            "AUDIT_CANCELLED": cancelled,
            "AUDIT_TOOL_RELEASE": release
        }
    }))
    .unwrap();
    mcp.upsert(dto).unwrap();
    mcp.start_preparation();
    let prepared = tokio::time::timeout(Duration::from_secs(5), async {
        while mcp.ready_registry().server_count() != 1 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    let ready = mcp.ready_registry();
    if prepared.is_err() {
        ready.close_all().await.unwrap();
        panic!("the local MCP fixture must be ready before the model run");
    }

    let model = Arc::new(McpCancellationModel {
        calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db.clone(), settings, model.clone(), root.path()).with_mcp(mcp);
    let session_id = chat.create_session(None).unwrap();
    let run_chat = chat.clone();
    let run_session_id = session_id.clone();
    let run = tokio::spawn(async move {
        run_chat
            .run_in_session(
                "Call the waiting MCP tool.",
                Some(&run_session_id),
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("audit-cancel-inflight-mcp"),
                |_| {},
                |_| {},
            )
            .await
    });

    tokio::time::timeout(Duration::from_secs(5), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the real MCP child must enter tools/call");

    let cancellation = chat.request_cancel(&session_id).unwrap();
    tokio::time::sleep(Duration::from_millis(250)).await;
    let run_still_waiting = !run.is_finished();
    let cancellation_notification_received = cancelled.exists();

    // Always release and join the real child before asserting, including on the
    // expected failing path, so the audit does not leave a fixture process.
    std::fs::write(&release, "release").unwrap();
    let run_result = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("the cancelled MCP tool call must let the run finish")
        .expect("chat task must not panic");
    ready.close_all().await.unwrap();
    let terminal = deepagent_persistence::run_store::RunStore::new(&db)
        .get("audit-cancel-inflight-mcp")
        .unwrap()
        .expect("the cancelled run must persist a terminal record");
    let events = deepagent_persistence::run_store::RunStore::new(&db)
        .events_after("audit-cancel-inflight-mcp", None)
        .unwrap();

    eprintln!(
        "MCP cancellation audit: accepted={}, run_still_waiting_after_250ms={}, cancellation_notification_received={}, terminal_kind={:?}, model_requests={}",
        cancellation.accepted,
        run_still_waiting,
        cancellation_notification_received,
        terminal.terminal_kind,
        model.calls.load(Ordering::Acquire)
    );
    assert!(
        cancellation.accepted,
        "the run must accept the stop request"
    );
    assert!(
        !run_still_waiting,
        "the active MCP tool await must stop promptly after run cancellation"
    );
    assert!(
        cancellation_notification_received,
        "stdio MCP server must receive notifications/cancelled for the active request"
    );
    assert!(
        run_result.is_ok(),
        "the cancelled run should finalize cleanly"
    );
    assert_eq!(terminal.terminal_kind.as_deref(), Some("cancelled"));
    assert_eq!(model.calls.load(Ordering::Acquire), 1);
    assert!(
        events.iter().any(|event| {
            let error = event.data["output"]["error"].as_str().unwrap_or_default();
            event.event_type == "tool_completed"
                && event.data["call_id"] == "audit-cancel-mcp-call"
                && event.data["ok"] == false
                && error.contains("remote outcome may be unknown")
        }),
        "the interrupted MCP tool call must have a paired failure result"
    );
}
