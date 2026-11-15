//! H-108: the same lifecycle wire projection reaches every action backend.
use super::*;
use crate::{SessionEndReason, SessionStartSource};
use serde_json::{json, Value};
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Default)]
struct Capture(Mutex<Vec<(String, Value)>>);

#[async_trait]
impl HookCommandRunner for Capture {
    async fn run(
        &self,
        _: &str,
        input: &str,
        _: HookCommandShell,
        _: &BTreeMap<String, String>,
        _: Duration,
    ) -> Result<HookCommandResult> {
        self.0
            .lock()
            .unwrap()
            .push(("command".into(), serde_json::from_str(input)?));
        Ok(HookCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[async_trait]
impl HookActionExecutor for Capture {
    async fn execute(&self, action: &HookAction, payload: Value) -> Result<HookOutcome> {
        self.0
            .lock()
            .unwrap()
            .push((format!("{:?}", action.action_type), payload));
        Ok(HookOutcome::Continue)
    }
}

async fn read_payload(socket: &mut tokio::net::TcpStream) -> Value {
    let mut reader = BufReader::new(socket);
    let mut length: Option<usize> = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).await.unwrap();
        header_bytes += count;
        assert!(count > 0 && header_bytes <= 8192);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse().unwrap());
            }
        }
    }
    let length = length.unwrap();
    assert!(length <= 8192);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn lifecycle_cases() -> Vec<(HookEvent, HookData, Value)> {
    let mut cases = Vec::new();
    for (source, wire) in [
        (SessionStartSource::Startup, "startup"),
        (SessionStartSource::Resume, "resume"),
        (SessionStartSource::Clear, "clear"),
        (SessionStartSource::Compact, "compact"),
        (SessionStartSource::Fork, "fork"),
    ] {
        assert_eq!(serde_json::to_value(source).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<SessionStartSource>(json!(wire)).unwrap(),
            source
        );
        cases.push((
            HookEvent::SessionStart,
            HookData::SessionStart { source },
            json!({"source":wire}),
        ));
    }
    for (reason, wire) in [
        (SessionEndReason::Clear, "clear"),
        (SessionEndReason::Resume, "resume"),
        (SessionEndReason::Logout, "logout"),
        (SessionEndReason::PromptInputExit, "prompt_input_exit"),
        (SessionEndReason::Other, "other"),
    ] {
        assert_eq!(serde_json::to_value(reason).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<SessionEndReason>(json!(wire)).unwrap(),
            reason
        );
        cases.push((
            HookEvent::SessionEnd,
            HookData::SessionEnd { reason },
            json!({"reason":wire}),
        ));
    }
    cases
}

#[tokio::test]
async fn h108_all_action_backends_receive_identical_typed_payloads_and_normal_controls() {
    payload_matrix(None).await;
}

#[tokio::test]
async fn h104_all_action_backends_preserve_common_input_and_event_fields() {
    let directory = tempfile::tempdir().unwrap();
    let file = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    payload_matrix(Some(crate::HookBaseInput {
        cwd: directory.path().to_str().unwrap().into(),
        transcript_path: file.path().to_str().unwrap().into(),
        transcript_format: "deepagent.events.v1".into(),
    }))
    .await;
}

async fn payload_matrix(base: Option<crate::HookBaseInput>) {
    let mut cases = lifecycle_cases();
    cases.extend([
        (
            HookEvent::InstructionsLoaded,
            HookData::InstructionsLoaded {
                file_path: "C:/synthetic/CLAUDE.md".into(),
                memory_type: crate::InstructionMemoryType::User,
                load_reason: crate::InstructionLoadReason::SessionStart,
            },
            json!({"file_path":"C:/synthetic/CLAUDE.md","paths":["C:/synthetic/CLAUDE.md"],"memory_type":"User","load_reason":"session_start"}),
        ),
        (
            HookEvent::InstructionsLoaded,
            HookData::InstructionsLoaded {
                file_path: "C:/synthetic/src/AGENTS.md".into(),
                memory_type: crate::InstructionMemoryType::Project,
                load_reason: crate::InstructionLoadReason::NestedTraversal,
            },
            json!({"file_path":"C:/synthetic/src/AGENTS.md","paths":["C:/synthetic/src/AGENTS.md"],"memory_type":"Project","load_reason":"nested_traversal"}),
        ),
        (
            HookEvent::UserPromptSubmit,
            HookData::prompt("synthetic"),
            json!({"prompt":"synthetic"}),
        ),
        (
            HookEvent::PreToolUse,
            HookData::before_tool_with_id(
                "read_file",
                json!({"path":"synthetic.txt"}),
                Some("call_pre_1".into()),
            ),
            json!({"tool_use_id":"call_pre_1","tool_name":"read_file","tool_input":{"path":"synthetic.txt"}}),
        ),
        (
            HookEvent::PostToolUse,
            HookData::after_tool_with_result(
                "read_file",
                json!({"path":"synthetic.txt"}),
                Some("call_post_1".into()),
                true,
                json!({"value":"synthetic result"}),
            ),
            json!({"tool_use_id":"call_post_1","tool_name":"read_file","tool_input":{"path":"synthetic.txt"},"tool_response":{"value":"synthetic result"}}),
        ),
        (
            HookEvent::PostToolUseFailure,
            HookData::after_tool_with_result(
                "read_file",
                json!({"path":"synthetic.txt"}),
                Some("call_post_failure_1".into()),
                false,
                json!({"error":"synthetic failure"}),
            ),
            json!({"tool_use_id":"call_post_failure_1","tool_name":"read_file","tool_input":{"path":"synthetic.txt"},"error":"synthetic failure"}),
        ),
        (
            HookEvent::PermissionRequest,
            HookData::Permission {
                tool: "read_file".into(),
                arguments: json!({}),
                reason: "approval required".into(),
            },
            json!({"tool_name":"read_file","tool_input":{},"reason":"approval required"}),
        ),
        (
            HookEvent::PermissionDenied,
            HookData::PermissionDenied {
                call_id: "call_denied_1".into(),
                tool: "read_file".into(),
                arguments: json!({"path":"synthetic.txt"}),
                reason: "ordinary rejection".into(),
            },
            json!({"tool_use_id":"call_denied_1","tool_name":"read_file","tool_input":{"path":"synthetic.txt"},"reason":"ordinary rejection"}),
        ),
        (
            HookEvent::PreCompact,
            HookData::Compact {
                trigger: "context_overflow".into(),
                summary: None,
            },
            json!({"trigger":"auto","custom_instructions":null}),
        ),
        (
            HookEvent::PostCompact,
            HookData::Compact {
                trigger: "micro_compact".into(),
                summary: Some("synthetic compact summary".into()),
            },
            json!({"trigger":"auto","compact_summary":"synthetic compact summary"}),
        ),
        (
            HookEvent::WorktreeCreate,
            HookData::WorktreeCreate {
                name: "feature-a".into(),
                path: "C:/worktrees/feature-a".into(),
            },
            json!({"name":"feature-a","path":"C:/worktrees/feature-a"}),
        ),
        (
            HookEvent::WorktreeRemove,
            HookData::WorktreeRemove {
                worktree_path: "C:/worktrees/feature-a".into(),
            },
            json!({"worktree_path":"C:/worktrees/feature-a","path":"C:/worktrees/feature-a"}),
        ),
        (
            HookEvent::CwdChanged,
            HookData::CwdChanged {
                old_cwd: "C:/workspace".into(),
                new_cwd: "C:/worktrees/feature-a".into(),
            },
            json!({"old_cwd":"C:/workspace","new_cwd":"C:/worktrees/feature-a","path":"C:/worktrees/feature-a"}),
        ),
        (
            HookEvent::FileChanged,
            HookData::FileChange {
                path: "src/main.rs".into(),
                kind: "modified".into(),
            },
            json!({"file_path":"src/main.rs","event":"change"}),
        ),
    ]);
    assert_payload_matrix(base, cases).await;
}

pub(super) async fn assert_payload_matrix(
    base: Option<crate::HookBaseInput>,
    cases: Vec<(HookEvent, HookData, Value)>,
) {
    let count = cases.len();
    let captured = Arc::new(Capture::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let http_capture = captured.clone();
    let server = tokio::spawn(async move {
        for _ in 0..count {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let payload = tokio::time::timeout(Duration::from_secs(5), read_payload(&mut socket))
                .await
                .unwrap();
            http_capture
                .0
                .lock()
                .unwrap()
                .push(("http".into(), payload));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        }
    });
    for (event, data, fields) in cases {
        let definitions = HookDefinitions::parse(
            &json!({"hooks":{event.label():[{
                "hooks":[
                    {"type":"command","command":"synthetic captured command","timeout":5},
                    {"type":"http","url":format!("http://{address}/synthetic"),"timeout":5},
                    {"type":"prompt","prompt":"synthetic captured prompt"},
                    {"type":"agent","prompt":"synthetic captured agent"},
                    {"type":"mcp_tool","command":"synthetic captured tool"}
                ]
            }]}})
            .to_string(),
        )
        .unwrap();
        let mut registry = HookRegistry::new();
        assert_eq!(
            definitions.register_into_with_host(&mut registry, captured.clone(), captured.clone()),
            5
        );
        let before = captured.0.lock().unwrap().len();
        let mut context =
            HookContext::new(deepagent_core::id::SessionId::nil(), event.point(), data);
        if let Some(base) = &base {
            context = context.with_base_input(base.clone());
        }
        registry.dispatch(&context).await.unwrap();
        let mut expected = fields.as_object().unwrap().clone();
        expected.insert("hook_event_name".into(), json!(event.label()));
        expected.insert("session_id".into(), json!(context.session_id.to_string()));
        if let Some(base) = &base {
            expected.insert("cwd".into(), json!(base.cwd));
            expected.insert("transcript_path".into(), json!(base.transcript_path));
            expected.insert("transcript_format".into(), json!(base.transcript_format));
        }
        let records = captured.0.lock().unwrap();
        assert_eq!(records.len() - before, 5, "{event:?}");
        for (backend, payload) in &records[before..] {
            assert_eq!(
                payload,
                &Value::Object(expected.clone()),
                "{event:?}/{backend}"
            );
        }
    }
    server.await.unwrap();
}

#[test]
fn h110_compaction_wire_uses_protocol_trigger_and_rejects_mismatched_points() {
    let mut context = HookContext::new(
        deepagent_core::id::SessionId::nil(),
        HookPoint::BeforeCompact,
        HookData::Compact {
            trigger: "manual".into(),
            summary: None,
        },
    );
    assert_eq!(
        super::build_hook_payload(HookEvent::PreCompact, &context),
        json!({
            "hook_event_name":"PreCompact",
            "session_id":deepagent_core::id::SessionId::nil().to_string(),
            "trigger":"manual",
            "custom_instructions":null,
        })
    );
    context.point = HookPoint::PostCompact;
    context.data = HookData::Compact {
        trigger: "auto_compact_threshold".into(),
        summary: Some("summary".into()),
    };
    assert_eq!(
        super::build_hook_payload(HookEvent::PostCompact, &context)["trigger"],
        "auto"
    );
    assert!(
        super::build_hook_payload(HookEvent::PreCompact, &context)
            .get("custom_instructions")
            .is_none(),
        "a mismatched compaction point must not manufacture PreCompact fields"
    );
}

#[test]
fn h112_worktree_wire_rejects_mismatched_event_data() {
    let context = HookContext::new(
        deepagent_core::id::SessionId::nil(),
        HookPoint::WorktreeCreate,
        HookData::WorktreeCreate {
            name: "feature-a".into(),
            path: "C:/worktrees/feature-a".into(),
        },
    );
    assert_eq!(
        super::build_hook_payload(HookEvent::WorktreeCreate, &context)["name"],
        "feature-a"
    );
    assert!(
        super::build_hook_payload(HookEvent::WorktreeRemove, &context)
            .get("worktree_path")
            .is_none(),
        "a mismatched worktree event must not manufacture its path field"
    );
}

#[test]
fn h113_file_change_wire_maps_known_kinds_and_rejects_unknown() {
    let context = HookContext::new(
        deepagent_core::id::SessionId::nil(),
        HookPoint::FileChanged,
        HookData::FileChange {
            path: "src/main.rs".into(),
            kind: "created".into(),
        },
    );
    assert_eq!(
        super::build_hook_payload(HookEvent::FileChanged, &context)["event"],
        "add"
    );
    let mut unknown = context;
    unknown.data = HookData::FileChange {
        path: "src/main.rs".into(),
        kind: "unchanged".into(),
    };
    assert!(
        super::build_hook_payload(HookEvent::FileChanged, &unknown)
            .get("file_path")
            .is_none(),
        "unknown mutation labels must fail closed"
    );
}

#[test]
fn h108_unknown_or_mismatched_metadata_is_not_invented() {
    for invalid in [json!(null), json!(1), json!("not_a_source")] {
        assert!(serde_json::from_value::<SessionStartSource>(invalid.clone()).is_err());
        assert!(serde_json::from_value::<SessionEndReason>(invalid).is_err());
    }
    // This legacy value was removed by the current reference protocol. Nothing
    // in production should claim that disabling permissions ended our session.
    assert!(
        serde_json::from_value::<SessionEndReason>(json!("bypass_permissions_disabled")).is_err()
    );
    for (event, point, data) in [
        (
            HookEvent::SessionStart,
            HookPoint::SessionStart,
            HookData::None,
        ),
        (HookEvent::SessionEnd, HookPoint::SessionEnd, HookData::None),
        (
            HookEvent::UserPromptSubmit,
            HookPoint::UserPromptSubmit,
            HookData::SessionStart {
                source: SessionStartSource::Startup,
            },
        ),
        (
            HookEvent::SessionStart,
            HookPoint::SessionStart,
            HookData::SessionEnd {
                reason: SessionEndReason::Other,
            },
        ),
        (
            HookEvent::SessionEnd,
            HookPoint::Stop,
            HookData::SessionEnd {
                reason: SessionEndReason::Other,
            },
        ),
        (
            HookEvent::PermissionDenied,
            HookPoint::PermissionRequest,
            HookData::PermissionDenied {
                call_id: "call_mismatched".into(),
                tool: "read_file".into(),
                arguments: json!({}),
                reason: "ordinary rejection".into(),
            },
        ),
        (
            HookEvent::PermissionRequest,
            HookPoint::PermissionDenied,
            HookData::PermissionDenied {
                call_id: "call_wrong_event".into(),
                tool: "read_file".into(),
                arguments: json!({}),
                reason: "ordinary rejection".into(),
            },
        ),
    ] {
        let context = HookContext::new(deepagent_core::id::SessionId::nil(), point, data);
        assert_eq!(
            build_hook_payload(event, &context),
            json!({
                "session_id":context.session_id.to_string(), "hook_event_name":event.label()
            })
        );
    }
}

#[test]
fn h103_tool_lifecycle_metadata_requires_the_matching_event_and_point() {
    for (event, point) in [
        (HookEvent::PreToolUse, HookPoint::AfterToolUse),
        (HookEvent::PostToolUse, HookPoint::BeforeToolUse),
        (HookEvent::PostToolUseFailure, HookPoint::AfterToolUse),
        (HookEvent::PostToolUseFailure, HookPoint::PostToolUseFailure),
    ] {
        let context = HookContext::new(
            deepagent_core::id::SessionId::nil(),
            point,
            HookData::after_tool_with_result(
                "read_file",
                json!({}),
                Some("call_mismatched".into()),
                true,
                json!({"private":"must not project"}),
            ),
        );
        let payload = build_hook_payload(event, &context);
        assert!(payload.get("tool_use_id").is_none(), "{event:?}/{point:?}");
        assert!(
            payload.get("tool_response").is_none(),
            "{event:?}/{point:?}"
        );
        assert!(payload.get("error").is_none(), "{event:?}/{point:?}");
    }

    let context = HookContext::new(
        deepagent_core::id::SessionId::nil(),
        HookPoint::BeforeToolUse,
        HookData::before_tool("read_file", json!({})),
    );
    assert!(build_hook_payload(HookEvent::PreToolUse, &context)
        .get("tool_use_id")
        .is_none());
}
