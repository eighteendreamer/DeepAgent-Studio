//! H-218 exercises the real dispatcher and authoritative file/SQLite stores.
//! No provider, tool execution, credential lookup or external network is used.

use super::*;
use deepagent_core::event::EventPayload;
use deepagent_core::message::Message;
use deepagent_persistence::artifact_store::{ArtifactKind, ArtifactRecord, ArtifactStore};
use deepagent_persistence::run_control::{NewRunAction, NewRunApproval, RunControlStore};
use deepagent_persistence::subagent_store::{SubagentRunRecord, SubagentRunStore};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

struct Fixture {
    _temp: tempfile::TempDir,
    path: PathBuf,
    workspace: PathBuf,
    state: ServerState,
    thread: String,
}

fn call(state: &mut ServerState, method: &str, params: Value) -> RpcResponse {
    let (response, launch) = state.dispatch(
        RpcRequest {
            jsonrpc: "2.0".into(),
            id: json!(1),
            method: method.into(),
            params,
        },
        None,
    );
    assert!(launch.is_none(), "history must not launch a turn");
    response
}

impl Fixture {
    fn new(messages: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let path = temp.path().join("history.sqlite3");
        let mut state = Self::open(&path, &workspace);
        let started = call(&mut state, "thread/start", json!({}));
        assert!(started.is_success(), "{started:?}");
        let thread = started.result()["threadId"].as_str().unwrap().to_owned();
        let mut fixture = Self {
            _temp: temp,
            path,
            workspace,
            state,
            thread,
        };
        for index in 0..messages {
            fixture.append(&format!("history-{index}-{}", "x".repeat(1024)));
        }
        fixture
    }

    fn open(path: &std::path::Path, workspace: &std::path::Path) -> ServerState {
        let mut state = ServerState::new_for_test_with_database(
            Arc::new(Database::open(path).unwrap()),
            workspace.to_owned(),
        );
        let initialized = call(
            &mut state,
            "initialize",
            json!({"clientName":"h218-local-test","clientVersion":"1","protocolVersion":PROTOCOL_VERSION}),
        );
        assert!(initialized.is_success());
        state
    }

    fn append(&mut self, content: &str) {
        Session::recover(
            &self.state.database,
            &SystemClock,
            self.thread.parse().unwrap(),
        )
        .unwrap()
        .append(EventPayload::MessageAppended {
            message: Message::user(content),
        })
        .unwrap();
    }

    fn read(&mut self, mut params: Value) -> RpcResponse {
        params["threadId"] = json!(self.thread);
        call(&mut self.state, "thread/read", params)
    }
}

fn items(page: &Value) -> usize {
    page["events"].as_array().unwrap().len()
        + page["runEvents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| {
                1 + ["events", "actions", "approvals", "artifacts"]
                    .iter()
                    .map(|field| run[field].as_array().unwrap().len())
                    .sum::<usize>()
                    + run["graph"]["nodes"].as_array().unwrap().len()
            })
            .sum::<usize>()
}

fn assert_budget(response: &RpcResponse, page_size: usize, max_bytes: usize) {
    assert!(response.is_success(), "{response:?}");
    let bytes = serde_json::to_vec(response).unwrap().len();
    assert!(bytes <= max_bytes, "response {bytes} exceeds {max_bytes}");
    assert!(
        items(response.result()) <= page_size,
        "unbounded page: {response:?}"
    );
    let more = response.result()["hasMore"]
        .as_bool()
        .expect("explicit page exhaustion");
    assert_eq!(response.result()["nextCursor"].is_string(), more);
}

#[test]
fn h218_session_pages_keep_projection_groups_and_survive_append_and_reopen() {
    let mut fixture = Fixture::new(35);
    let expected = EventStore::new(&fixture.state.database)
        .load_session(fixture.thread.parse().unwrap())
        .unwrap();
    let mut response = fixture.read(json!({"pageSize":9,"maxBytes":16384}));
    assert_budget(&response, 9, 16384);
    assert_eq!(response.result()["hasMore"], true);
    let mut actual = Vec::new();
    let mut seen_cursors = BTreeSet::new();
    for page_number in 0..100 {
        let page = response.result();
        let events = page["events"].as_array().unwrap();
        // Each new MessageAppended source owns the following native item.
        for (index, event) in events.iter().enumerate() {
            if event["payload"]["type"] == "message_appended" {
                assert!(
                    index + 1 < events.len(),
                    "source/native group split by page"
                );
                assert_eq!(
                    events[index + 1]["sequence"],
                    event["sequence"].as_u64().unwrap() + 1
                );
            }
        }
        actual.extend(events.iter().map(|event| event["id"].clone()));
        if page["hasMore"] == false {
            break;
        }
        let cursor = page["nextCursor"].as_str().unwrap().to_owned();
        assert!(
            seen_cursors.insert(cursor.clone()),
            "cursor made no progress"
        );
        if page_number == 0 {
            fixture.append("appended-after-the-frozen-page");
            fixture.state = Fixture::open(&fixture.path, &fixture.workspace);
        }
        response = fixture.read(json!({"cursor":cursor,"pageSize":9,"maxBytes":16384}));
        assert_budget(&response, 9, 16384);
    }
    assert_eq!(
        response.result()["hasMore"],
        false,
        "pagination failed to terminate"
    );
    assert_eq!(
        actual,
        expected
            .iter()
            .map(|event| json!(event.id))
            .collect::<Vec<_>>()
    );
}

#[test]
fn h218_invalid_budget_and_foreign_or_malformed_cursor_are_rejected() {
    let mut fixture = Fixture::new(6);
    for params in [
        json!({"pageSize":0}),
        json!({"pageSize":501}),
        json!({"maxBytes":4095}),
        json!({"maxBytes":1048577}),
        json!({"cursor":"x".repeat(4097)}),
    ] {
        let response = fixture.read(params.clone());
        assert_eq!(
            response.error_code(),
            Some(ERR_INVALID_PARAMS),
            "{params}: {response:?}"
        );
    }
    let first = fixture.read(json!({"pageSize":5}));
    let cursor = first.result()["nextCursor"].as_str().unwrap();
    let altered = format!("{cursor}x");
    let invalid = fixture.read(json!({"cursor":altered}));
    assert_eq!(invalid.error_code(), Some(ERR_INVALID_PARAMS));
    let other = call(&mut fixture.state, "thread/start", json!({}));
    let foreign = call(
        &mut fixture.state,
        "thread/read",
        json!({
            "threadId":other.result()["threadId"],"cursor":cursor
        }),
    );
    assert_eq!(foreign.error_code(), Some(ERR_INVALID_PARAMS));
}

#[test]
fn h218_oversized_session_group_is_rejected_without_payload_echo_or_mutation() {
    let mut fixture = Fixture::new(0);
    fixture.append(&format!(
        "H218_PRIVATE_SYNTHETIC_MARKER{}",
        "x".repeat(100_000)
    ));
    let expected = EventStore::new(&fixture.state.database)
        .load_session(fixture.thread.parse().unwrap())
        .unwrap();
    let response = fixture.read(json!({"maxBytes":16384}));
    assert!(
        !response.is_success(),
        "oversized group escaped the response budget"
    );
    let wire = serde_json::to_string(&response).unwrap();
    assert!(wire.len() < 4096);
    assert!(!wire.contains("H218_PRIVATE_SYNTHETIC_MARKER"));
    assert_eq!(
        EventStore::new(&fixture.state.database)
            .load_session(fixture.thread.parse().unwrap())
            .unwrap(),
        expected
    );
}

fn seed_run_views(fixture: &Fixture, run: &str, count: usize) {
    let db = &fixture.state.database;
    RunStore::new(db)
        .create(run, &fixture.thread, None, 1)
        .unwrap();
    seed_run_rows(fixture, run, 0..count);
}

fn seed_run_rows(fixture: &Fixture, run: &str, indices: std::ops::Range<usize>) {
    let db = &fixture.state.database;
    for index in indices {
        let call_id = format!("call-{index:04}");
        RunControlStore::new(db)
            .create_action(&NewRunAction {
                run_id: run,
                turn_id: run,
                call_id: &call_id,
                sequence: index as u64,
                tool_name: "synthetic-only",
                arguments_hash: "synthetic-hash",
                risk: "low",
                parent_action_id: None,
                now: 2 + index as i64,
            })
            .unwrap();
        RunControlStore::new(db)
            .request_approval(&NewRunApproval {
                approval_id: &format!("approval-{run}-{index:04}"),
                run_id: run,
                call_id: &call_id,
                scope: "once",
                risk: "low",
                reason: Some("synthetic-only"),
                policy_snapshot: None,
                expires_at: None,
                now: 2 + index as i64,
            })
            .unwrap();
        ArtifactStore::new(db)
            .put_new(&ArtifactRecord {
                id: format!("artifact-{run}-{index:04}"),
                kind: ArtifactKind::ToolResult,
                run_id: Some(run.into()),
                call_id: Some(call_id),
                workspace_id: None,
                path: fixture
                    .workspace
                    .join(format!("not-created-{index}"))
                    .to_string_lossy()
                    .into_owned(),
                media_type: Some("text/plain".into()),
                byte_size: 0,
                digest: None,
                created_at: 2 + index as i64,
            })
            .unwrap();
        SubagentRunStore::new(db)
            .create(&SubagentRunRecord {
                id: format!("child-{run}-{index:04}"),
                parent_run_id: run.into(),
                origin_parent_run_id: run.into(),
                state: "completed".into(),
                agent_type: "general".into(),
                transcript_path: None,
                worktree_path: None,
                summary: Some("synthetic-only".into()),
                created_at: 2 + index as i64,
                updated_at: 2 + index as i64,
                finished_at: Some(2 + index as i64),
                resume_count: 0,
            })
            .unwrap();
    }
}

#[test]
fn h218_budget_covers_all_runs_events_actions_approvals_artifacts_and_graph_nodes() {
    let mut fixture = Fixture::new(0);
    for run in ["run-a", "run-b", "run-c"] {
        seed_run_views(&fixture, run, 12);
    }
    let mut response = fixture.read(json!({"pageSize":9,"maxBytes":16384}));
    // Freeze once, not lazily when a later run/section is first reached.
    seed_run_views(&fixture, "late-run", 2);
    for run in ["run-a", "run-b", "run-c"] {
        seed_run_rows(&fixture, run, 12..15);
    }
    fixture.state = Fixture::open(&fixture.path, &fixture.workspace);
    let mut observed: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut cursors = BTreeSet::new();
    for _ in 0..200 {
        assert_budget(&response, 9, 16384);
        for run in response.result()["runEvents"].as_array().unwrap() {
            let id = run["runId"].as_str().unwrap();
            assert_ne!(id, "late-run", "late run entered the frozen hydration");
            for (field, key) in [
                ("events", "sequence"),
                ("actions", "callId"),
                ("approvals", "approvalId"),
                ("artifacts", "id"),
            ] {
                for item in run[field].as_array().unwrap() {
                    assert!(
                        observed
                            .entry((id.into(), field.into()))
                            .or_default()
                            .insert(item[key].to_string()),
                        "duplicate {id}/{field}/{item}"
                    );
                }
            }
            for node in run["graph"]["nodes"].as_array().unwrap() {
                assert!(observed
                    .entry((id.into(), "graph".into()))
                    .or_default()
                    .insert(node["runId"].to_string()));
            }
        }
        if response.result()["hasMore"] == false {
            break;
        }
        let cursor = response.result()["nextCursor"].as_str().unwrap().to_owned();
        assert!(cursors.insert(cursor.clone()), "pagination did not advance");
        response = fixture.read(json!({"cursor":cursor,"pageSize":9,"maxBytes":16384}));
    }
    assert_eq!(response.result()["hasMore"], false);
    for run in ["run-a", "run-b", "run-c"] {
        assert_eq!(
            observed[&(run.into(), "events".into())].len(),
            25, // admitted event plus the original 12 action/approval pairs
        );
        for field in ["actions", "approvals", "artifacts"] {
            assert_eq!(observed[&(run.into(), field.into())].len(), 12);
        }
        assert_eq!(observed[&(run.into(), "graph".into())].len(), 13);
    }
}

#[test]
fn h218_minimum_byte_budget_counts_envelope_and_group_checkpoints_without_loss() {
    let mut fixture = Fixture::new(0);
    for index in 0..20 {
        fixture.append(&format!("small-{index}"));
    }
    let expected = EventStore::new(&fixture.state.database)
        .load_session(fixture.thread.parse().unwrap())
        .unwrap();
    let mut response = fixture.read(json!({"pageSize":9,"maxBytes":4096}));
    let mut ids = Vec::new();
    for _ in 0..50 {
        assert_budget(&response, 9, 4096);
        ids.extend(
            response.result()["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|event| event["id"].clone()),
        );
        if response.result()["hasMore"] == false {
            break;
        }
        response = fixture
            .read(json!({"pageSize":9,"maxBytes":4096,"cursor":response.result()["nextCursor"]}));
    }
    assert_eq!(response.result()["hasMore"], false);
    assert_eq!(
        ids,
        expected
            .iter()
            .map(|event| json!(event.id))
            .collect::<Vec<_>>()
    );
}

#[test]
fn h218_run_pages_reject_session_rewind_and_changed_filters() {
    let mut fixture = Fixture::new(1);
    seed_run_views(&fixture, "run-a", 12);
    let first = fixture.read(json!({"pageSize":7}));
    assert_budget(&first, 7, 256 * 1024);
    assert!(!first.result()["runEvents"].as_array().unwrap().is_empty());
    let cursor = first.result()["nextCursor"].as_str().unwrap();
    let changed = fixture.read(json!({"cursor":cursor,"sessionAfterSequence":1}));
    assert_eq!(changed.error_code(), Some(ERR_INVALID_PARAMS));
    EventStore::new(&fixture.state.database)
        .truncate_after(fixture.thread.parse().unwrap(), 0)
        .unwrap();
    fixture.state = Fixture::open(&fixture.path, &fixture.workspace);
    let stale = fixture.read(json!({"cursor":cursor}));
    assert_eq!(stale.error_code(), Some(ERR_INVALID_PARAMS));
    assert!(stale.error_message().unwrap().contains("stale"));
}

#[test]
fn h218_oversized_session_metadata_does_not_echo_or_load_history() {
    let mut fixture = Fixture::new(0);
    EventStore::new(&fixture.state.database)
        .rename_session(
            fixture.thread.parse().unwrap(),
            Some(&format!("H218_METADATA_PRIVATE{}", "x".repeat(20000))),
            deepagent_core::clock::Timestamp::from_millis(2),
        )
        .unwrap();
    let response = fixture.read(json!({"maxBytes":4096}));
    assert!(!response.is_success());
    let wire = serde_json::to_string(&response).unwrap();
    assert!(wire.len() < 4096);
    assert!(!wire.contains("H218_METADATA_PRIVATE"));
}

#[test]
fn h218_oversized_run_event_is_rejected_without_payload_echo() {
    let mut fixture = Fixture::new(0);
    let runs = RunStore::new(&fixture.state.database);
    runs.create("large-run", &fixture.thread, None, 1).unwrap();
    runs.append_event(
        "large-run",
        2,
        "running",
        "completed",
        "test",
        &json!({"content":format!("H218_RUN_PRIVATE{}","x".repeat(100_000))}),
    )
    .unwrap();
    let response = fixture.read(json!({"maxBytes":16384}));
    assert!(!response.is_success());
    let wire = serde_json::to_string(&response).unwrap();
    assert!(wire.len() < 4096);
    assert!(!wire.contains("H218_RUN_PRIVATE"));
}
