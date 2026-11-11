//! H-220: real dispatcher, repository and synthetic file-backed sessions.
use super::*;
use serde_json::{json, Value};

fn start(state: &mut ServerState, cwd: &std::path::Path, provider: Option<&str>) -> String {
    let response = state.handle_request_for_test(rpc(
        2,
        "thread/start",
        json!({"cwd":cwd,"provider":provider}),
    ));
    assert!(response.is_success(), "{response:?}");
    response.result()["threadId"].as_str().unwrap().to_owned()
}
fn rpc(id: u64, method: &str, params: Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id: json!(id),
        method: method.into(),
        params,
    }
}
fn list(state: &mut ServerState, params: Value) -> Value {
    let response = state.handle_request_for_test(rpc(9, "thread/list", params));
    assert!(response.is_success(), "{response:?}");
    response.result().clone()
}
fn ids(page: &Value) -> Vec<String> {
    page["threads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["threadId"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn h220_default_explicit_boundaries_and_all_pages_are_stable_and_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(temp.path().join("db.sqlite")).unwrap());
    let mut state = ServerState::new_for_test_with_database(db.clone(), temp.path().to_owned());
    state.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"h220","clientVersion":"test","protocolVersion":1}),
    ));
    let empty = list(&mut state, json!({}));
    assert_eq!(empty["nextCursor"], Value::Null);
    assert_eq!(empty["hasMore"], false);
    for _ in 0..57 {
        start(&mut state, temp.path(), None);
    }
    // Exact ties must use a stable, explicit identity tiebreaker.
    db.with_conn(|c| {
        c.execute("UPDATE sessions SET updated_at=1234", [])
            .map(|_| ())
            .map_err(|e| CoreError::Persistence(e.to_string()))
    })
    .unwrap();
    let expected: Vec<_> = {
        let mut ids: Vec<_> = EventStore::new(&db)
            .list_sessions()
            .unwrap()
            .iter()
            .map(|s| s.id.to_string())
            .collect();
        ids.sort_by(|a, b| b.cmp(a));
        ids
    };
    let first = list(&mut state, json!({}));
    assert_eq!(ids(&first).len(), 25);
    assert_eq!(first["listPageVersion"], 1);
    assert_eq!(first["hasMore"], true);
    let second = list(&mut state, json!({"cursor":first["nextCursor"]}));
    assert_eq!(
        ids(&second),
        ids(&list(&mut state, json!({"cursor":first["nextCursor"]}))),
        "read-only retry"
    );
    drop(state);
    drop(db);
    let db = Arc::new(Database::open(temp.path().join("db.sqlite")).unwrap());
    let mut state = ServerState::new_for_test_with_database(db, temp.path().to_owned());
    state.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"h220-reopen","clientVersion":"test","protocolVersion":1}),
    ));
    let third = list(&mut state, json!({"cursor":second["nextCursor"]}));
    assert_eq!(third["hasMore"], false);
    assert_eq!(third["nextCursor"], Value::Null);
    let all: Vec<_> = ids(&first)
        .into_iter()
        .chain(ids(&second))
        .chain(ids(&third))
        .collect();
    assert_eq!(all, expected);
    assert_eq!(ids(&list(&mut state, json!({"limit":57}))).len(), 57);
    assert_eq!(list(&mut state, json!({"limit":57}))["hasMore"], false);
}

#[test]
fn h220_filters_apply_before_paging_and_provider_is_explicit_not_inferred() {
    let temp = tempfile::tempdir().unwrap();
    let other = temp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut state = ServerState::new_for_test_with_database(db.clone(), temp.path().to_owned());
    state.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"h220","clientVersion":"test","protocolVersion":1}),
    ));
    let kept = start(&mut state, temp.path(), Some("deepseek"));
    let archived = start(&mut state, temp.path(), Some("deepseek"));
    start(&mut state, temp.path(), Some("openai"));
    start(&mut state, &other, Some("deepseek"));
    start(&mut state, temp.path(), None);
    EventStore::new(&db)
        .rename_session(
            kept.parse().unwrap(),
            Some("目标_%标题"),
            deepagent_core::clock::Timestamp::from_millis(99999),
        )
        .unwrap();
    ArchiveService::new(db.clone())
        .archive_session(&archived)
        .unwrap();
    let workspace = temp.path().canonicalize().unwrap();
    let page = list(
        &mut state,
        json!({"cwd":workspace,"provider":"deepseek","search":"_%","updatedAfter":50000,"limit":1,"sortKey":"updated_at","sortDirection":"asc"}),
    );
    assert_eq!(ids(&page), vec![kept]);
    let page = list(
        &mut state,
        json!({"archived":true,"provider":"deepseek","limit":1}),
    );
    assert_eq!(ids(&page), vec![archived]);
    let page = list(&mut state, json!({"cwd":workspace,"provider":"absent"}));
    assert!(ids(&page).is_empty());
}

#[test]
fn h220_invalid_and_foreign_cursors_and_changed_queries_are_explicit_errors() {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut state = ServerState::new_for_test_with_database(db.clone(), temp.path().to_owned());
    state.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"h220","clientVersion":"test","protocolVersion":1}),
    ));
    for _ in 0..3 {
        start(&mut state, temp.path(), None);
    }
    let first = list(&mut state, json!({"limit":1}));
    let mut tampered = first["nextCursor"].as_str().unwrap().as_bytes().to_vec();
    tampered[10] = if tampered[10] == b'A' { b'B' } else { b'A' };
    let history =
        history_cursor::encode(&db, "a-real-history-domain", &json!({"position":1})).unwrap();
    for bad in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"limit":"2"}),
        json!({"cursor":"bad"}),
        json!({"cursor":String::from_utf8(tampered).unwrap()}),
        json!({"cursor":history}),
        json!({"maxBytes":1}),
        json!({"sortKey":"title"}),
        json!({"provider":""}),
        json!({"cursor":first["nextCursor"],"archived":true}),
        json!({"cursor":first["nextCursor"],"sortDirection":"asc"}),
    ] {
        let response = state.handle_request_for_test(rpc(8, "thread/list", bad.clone()));
        assert_eq!(
            response.error.as_ref().map(|e| e.code),
            Some(ERR_INVALID_PARAMS),
            "{bad}: {response:?}"
        );
    }
    let foreign = Arc::new(Database::open_in_memory().unwrap());
    let mut other = ServerState::new_for_test_with_database(foreign, temp.path().to_owned());
    other.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"other","clientVersion":"test","protocolVersion":1}),
    ));
    assert!(!other
        .handle_request_for_test(rpc(8, "thread/list", json!({"cursor":first["nextCursor"]})))
        .is_success());
    let second = list(&mut state, json!({"cursor":first["nextCursor"],"limit":1}));
    assert_ne!(first["nextCursor"], second["nextCursor"]);
}

#[test]
fn h220_insert_rename_archive_and_restore_invalidate_old_cursors() {
    for mutation in ["insert", "rename", "archive", "restore"] {
        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let mut state = ServerState::new_for_test_with_database(db.clone(), temp.path().to_owned());
        state.handle_request_for_test(rpc(
            1,
            "initialize",
            json!({"clientName":"h220","clientVersion":"test","protocolVersion":1}),
        ));
        let id = start(&mut state, temp.path(), None);
        start(&mut state, temp.path(), None);
        if mutation == "restore" {
            ArchiveService::new(db.clone())
                .archive_session(&id)
                .unwrap();
        }
        let first = list(
            &mut state,
            json!({"limit":1,"archived":mutation=="restore"}),
        );
        let token = if first["hasMore"] == true {
            first["nextCursor"].clone()
        } else {
            start(&mut state, temp.path(), None);
            list(&mut state, json!({"limit":1}))["nextCursor"].clone()
        };
        match mutation {
            "insert" => {
                start(&mut state, temp.path(), None);
            }
            "rename" => {
                EventStore::new(&db)
                    .rename_session(
                        id.parse().unwrap(),
                        Some("changed"),
                        deepagent_core::clock::Timestamp::from_millis(777),
                    )
                    .unwrap();
            }
            "archive" => {
                ArchiveService::new(db.clone())
                    .archive_session(&id)
                    .unwrap();
            }
            "restore" => {
                ArchiveService::new(db.clone())
                    .unarchive_session(&id)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let response =
            state.handle_request_for_test(rpc(8, "thread/list", json!({"cursor":token})));
        assert_eq!(
            response.error.as_ref().map(|e| e.code),
            Some(ERR_INVALID_PARAMS),
            "{mutation}: {response:?}"
        );
        assert!(
            response.error.unwrap().message.contains("stale"),
            "{mutation}"
        );
    }
}

#[test]
fn h220_sparse_provider_filter_returns_advancing_empty_pages_not_unbounded_scans() {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut state = ServerState::new_for_test_with_database(db, temp.path().to_owned());
    state.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"h220","clientVersion":"test","protocolVersion":1}),
    ));
    let target = start(&mut state, temp.path(), Some("deepseek"));
    for _ in 0..105 {
        start(&mut state, temp.path(), None);
    }
    let first = list(&mut state, json!({"provider":"deepseek"}));
    assert!(ids(&first).is_empty());
    assert_eq!(first["hasMore"], true);
    let second = list(&mut state, json!({"cursor":first["nextCursor"]}));
    assert_eq!(ids(&second), vec![target]);
    assert_eq!(second["hasMore"], false);
}

#[test]
fn h220_byte_limit_bounds_complete_envelope_and_never_drops_an_oversized_row() {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut state = ServerState::new_for_test_with_database(db.clone(), temp.path().to_owned());
    state.handle_request_for_test(rpc(
        1,
        "initialize",
        json!({"clientName":"h220","clientVersion":"test","protocolVersion":1}),
    ));
    for _ in 0..6 {
        let id = start(&mut state, temp.path(), None);
        EventStore::new(&db)
            .rename_session(
                id.parse().unwrap(),
                Some(&"\"\\中文".repeat(50)),
                deepagent_core::clock::Timestamp::from_millis(99),
            )
            .unwrap();
    }
    let mut params = json!({"maxBytes":4096});
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let response = state.handle_request_for_test(rpc(9, "thread/list", params));
        assert!(response.is_success(), "{response:?}");
        assert!(serde_json::to_vec(&response).unwrap().len() <= 4096);
        for id in ids(response.result()) {
            assert!(seen.insert(id));
        }
        if response.result()["hasMore"] == false {
            break;
        }
        params = json!({"cursor":response.result()["nextCursor"],"maxBytes":4096});
        assert!(seen.len() <= 6);
    }
    assert_eq!(seen.len(), 6);
    db.with_conn(|c| {
        c.execute("UPDATE sessions SET title=?1", ["x".repeat(100_000)])
            .map(|_| ())
            .map_err(|e| CoreError::Persistence(e.to_string()))
    })
    .unwrap();
    let response = state.handle_request_for_test(rpc(9, "thread/list", json!({})));
    assert!(
        !response.is_success(),
        "huge title must not be silently truncated/skipped"
    );
}
