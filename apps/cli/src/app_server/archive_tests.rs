use super::*;

fn rpc(id: u64, method: &str, params: serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id: id.into(),
        method: method.into(),
        params,
    }
}
fn state() -> ServerState {
    let mut state = ServerState::new_for_test();
    assert!(state
        .handle_request_for_test(rpc(
            1,
            "initialize",
            serde_json::json!({
                "clientName":"synthetic-archive", "clientVersion":"1", "protocolVersion":1
            })
        ))
        .is_success());
    state
}
fn start(state: &mut ServerState) -> String {
    state
        .handle_request_for_test(rpc(2, "thread/start", serde_json::json!({})))
        .result()["threadId"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn has_thread(response: &RpcResponse, id: &str) -> bool {
    response.result()["threads"]
        .as_array()
        .unwrap()
        .iter()
        .any(|thread| thread["threadId"] == id)
}

#[test]
fn h215_archive_lists_are_disjoint_and_only_explicit_restore_allows_resume() {
    let mut state = state();
    let id = start(&mut state);
    assert!(state
        .handle_request_for_test(rpc(3, "thread/archive", serde_json::json!({"threadId":id})))
        .is_success());
    assert!(!has_thread(
        &state.handle_request_for_test(rpc(4, "thread/list", serde_json::json!({}))),
        &id
    ));
    let archived =
        state.handle_request_for_test(rpc(5, "thread/list", serde_json::json!({"archived":true})));
    assert!(has_thread(&archived, &id));
    assert_eq!(archived.result()["threads"][0]["archived"], true);
    assert!(!state
        .handle_request_for_test(rpc(6, "thread/resume", serde_json::json!({"threadId":id})))
        .is_success());
    assert!(state
        .handle_request_for_test(rpc(
            7,
            "thread/unarchive",
            serde_json::json!({"threadId":id})
        ))
        .is_success());
    assert!(has_thread(
        &state.handle_request_for_test(rpc(8, "thread/list", serde_json::json!({}))),
        &id
    ));
    assert!(!has_thread(
        &state.handle_request_for_test(rpc(9, "thread/list", serde_json::json!({"archived":true}))),
        &id
    ));
    assert!(state
        .handle_request_for_test(rpc(10, "thread/resume", serde_json::json!({"threadId":id})))
        .is_success());
    let (response, launch) = state.dispatch(
        rpc(
            11,
            "turn/start",
            serde_json::json!({"threadId":id,"input":"/help"}),
        ),
        None,
    );
    assert!(response.is_success());
    assert!(launch.is_some());
}

#[test]
fn h215_archive_refuses_busy_and_cancelling_without_signalling_or_publishing_marker() {
    let mut state = state();
    let id = start(&mut state);
    let (response, launch) = state.dispatch(
        rpc(
            3,
            "turn/start",
            serde_json::json!({"threadId":id,"input":"/help"}),
        ),
        None,
    );
    assert!(response.is_success());
    let launch = launch.unwrap();
    for number in 4..6 {
        if number == 5 {
            assert!(
                launch
                    .chat
                    .request_cancel(&launch.turn_id)
                    .unwrap()
                    .accepted
            );
        }
        let archived = state.handle_request_for_test(rpc(
            number,
            "thread/archive",
            serde_json::json!({"threadId":id}),
        ));
        assert!(!archived.is_success());
        assert!(!ArchiveService::new(state.database.clone())
            .is_archived(&id)
            .unwrap());
        assert!(state.threads.contains_key(&id));
        assert!(state.turns[&launch.turn_id].active);
        assert!(RunStore::new(&state.database)
            .get(&launch.turn_id)
            .unwrap()
            .unwrap()
            .finished_at
            .is_none());
    }
}

#[test]
fn h215_archive_write_failure_keeps_loaded_thread_and_reports_failure() {
    let mut state = state();
    let id = start(&mut state);
    state.database.with_conn(|conn|conn.execute_batch("CREATE TRIGGER h215_archive_fail BEFORE INSERT ON documents WHEN NEW.collection='archived_conversations' BEGIN SELECT RAISE(ABORT,'synthetic archive write failure'); END;").map_err(|error|CoreError::Persistence(error.to_string()))).unwrap();
    assert!(!state
        .handle_request_for_test(rpc(3, "thread/archive", serde_json::json!({"threadId":id})))
        .is_success());
    assert!(state.threads.contains_key(&id));
    assert!(!ArchiveService::new(state.database.clone())
        .is_archived(&id)
        .unwrap());
    assert!(has_thread(
        &state.handle_request_for_test(rpc(4, "thread/list", serde_json::json!({}))),
        &id
    ));
}

#[test]
fn h215_archived_source_cannot_fork_or_resume_cold_and_no_child_is_created() {
    let mut state = state();
    let id = start(&mut state);
    let database = state.database.clone();
    let workspace = state.workspace.clone();
    assert!(state
        .handle_request_for_test(rpc(3, "thread/archive", serde_json::json!({"threadId":id})))
        .is_success());
    let count = EventStore::new(&database).list_sessions().unwrap().len();
    drop(state);
    let mut cold = ServerState::new_for_test_with_database(database.clone(), workspace);
    assert!(cold
        .handle_request_for_test(rpc(
            1,
            "initialize",
            serde_json::json!({"clientName":"cold","clientVersion":"1","protocolVersion":1})
        ))
        .is_success());
    for method in ["thread/resume", "thread/fork"] {
        assert!(!cold
            .handle_request_for_test(rpc(4, method, serde_json::json!({"threadId":id})))
            .is_success());
    }
    assert_eq!(
        EventStore::new(&database).list_sessions().unwrap().len(),
        count
    );
    assert!(cold
        .handle_request_for_test(rpc(
            5,
            "thread/unarchive",
            serde_json::json!({"threadId":id})
        ))
        .is_success());
    assert!(cold
        .handle_request_for_test(rpc(6, "thread/resume", serde_json::json!({"threadId":id})))
        .is_success());
}

#[test]
fn h215_external_durable_archive_blocks_stale_loaded_alias_without_new_run() {
    let mut state = state();
    let id = start(&mut state);
    let alias = format!(
        "ses_{}",
        id.strip_prefix("ses_").unwrap().to_ascii_uppercase()
    );
    assert!(state
        .handle_request_for_test(rpc(
            3,
            "thread/resume",
            serde_json::json!({"threadId":alias})
        ))
        .is_success());
    ArchiveService::new(state.database.clone())
        .archive_session(&id)
        .unwrap();
    for identity in [&id, &alias] {
        let (response, launch) = state.dispatch(
            rpc(
                4,
                "turn/start",
                serde_json::json!({"threadId":identity,"input":"/help"}),
            ),
            None,
        );
        assert!(!response.is_success());
        assert!(launch.is_none());
    }
    assert!(state.turns.is_empty());
    assert!(RunStore::new(&state.database)
        .list_for_session(&id)
        .unwrap()
        .is_empty());
}

#[test]
fn h215_legacy_archive_marker_rejects_steer_before_replacement_or_cancel() {
    let mut state = state();
    let id = start(&mut state);
    let (response, launch) = state.dispatch(
        rpc(
            3,
            "turn/start",
            serde_json::json!({"threadId":id,"input":"/help"}),
        ),
        None,
    );
    assert!(response.is_success());
    let launch = launch.unwrap();
    // Simulate a marker written by a pre-upgrade host, not the new archive API.
    deepagent_persistence::document_store::DocumentStore::new(&state.database)
        .put(
            "archived_conversations",
            &id,
            "{}",
            None,
            deepagent_core::clock::Clock::now(&SystemClock),
        )
        .unwrap();
    let (response, replacement) = state.dispatch(
        rpc(
            4,
            "turn/steer",
            serde_json::json!({"threadId":id,"turnId":launch.turn_id,"input":"must not replace"}),
        ),
        None,
    );
    assert!(!response.is_success());
    assert!(replacement.is_none());
    assert_eq!(state.turns.len(), 1);
    assert_eq!(
        RunStore::new(&state.database)
            .list_for_session(&id)
            .unwrap()
            .len(),
        1
    );
}
