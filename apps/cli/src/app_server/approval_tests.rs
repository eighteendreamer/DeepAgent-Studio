//! H-84: real gate notifications, wire responses, effects and durable outcomes.
use super::backpressure_tests::{pipe_server, PipeClient};
use super::*;
use deepagent_persistence::run_control::{ApprovalState, RunActionState, RunControlStore};
use tokio::io::AsyncWriteExt;

async fn wire_message(client: &mut PipeClient, events: &mut Vec<Value>) -> Option<Value> {
    let line = tokio::time::timeout(Duration::from_secs(10), client.lines.next_line())
        .await
        .expect("bounded wire read")
        .unwrap()?;
    let message: Value = serde_json::from_str(&line).unwrap();
    if message["method"] == "harness/event" {
        events.push(message["params"].clone());
    }
    Some(message)
}

async fn wire_response(
    client: &mut PipeClient,
    events: &mut Vec<Value>,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    client
        .input
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    loop {
        let message = wire_message(client, events).await.expect("wire response");
        if message["id"] == id {
            return message;
        }
    }
}

#[tokio::test]
async fn h84_wire_approval_round_trip_and_session_grant_keep_one_actionable_prompt() {
    for (policy, approved, scope) in [
        (ApprovalPolicy::AlwaysAsk, true, "single_call"),
        (ApprovalPolicy::AlwaysAsk, false, "single_call"),
        (ApprovalPolicy::AlwaysAsk, true, "tool_session"),
        (ApprovalPolicy::AutoReview, true, "single_call"),
        (ApprovalPolicy::AutoReview, false, "single_call"),
    ] {
        let granted = scope == "tool_session";
        let mut script = vec![function_call(
            "h84-first",
            "delete_path",
            json!({"path":"first.txt"}),
        )];
        if granted {
            script.push(function_call(
                "h84-second",
                "delete_path",
                json!({"path":"second.txt"}),
            ));
        }
        script.push(completed_text("H84_DONE"));
        let fixture = Fixture::new_scripted(script).await;
        fixture.settings.set_approval_policy(policy).unwrap();
        for name in ["first.txt", "second.txt"] {
            std::fs::write(fixture.workspace.join(name), "synthetic unchanged content").unwrap();
        }
        let chat = fixture.state.lock().unwrap().base_chat.clone();
        let (mut client, server) = pipe_server(&fixture);
        client
            .call(
                1,
                "initialize",
                json!({"clientName":"h84","clientVersion":"1","protocolVersion":PROTOCOL_VERSION}),
            )
            .await;
        client
            .call(2, "thread/resume", json!({"threadId":fixture.thread_id}))
            .await;
        let mut events = Vec::new();
        let started = wire_response(
            &mut client,
            &mut events,
            3,
            "turn/start",
            json!({"threadId":fixture.thread_id,"input":"Test synthetic approvals."}),
        )
        .await;
        let turn = started["result"]["turnId"].as_str().unwrap().to_owned();
        let approval_id = loop {
            wire_message(&mut client, &mut events)
                .await
                .expect("approval before EOF");
            if let Some(id) = events
                .iter()
                .find(|e| e["type"] == "approval.requested")
                .and_then(|e| e["approvalId"].as_str())
            {
                break id.to_owned();
            }
        };
        assert!(!approval_id.is_empty());
        assert_eq!(chat.pending_approvals().pending_count(), 1);
        assert_eq!(
            std::fs::read_to_string(fixture.workspace.join("first.txt")).unwrap(),
            "synthetic unchanged content"
        );
        let db = Database::open(&fixture.database_path).unwrap();
        assert_eq!(
            RunControlStore::new(&db)
                .get_approval(&approval_id)
                .unwrap()
                .unwrap()
                .state,
            ApprovalState::Pending
        );

        let wrong = wire_response(
            &mut client,
            &mut events,
            4,
            "approval/respond",
            json!({"approvalId":"h84-not-a-pending-request","approved":true}),
        )
        .await;
        assert!(wrong.get("error").is_some());
        assert_eq!(chat.pending_approvals().pending_count(), 1);
        let response = wire_response(
            &mut client,
            &mut events,
            5,
            "approval/respond",
            json!({"approvalId":approval_id,"approved":approved,"scope":scope}),
        )
        .await;
        assert!(response.get("result").is_some(), "{response}");
        while !events
            .iter()
            .any(|e| e["type"] == "turn.completed" && e["turnId"] == turn)
        {
            wire_message(&mut client, &mut events)
                .await
                .expect("terminal before EOF");
        }
        // A late duplicate cannot escape the assertion: drain the real writer.
        client.input.shutdown().await.unwrap();
        while wire_message(&mut client, &mut events).await.is_some() {}
        tokio::time::timeout(Duration::from_secs(10), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let requests: Vec<_> = events
            .iter()
            .filter(|e| e["type"] == "approval.requested")
            .collect();
        assert_eq!(requests.len(), 1, "{scope}/{approved}: {requests:?}");
        assert_eq!(requests[0]["approvalId"], approval_id);
        assert_eq!(requests[0]["threadId"], fixture.thread_id);
        assert_eq!(requests[0]["turnId"], turn);
        assert!(events
            .iter()
            .any(|e| e["item"]["eventType"] == "tool_blocked"
                && e["item"]["data"]["needs_approval"] == true));
        assert_eq!(chat.pending_approvals().pending_count(), 0);
        assert_eq!(fixture.workspace.join("first.txt").exists(), !approved);
        assert_eq!(fixture.workspace.join("second.txt").exists(), !granted);
        // A new SQLite connection reads the actual response and terminal, not
        // an in-memory mirror. No replay or second dispatch is used to verify.
        drop(db);
        let reopened = Database::open(&fixture.database_path).unwrap();
        let record = RunControlStore::new(&reopened)
            .get_approval(&approval_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            record.state,
            if approved {
                ApprovalState::Approved
            } else {
                ApprovalState::Denied
            }
        );
        assert_eq!(record.scope, scope);
        let run = RunStore::new(&reopened).get(&turn).unwrap().unwrap();
        assert!(run.finished_at.is_some());
        assert_eq!(run.terminal_kind.as_deref(), Some("succeeded"));
        assert_eq!(
            RunStore::new(&reopened)
                .events_after(&turn, None)
                .unwrap()
                .iter()
                .filter(|e| e.event_type == "run_terminal")
                .count(),
            1
        );
    }
}

fn ask_hook(fixture: &Fixture, tool: &str) {
    let body = r#"{"permissionDecision":"ask","permissionDecisionReason":"h84 hook ask"}"#;
    let (command, shell) = if cfg!(windows) {
        (format!("Write-Output '{body}'"), "powershell")
    } else {
        (format!("printf '%s' '{body}'"), "sh")
    };
    fixture
        .settings
        .set_hooks_json(
            &json!({"hooks":{"PreToolUse":[{"matcher":tool,"hooks":[{
                "type":"command","command":command,"shell":shell,"timeout":5
            }]}]}})
            .to_string(),
        )
        .unwrap();
}

#[tokio::test]
async fn h84_policy_auto_deny_keeps_diagnostic_but_never_prompts_or_executes() {
    // This is a synthetic classifier marker, not a key path or real command.
    let fixture = Fixture::new_scripted(vec![
        function_call(
            "h84-denied",
            "bash",
            json!({"command":"echo h84.pem > denied-canary.txt"}),
        ),
        completed_text("H84_DENIED"),
    ])
    .await;
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::AutoReview)
        .unwrap();
    ask_hook(&fixture, "bash");
    let turn = fixture.run_turn().await;
    let events = fixture.events.lock().unwrap();
    assert!(!events.iter().any(|e| e["type"] == "approval.requested"));
    assert!(events
        .iter()
        .any(|e| e["item"]["eventType"] == "tool_blocked"
            && e["item"]["data"]["needs_approval"] == true));
    // A blocked preparation does not start/complete a tool. Its final denial
    // uses the existing error projection and durable action record.
    assert!(events.iter().any(|e| e["type"] == "error"
        && e["code"] == "tool_blocked"
        && e["message"].as_str().unwrap().contains("approval denied")));
    assert!(!events.iter().any(|e| e["item"]["callId"] == "h84-denied"));
    drop(events);
    assert!(!fixture.workspace.join("denied-canary.txt").exists());
    let db = Database::open(&fixture.database_path).unwrap();
    assert_eq!(
        RunControlStore::new(&db)
            .get_action(&turn, "h84-denied")
            .unwrap()
            .unwrap()
            .state,
        RunActionState::Denied
    );
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .base_chat
            .pending_approvals()
            .pending_count(),
        0
    );
    assert_durable_terminal(&fixture.read(), &turn, "succeeded");
}

#[tokio::test]
async fn h84_full_access_hook_ask_is_diagnostic_only_and_preserves_normal_read() {
    let fixture = Fixture::new_scripted(vec![
        function_call("h84-full", "read_file", json!({"path":"normal.txt"})),
        completed_text("H84_NORMAL"),
    ])
    .await;
    std::fs::write(
        fixture.workspace.join("normal.txt"),
        "h84 synthetic normal content",
    )
    .unwrap();
    fixture
        .settings
        .set_approval_policy(ApprovalPolicy::FullAccess)
        .unwrap();
    ask_hook(&fixture, "read_file");
    let turn = fixture.run_turn().await;
    let events = fixture.events.lock().unwrap();
    assert!(!events.iter().any(|e| e["type"] == "approval.requested"));
    assert!(events.iter().any(|e| e["type"] == "item.completed"
        && e["item"]["callId"] == "h84-full"
        && e["item"]["ok"] == true
        && e["item"]["output"]["content"]
            .as_str()
            .unwrap()
            .contains("h84 synthetic normal content")));
    drop(events);
    assert_durable_terminal(&fixture.read(), &turn, "succeeded");
}
