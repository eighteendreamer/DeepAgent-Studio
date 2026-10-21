use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_app_core::{
    ChatService, CostService, MemorySecretStore, PermissionPreset, SettingsService,
};
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

/// Audit-only probe: records backend routing without running any host command.
#[tokio::test]
#[ignore = "audit reproducer: a per-turn sandbox override must not use the host default backend"]
async fn audit_sandbox_override_does_not_route_to_default_direct_backend() {
    use deepagent_app_core::{
        ApprovalPolicy, HarnessRunOverrides, SandboxBackend, SandboxBackendCommandExecutor,
        SandboxBackendKind, SandboxCapabilities, SandboxExecutionRequest, SandboxExecutionResult,
        SandboxMode,
    };
    use deepagent_builtins::bash_tool::CommandOutcome;

    struct RecordingDirectBackend(Arc<AtomicUsize>);

    #[async_trait]
    impl SandboxBackend for RecordingDirectBackend {
        fn capabilities(&self) -> SandboxCapabilities {
            SandboxCapabilities {
                kind: SandboxBackendKind::Direct,
                available: true,
                supports_one_shot: true,
                supports_interactive_pty: false,
                supports_network_toggle: false,
                supports_readonly_mapping: false,
                message: "audit fake: never executes a process".into(),
            }
        }

        async fn execute(
            &self,
            _request: SandboxExecutionRequest,
        ) -> Result<SandboxExecutionResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(SandboxExecutionResult {
                outcome: CommandOutcome {
                    exit_code: Some(0),
                    stdout: "audit fake output".into(),
                    stderr: String::new(),
                },
                backend: SandboxBackendKind::Direct,
                artifacts: Vec::new(),
            })
        }
    }

    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let discovery = Arc::new(MockTransport::with_get_json(
        r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
    ));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        discovery,
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    settings
        .set_sandbox_mode(SandboxMode::WorkspaceWrite)
        .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = SandboxBackendCommandExecutor::new(
        Arc::new(RecordingDirectBackend(calls.clone())),
        root.path(),
        SandboxMode::WorkspaceWrite,
    );
    let transport = Arc::new(ReplayTransport::new([
        response_function_call(
            "audit-command",
            "bash",
            serde_json::json!({"command":"echo audit"}),
        ),
        response_text("done"),
    ]));
    let chat = ChatService::new(db, settings, transport, root.path())
        .with_local_command_executor(Arc::new(executor));
    let result = chat
        .run_in_session_with_overrides(
            "Run the audit command using the requested sandbox.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-sandbox-override"),
            HarnessRunOverrides {
                sandbox_backend: Some("windows_sandbox".into()),
                ..Default::default()
            },
            |_| {},
            |_| {},
        )
        .await;
    eprintln!(
        "audit sandbox: run_ok={}, direct_backend_calls={}",
        result.is_ok(),
        calls.load(Ordering::SeqCst)
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "a windows_sandbox override must select that backend or reject the run, never execute using direct"
    );
}

#[tokio::test]
#[ignore = "audit integration: requires local Node to hold a real MCP initialization open"]
async fn pending_host_mcp_initialization_does_not_block_conversations() {
    use deepagent_app_core::{McpServerDto, McpService};
    use std::time::{Duration, Instant};

    struct ObservedModel {
        inner: ReplayTransport,
        requests: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl HttpTransport for ObservedModel {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            self.requests.lock().unwrap().push(request.body.clone());
            self.inner.stream(request, sink).await
        }
    }

    let root = tempfile::tempdir().unwrap();
    let script = root.path().join("slow-initialize.cjs");
    let started = root.path().join("initialize.log");
    let release = root.path().join("release");
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
  if (request.method === 'initialize') {
    fs.appendFileSync(process.env.AUDIT_INIT_LOG, 'initialize\n');
    const began = Date.now();
    const timer = setInterval(() => {
      if (!fs.existsSync(process.env.AUDIT_RELEASE) && Date.now() - began < 180000) return;
      clearInterval(timer);
      reply(request.id, { protocolVersion: request.params.protocolVersion,
        capabilities: { tools: {} }, serverInfo: { name: 'slow-audit', version: '1' } });
    }, 20);
  } else if (request.method === 'tools/list') {
    reply(request.id, { tools: [{ name: 'pending_audit_tool',
      description: 'Pending audit capability', inputSchema: { type: 'object' } }] });
  } else if (request.method === 'ping') {
    reply(request.id, {});
  }
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
    settings.initialize("sk-audit-placeholder").await.unwrap();
    let mcp = Arc::new(McpService::new(db.clone()));
    let dto: McpServerDto = serde_json::from_value(serde_json::json!({
        "name":"slow_audit", "transport":"stdio", "enabled":true,
        "command":"node", "args":[script],
        "env":{"AUDIT_INIT_LOG":started,"AUDIT_RELEASE":release}
    }))
    .unwrap();
    mcp.upsert(dto).unwrap();
    let model = Arc::new(ObservedModel {
        inner: ReplayTransport::new([response_text("first ready"), response_text("second ready")]),
        requests: Mutex::new(Vec::new()),
    });
    let chat = ChatService::new(db, settings, model.clone(), root.path()).with_mcp(mcp.clone());
    chat.start_plugin_preparation(Arc::new(|| {}));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real MCP child must have received initialize");
    assert!(mcp.ready_registry().all_tools().is_empty());

    let began = Instant::now();
    let mut session = None;
    let conversations = tokio::time::timeout(Duration::from_secs(5), async {
        for run_id in ["pending-mcp-first", "pending-mcp-second"] {
            session = Some(
                chat.run_in_session(
                    "Reply briefly.",
                    session.as_deref(),
                    None,
                    None,
                    Vec::new(),
                    None,
                    false,
                    Some(run_id),
                    |_| {},
                    |_| {},
                )
                .await?,
            );
        }
        Ok::<_, CoreError>(())
    })
    .await;
    let elapsed = began.elapsed();
    let pending_tools = mcp.ready_registry().all_tools();
    let init_count = std::fs::read_to_string(&started).unwrap().lines().count();

    // Release the held handshake and wait for publication before dropping the
    // fixture. This also verifies late readiness without leaking the child.
    std::fs::write(&release, "release").unwrap();
    let published = tokio::time::timeout(Duration::from_secs(5), async {
        while mcp.ready_registry().all_tools().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if published.is_ok() {
        mcp.ready_registry().close_all().await.unwrap();
    }
    eprintln!(
        "audit pending MCP: two_turns_ms={}, init_count={init_count}, pending_tools={}",
        elapsed.as_millis(),
        pending_tools.len()
    );
    assert!(matches!(conversations, Ok(Ok(()))), "{conversations:?}");
    assert!(published.is_ok(), "released server must become ready");
    assert!(
        pending_tools.is_empty(),
        "pending capabilities must not publish"
    );
    assert_eq!(
        init_count, 1,
        "conversation must not start another MCP connection"
    );
    let requests = model.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|body| !body.contains("pending_audit_tool")));
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
#[ignore = "audit: parallel reads must not overtake an earlier serial write in the same batch"]
async fn audit_unrelated_read_does_not_move_reads_before_write_barrier() {
    struct CaptureReplay {
        inner: ReplayTransport,
        requests: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl HttpTransport for CaptureReplay {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            self.requests.lock().unwrap().push(request.body.clone());
            self.inner.stream(request, sink).await
        }
    }

    let mut results = Vec::new();
    for extra_read in [false, true] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("other.txt"), "unrelated").unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(
                r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
            )),
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("sk-audit-placeholder").await.unwrap();
        let mut calls = vec![
            (
                "audit-write",
                "write_file",
                serde_json::json!({"path":"source.txt","content":"AUDIT_NEW_CONTENT"}),
            ),
            (
                "audit-read-source",
                "read_file",
                serde_json::json!({"path":"source.txt"}),
            ),
        ];
        if extra_read {
            calls.push((
                "audit-read-other",
                "read_file",
                serde_json::json!({"path":"other.txt"}),
            ));
        }
        let mut batch = Vec::new();
        for (id, name, args) in calls {
            let mut events = response_function_call(id, name, args);
            events.pop(); // one terminal marker for the whole model response
            batch.extend(events);
        }
        batch.push(
            serde_json::json!({"type":"response.completed","response":{"status":"completed"}})
                .to_string(),
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let order = Arc::new(Mutex::new(Vec::new()));
        let capture_order = order.clone();
        let chat = ChatService::new(
            db,
            settings,
            Arc::new(CaptureReplay {
                inner: ReplayTransport::new([batch, response_text("done")]),
                requests: requests.clone(),
            }),
            root.path(),
        );
        chat.run_in_session(
            "Create source.txt and read it back, then inspect other.txt if requested.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-barrier"),
            move |event| {
                if let deepagent_runtime::RuntimeEvent::ToolStarted { call_id, .. } = event {
                    capture_order.lock().unwrap().push(call_id);
                }
            },
            |_| {},
        )
        .await
        .unwrap();
        let disk = std::fs::read_to_string(root.path().join("source.txt")).unwrap();
        assert_eq!(
            disk, "AUDIT_NEW_CONTENT",
            "the real write must have completed"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let next: serde_json::Value = serde_json::from_str(&requests[1]).unwrap();
        let output = next["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| {
                item["type"] == "function_call_output" && item["call_id"] == "audit-read-source"
            })
            .expect("read result must reach the real next model request")["output"]
            .to_string();
        let read_saw_new = output.contains("AUDIT_NEW_CONTENT");
        let read_failed = output.contains("error");
        eprintln!(
            "extra_read={extra_read} live_order={:?} read_saw_new={read_saw_new} read_failed={read_failed} disk_is_new=true",
            order.lock().unwrap()
        );
        results.push((read_saw_new, read_failed));
    }
    assert_eq!(results[0], (true, false), "two-call serial control");
    assert_eq!(
        results[1],
        (true, false),
        "adding an unrelated safe read must not move an earlier write behind its dependent read"
    );
}

#[tokio::test]
#[ignore = "audit: model-visible saved tool results must survive turn completion and database reopen"]
async fn audit_saved_tool_result_survives_turn_and_database_reopen() {
    use deepagent_core::event::EventPayload;
    use deepagent_persistence::{artifact_store::ArtifactStore, event_store::EventStore};

    struct InspectSavedResult {
        inner: ReplayTransport,
        observed: Arc<Mutex<Option<(PathBuf, bool)>>>,
    }
    #[async_trait]
    impl HttpTransport for InspectSavedResult {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            let body: serde_json::Value = serde_json::from_str(&request.body)?;
            for item in body["input"].as_array().unwrap() {
                if item["type"] == "function_call_output" && item["call_id"] == "audit-large-result"
                {
                    let output: serde_json::Value = match item["output"].as_str() {
                        Some(text) => serde_json::from_str(text)?,
                        None => item["output"].clone(),
                    };
                    assert_eq!(output["status"], "ok");
                    let path = PathBuf::from(
                        output["result"]["saved_path"]
                            .as_str()
                            .expect("budgeted result"),
                    );
                    let existed = path.is_file();
                    let full = std::fs::read_to_string(&path).unwrap();
                    assert!(full.contains("AUDIT_RESULT_TAIL"));
                    *self.observed.lock().unwrap() = Some((path, existed));
                }
            }
            self.inner.stream(request, sink).await
        }
    }

    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("audit.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    // Fewer than 2000 lines, but above the production 4000-token result budget.
    let content = format!(
        "{}\nAUDIT_RESULT_TAIL",
        ("audit-row-".repeat(20) + "\n").repeat(200)
    );
    std::fs::write(root.path().join("large.txt"), &content).unwrap();
    let observed = Arc::new(Mutex::new(None));
    let chat = ChatService::new(
        db.clone(),
        settings,
        Arc::new(InspectSavedResult {
            inner: ReplayTransport::new([
                response_function_call(
                    "audit-large-result",
                    "read_file",
                    serde_json::json!({"path":"large.txt"}),
                ),
                response_text("done"),
            ]),
            observed: observed.clone(),
        }),
        root.path(),
    );
    let session_id = chat
        .run_in_session(
            "Inspect large.txt.",
            None,
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("audit-result-lifetime"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let (path, existed_during_tool) = observed.lock().unwrap().clone().unwrap();
    let exists_after_turn = path.is_file();
    drop(chat);
    drop(db);
    let reopened = Database::open(&db_path).unwrap();
    let artifacts = ArtifactStore::new(&reopened)
        .list_for_run("audit-result-lifetime")
        .unwrap();
    assert_eq!(
        artifacts.len(),
        1,
        "serial pipeline must index the full result"
    );
    assert_eq!(PathBuf::from(&artifacts[0].path), path);
    let events = EventStore::new(&reopened)
        .load_session(session_id.parse().unwrap())
        .unwrap();
    let persisted = events
        .iter()
        .find_map(|event| match &event.payload {
            EventPayload::ToolCallCompleted {
                call_id, output, ..
            } if call_id == "audit-large-result" => Some(output),
            _ => None,
        })
        .unwrap();
    assert_eq!(persisted["truncated"], true);
    assert_eq!(persisted["saved_path"].as_str(), path.to_str());
    assert!(
        !persisted.to_string().contains("AUDIT_RESULT_TAIL"),
        "tail must only exist in the saved full result"
    );
    let reread_ok = std::fs::read_to_string(&path).is_ok();
    eprintln!("during_tool={existed_during_tool} after_turn={exists_after_turn} reopened_artifacts={} replay_has_reference=true reread_ok={reread_ok}", artifacts.len());
    assert!(existed_during_tool);
    assert!(
        reread_ok,
        "a durable model-visible result reference must remain readable after the turn"
    );
}

#[tokio::test]
#[ignore = "audit reproducer: cancelled approval waiters must expire before late scoped answers"]
async fn audit_cancelled_run_rejects_late_approval_and_releases_waiter() {
    use deepagent_app_core::ApprovalPolicy;
    use deepagent_builtins::bash_tool::{CommandExecutor, CommandOutcome};
    use deepagent_persistence::run_control::{ApprovalState, RunActionState, RunControlStore};
    use std::time::Duration;

    struct NeverExecutes(Arc<AtomicUsize>);

    #[async_trait]
    impl CommandExecutor for NeverExecutes {
        async fn run(&self, _command: &str, _cwd: &str) -> Result<CommandOutcome> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::other(
                "audit fixture must never execute a process",
            ))
        }
    }

    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    settings
        .set_approval_policy(ApprovalPolicy::AlwaysAsk)
        .unwrap();
    let command_calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(ReplayTransport::new([response_function_call(
        "audit-waiting-approval",
        "bash",
        serde_json::json!({"command":"npm install audit-fixture-no-execution"}),
    )]));
    let chat = Arc::new(
        ChatService::new(db.clone(), settings, model, root.path())
            .with_local_command_executor(Arc::new(NeverExecutes(command_calls.clone()))),
    );
    let pending = chat.pending_approvals();
    let notified = Arc::new(tokio::sync::Notify::new());
    let runner = chat.clone();
    let notify = notified.clone();
    let task = tokio::spawn(async move {
        runner
            .run_in_session(
                "Request the audit command; wait for approval.",
                None,
                None,
                None,
                Vec::new(),
                None,
                false,
                Some("audit-cancelled-approval"),
                |_| {},
                move |_| notify.notify_one(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), notified.notified())
        .await
        .expect("tool must reach the real approval gate");
    assert_eq!(pending.pending_count(), 1);
    assert!(
        chat.request_cancel("audit-cancelled-approval")
            .unwrap()
            .accepted
    );
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("cancel must finish the run")
        .unwrap()
        .unwrap();
    let events = chat.run_events("audit-cancelled-approval", None).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_type == "run_terminal" && event.data["kind"] == "cancelled")
            .count(),
        1
    );
    let controls = RunControlStore::new(&db);
    let approvals = controls.list_approvals("audit-cancelled-approval").unwrap();
    assert_eq!(approvals.len(), 1);
    let action = controls
        .get_action("audit-cancelled-approval", "audit-waiting-approval")
        .unwrap()
        .unwrap();
    assert_eq!(action.state, RunActionState::Cancelled);
    assert_eq!(command_calls.load(Ordering::SeqCst), 0);

    let pending_after_terminal = pending.pending_count();
    let late = chat.resolve_approval_scoped(
        &approvals[0].approval_id,
        true,
        Some("tool_session"),
        "audit-late-user",
    );
    let granted_after_terminal = !pending.grants().list().is_empty();
    let last_event = chat
        .run_events("audit-cancelled-approval", None)
        .unwrap()
        .last()
        .unwrap()
        .event_type
        .clone();
    eprintln!(
        "audit cancelled approval: terminal=cancelled, action=cancelled, command_calls=0, pending_after_terminal={pending_after_terminal}, durable_state={:?}, late={late:?}, granted_after_terminal={granted_after_terminal}, last_event={last_event}",
        approvals[0].state
    );
    assert!(
        pending_after_terminal == 0
            && approvals[0].state != ApprovalState::Pending
            && !granted_after_terminal,
        "cancelled runs must release/expire approvals and reject stale permission grants"
    );
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

/// A text answer that also reports provider usage, so usage-gated behaviour
/// (auto-compaction, the session-note pass) can be driven offline.
fn answer_with_usage(text: &str, prompt: u32, completion: u32) -> Vec<String> {
    vec![
        serde_json::json!({"type":"response.output_text.delta","delta":text}).to_string(),
        serde_json::json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": {
                    "input_tokens": prompt,
                    "output_tokens": completion,
                    "total_tokens": prompt + completion,
                },
            },
        })
        .to_string(),
    ]
}

/// Answers the session-note pass by its own script and everything else with a
/// plain text turn. The pass is the only caller whose request names the note
/// file, which is what makes the routing unambiguous: a queue-position script
/// would be shifted by any other post-run model call (session title, hooks).
///
/// Each pass is a three-call round: read the note, edit it, then say it is done.
/// Ordinary turns report growing context, which is what opens the gate.
#[derive(Debug, Default)]
struct NotePassTransport {
    note_calls: AtomicUsize,
    run_calls: AtomicUsize,
    note_prompts: Mutex<Vec<String>>,
}

impl NotePassTransport {
    /// Context the nth turn reports. The step from 20k to 26k is above the
    /// gate's minimum delta; repeating 26k is below it.
    fn turn_usage(&self) -> (u32, u32) {
        match self.run_calls.fetch_add(1, Ordering::AcqRel) {
            0 => (20_000, 120),
            _ => (26_000, 140),
        }
    }
}

#[async_trait]
impl HttpTransport for NotePassTransport {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let events = if request.body.contains("session-memory.md") {
            let call = self.note_calls.fetch_add(1, Ordering::AcqRel);
            if call % 3 == 0 {
                self.note_prompts
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(request.body.clone());
                response_function_call(
                    "call-note-read",
                    "read_file",
                    serde_json::json!({"path": "session-memory.md"}),
                )
            } else if call % 3 == 1 {
                // Each round anchors on what the previous round wrote, the way a
                // real note-keeping agent appends instead of rewriting.
                let (old, new) = match call / 3 {
                    0 => (
                        "(nothing recorded yet)",
                        "- decision: one vault\n- backup: nightly",
                    ),
                    _ => ("- backup: nightly", "- backup: nightly, off-site"),
                };
                response_function_call(
                    "call-note-edit",
                    "edit_file",
                    serde_json::json!({"path": "session-memory.md", "old": old, "new": new}),
                )
            } else {
                answer_with_usage("note updated", 400, 60)
            }
        } else {
            let (prompt, completion) = self.turn_usage();
            answer_with_usage("recorded.", prompt, completion)
        };
        emit_responses_events(sink, &events)
    }
}

/// The session-note pass's durable commit: its cursors land in the `documents`
/// table only after the note has been edited and read back.
fn session_memory_state(db: &Database, session_id: &str) -> Option<serde_json::Value> {
    // `query_row` on an absent row is an error, and the error type is not
    // reachable from an integration test (rusqlite is not a dev-dependency), so
    // existence is probed with a count that always returns a row.
    let body: Option<String> = db
        .with_conn(|conn| {
            let present: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM documents WHERE collection = 'session_memory' AND id = ?1",
                    [session_id],
                    |row| row.get(0),
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            if present == 0 {
                return Ok(None);
            }
            conn.query_row(
                "SELECT body FROM documents WHERE collection = 'session_memory' AND id = ?1",
                [session_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    body.map(|body| serde_json::from_str(&body).unwrap())
}

fn cost_rows(db: &Database, session_id: &str) -> Vec<(i64, i64)> {
    db.with_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT input_tokens, output_tokens FROM costs WHERE session_id = ?1 \
                 ORDER BY input_tokens",
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|error| CoreError::Persistence(error.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        Ok(rows)
    })
    .unwrap()
}

/// Session-note wiring shared by the vertical tests: a real ChatService with a
/// cost ledger attached, served by the transport that scripts the note pass
/// separately from ordinary turns.
struct NoteE2e {
    base_run_id: String,
    root: PathBuf,
    db: Arc<Database>,
    transport: Arc<NotePassTransport>,
    chat: ChatService,
}

impl NoteE2e {
    async fn new(tag: &str) -> Self {
        let base_run_id = format!(
            "kernel-v2-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let root = e2e_root(&base_run_id);
        std::fs::create_dir_all(&root).unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let discovery: Arc<dyn HttpTransport> = Arc::new(MockTransport::with_get_json(
            r#"{"object":"list","data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        ));
        let settings = Arc::new(SettingsService::new(
            db.clone(),
            discovery,
            Arc::new(MemorySecretStore::new()),
        ));
        settings.initialize("sk-e2e-test").await.unwrap();
        let transport = Arc::new(NotePassTransport::default());
        let cost = Arc::new(CostService::new(db.clone()));
        let chat = ChatService::new(db.clone(), settings, transport.clone(), &root).with_cost(cost);
        Self {
            base_run_id,
            root,
            db,
            transport,
            chat,
        }
    }

    async fn run(&self, turn: u32, prompt: &str, session_id: Option<&str>) -> String {
        let run_id = format!("{}-t{turn}", self.base_run_id);
        self.chat
            .run_in_session(
                prompt,
                session_id,
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
            .unwrap()
    }

    fn note_path(&self, session_id: &str) -> PathBuf {
        let key =
            <deepagent_core::id::SessionId as std::str::FromStr>::from_str(session_id).unwrap();
        self.db.session_memory_path(key)
    }

    /// Wait for the background note pass to finish. The pass claims its slot
    /// inside the awaited run, so an idle result means the note, the cursors and
    /// the ledger row are already durable — no polling and no timing guess.
    async fn drain(&self) {
        assert!(
            self.chat
                .wait_for_session_memory_passes(std::time::Duration::from_secs(5))
                .await,
            "the note pass must finish on its own; a one-shot host would lose it"
        );
    }

    fn note_prompts(&self) -> Vec<String> {
        self.transport
            .note_prompts
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

/// A completed turn kicks off the session-note pass as a background kernel run:
/// it must edit the note that lives beside the session's event log, and bill its
/// own tokens. This is the vertical slice through the real run path (finalizer
/// scheduling, gate inputs from provider usage, the pinned read/edit channel).
#[tokio::test]
async fn completed_turn_writes_the_session_note_and_bills_the_pass() {
    let e2e = NoteE2e::new("note").await;
    let session_id = e2e.run(1, "Remember the storage decision.", None).await;
    e2e.drain().await;

    let note_path = e2e.note_path(&session_id);
    let note = std::fs::read_to_string(&note_path).unwrap_or_default();
    let note_calls = e2e.transport.note_calls.load(Ordering::Acquire);
    assert!(
        note.contains("decision: one vault"),
        "the note pass must edit the file beside the session log after {note_calls} pass calls; got {note:?} at {}",
        note_path.display()
    );

    // Cursors persisted, and the note covers the whole (tool-free) turn.
    let state =
        session_memory_state(&e2e.db, &session_id).expect("the note pass must commit its cursors");
    assert_eq!(state["initialized"], true);
    assert!(
        state["last_summarized_sequence"].is_number(),
        "a clean turn ends the note at a safe cut point: {state}"
    );

    // Two ledger rows, each carrying only its own call's usage: the turn's
    // 20k/120 and the note pass's 400/60. Folding the pass into the turn's row
    // would hide what maintenance actually costs.
    assert_eq!(
        cost_rows(&e2e.db, &session_id),
        vec![(400, 60), (20_000, 120)],
        "the turn and the note pass must bill separately for session {session_id}"
    );

    std::fs::remove_dir_all(&e2e.root).unwrap();
}

/// The two watermarks, proven through the real run path: a later pass folds in
/// only what the note does not cover yet, and a turn that grows the context by
/// less than the gate's minimum starts no pass and pays nothing.
#[tokio::test]
async fn a_later_pass_folds_only_the_uncovered_turn_and_a_flat_context_pays_nothing() {
    let e2e = NoteE2e::new("note-fold").await;
    let session_id = e2e.run(1, "Remember the storage decision.", None).await;
    e2e.drain().await;

    // The next turn reports 26k of context, six thousand above the note's own
    // watermark, which is the smallest growth the gate accepts.
    let continued = e2e
        .run(2, "Now remember the backup rule.", Some(&session_id))
        .await;
    assert_eq!(continued, session_id, "the turn continues the session");
    e2e.drain().await;

    let prompts = e2e.note_prompts();
    assert_eq!(prompts.len(), 2, "one opening prompt per pass round");
    assert!(
        prompts[0].contains("Remember the storage decision."),
        "the first round has to see the turn it is summarising: {}",
        prompts[0]
    );
    assert!(
        prompts[1].contains("Now remember the backup rule."),
        "the uncovered tail must reach the note: {}",
        prompts[1]
    );
    assert!(
        !prompts[1].contains("Remember the storage decision."),
        "the first turn is already covered, so re-sending it would pay a full window again"
    );
    let note_path = e2e.note_path(&session_id);
    let note_after_two = std::fs::read_to_string(&note_path).unwrap();
    assert!(
        note_after_two.contains("backup: nightly"),
        "the second round appends to the note it just read: {note_after_two:?}"
    );

    // A turn reporting no context growth must not start a pass at all. The pass
    // is claimed inside the awaited run, so an idle drain right after it is a
    // deterministic answer rather than a timing guess.
    let calls_before = e2e.transport.note_calls.load(Ordering::Acquire);
    e2e.run(3, "Nothing new to record.", Some(&session_id))
        .await;
    e2e.drain().await;
    assert_eq!(
        e2e.transport.note_calls.load(Ordering::Acquire),
        calls_before,
        "a flat context must not summarize"
    );
    assert_eq!(
        cost_rows(&e2e.db, &session_id),
        vec![
            (400, 60),
            (400, 60),
            (20_000, 120),
            (26_000, 140),
            (26_000, 140)
        ],
        "three turns and two passes, no third pass"
    );
    assert_eq!(
        std::fs::read_to_string(&note_path).unwrap(),
        note_after_two,
        "no pass, no note change"
    );

    std::fs::remove_dir_all(&e2e.root).unwrap();
}
