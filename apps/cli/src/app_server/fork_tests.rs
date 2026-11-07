use super::*;
use serde_json::json;

fn rpc(method: &str, params: serde_json::Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id: 1.into(),
        method: method.into(),
        params,
    }
}
fn initialize(state: &mut ServerState) {
    assert!(state
        .handle_request_for_test(rpc(
            "initialize",
            json!({
                "clientName":"synthetic-fork", "clientVersion":"1", "protocolVersion":1
            })
        ))
        .is_success());
}
fn assert_materialized(cold: bool) {
    for explicit in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("host");
        let project = root.path().join("project");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&project).unwrap();
        let path = root.path().join("synthetic.db");
        let database = Arc::new(Database::open(&path).unwrap());
        let mut state = ServerState::new_for_test_with_database(database, workspace.clone());
        initialize(&mut state);
        let params = if explicit {
            json!({"cwd":project,"provider":"deepseek","model":"deepseek-flash",
                "reasoningEffort":"none","permissionProfile":"read_only","sandboxBackend":"direct"})
        } else {
            json!({})
        };
        let started = state.handle_request_for_test(rpc("thread/start", params));
        assert!(started.is_success());
        let source = started.result()["threadId"].as_str().unwrap().to_owned();
        let expected = state.threads[&source].clone();
        let configuration = EventStore::new(&state.database)
            .session_execution_config(source.parse().unwrap())
            .unwrap();
        if cold {
            drop(state);
            state = ServerState::new_for_test_with_database(
                Arc::new(Database::open(&path).unwrap()),
                workspace,
            );
            initialize(&mut state);
            assert!(!state.threads.contains_key(&source));
        } else {
            // A stale loaded view is not the durable fork's configuration.
            let stale = state.threads.get_mut(&source).unwrap();
            stale.overrides.model = Some("synthetic-stale-model".into());
            stale.overrides.permission_profile = Some("full_access".into());
        }
        let forked = state.handle_request_for_test(rpc(
            "thread/fork",
            json!({"threadId":source,"atSequence":0}),
        ));
        assert!(forked.is_success(), "{forked:?}");
        let child = forked.result()["threadId"].as_str().unwrap().to_owned();
        let loaded = state
            .threads
            .get(&child)
            .expect("fork response must address a ready child");
        assert_eq!(loaded.workspace, expected.workspace);
        assert_eq!(loaded.overrides, expected.overrides);
        assert_eq!(
            EventStore::new(&state.database)
                .session_execution_config(child.parse().unwrap())
                .unwrap(),
            configuration
        );
        let (turn, launch) = state.dispatch(
            rpc("turn/start", json!({"threadId":child,"input":"/help"})),
            None,
        );
        assert!(turn.is_success(), "{turn:?}");
        assert_eq!(launch.as_ref().unwrap().overrides, expected.overrides);
        assert!(RunStore::new(&state.database)
            .list_for_session(&source)
            .unwrap()
            .is_empty());
        assert_eq!(
            RunStore::new(&state.database)
                .list_for_session(&child)
                .unwrap()
                .len(),
            1
        );
        let resumed =
            state.handle_request_for_test(rpc("thread/resume", json!({"threadId":child})));
        assert!(resumed.is_success());
        assert_eq!(
            resumed.result()["configuration"],
            serde_json::to_value(configuration).unwrap()
        );
        let (busy, extra) = state.dispatch(
            rpc("turn/start", json!({"threadId":child,"input":"/help"})),
            None,
        );
        assert_eq!(busy.error_code(), Some(ERR_THREAD_BUSY));
        assert!(extra.is_none(), "resume cannot replace an admitted owner");
        assert_eq!(
            RunStore::new(&state.database)
                .list_for_session(&child)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            EventStore::new(&state.database)
                .list_sessions()
                .unwrap()
                .len(),
            2
        );
    }
}
#[test]
fn h216_cold_fork_installs_durable_workspace_and_all_explicit_overrides_before_success() {
    assert_materialized(true);
}
#[test]
fn h216_hot_fork_uses_the_same_durable_materialization_not_stale_loaded_state() {
    assert_materialized(false);
}
#[test]
fn h216_legacy_cold_fork_uses_the_resume_default_without_inventing_configuration() {
    let mut state = ServerState::new_for_test();
    initialize(&mut state);
    let source = Session::create(&state.database, &SystemClock, Some("synthetic legacy"))
        .unwrap()
        .id();
    let forked = state.handle_request_for_test(rpc("thread/fork", json!({"threadId":source})));
    assert!(forked.is_success());
    let child = forked.result()["threadId"].as_str().unwrap();
    let loaded = state.threads.get(child).expect("legacy fork is ready too");
    assert_eq!(loaded.overrides, HarnessRunOverrides::default());
    assert_eq!(loaded.workspace, state.workspace);
    assert_eq!(
        EventStore::new(&state.database)
            .session_execution_config(child.parse().unwrap())
            .unwrap(),
        None
    );
}
#[test]
fn h216_failed_fork_publication_does_not_install_or_duplicate_a_child() {
    let mut state = ServerState::new_for_test();
    initialize(&mut state);
    let started = state.handle_request_for_test(rpc("thread/start", json!({})));
    let source = started.result()["threadId"].as_str().unwrap().to_owned();
    let before = EventStore::new(&state.database)
        .load_session(source.parse().unwrap())
        .unwrap();
    state.database.with_conn(|conn| conn.execute_batch(
        "CREATE TRIGGER h216_reject_fork BEFORE INSERT ON sessions BEGIN SELECT RAISE(ABORT,'synthetic fork failure'); END;"
    ).map_err(|error| CoreError::Persistence(error.to_string()))).unwrap();
    for _ in 0..2 {
        let forked = state.handle_request_for_test(rpc("thread/fork", json!({"threadId":source})));
        assert!(!forked.is_success());
        assert_eq!(state.threads.len(), 1);
        assert_eq!(
            EventStore::new(&state.database)
                .list_sessions()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            serde_json::to_value(
                EventStore::new(&state.database)
                    .load_session(source.parse().unwrap())
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&before).unwrap()
        );
    }
}
