//! H-217: method identity is independent of spelling conventions and params.
use super::*;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn request(id: Value, method: &str, params: Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        method: method.into(),
        params,
    }
}

fn initialize(state: &mut ServerState) {
    assert!(state
        .dispatch(
            request(
                json!("init"),
                "initialize",
                json!({"clientName":"h217","clientVersion":"1","protocolVersion":PROTOCOL_VERSION}),
            ),
            None,
        )
        .0
        .is_success());
}

#[test]
fn h217_method_lookup_and_param_decode_have_distinct_errors_and_no_side_effects() {
    let workspace = tempfile::tempdir().unwrap();
    let mut state = ServerState::new_for_test_with_database(
        Arc::new(Database::open_in_memory().unwrap()),
        workspace.path().to_owned(),
    );
    let unknown_methods = [
        "does-not-exist",
        "thread/does-not-exist",
        "future/v2/extension",
        "harness/event",
        "",
        "Thread/start",
        "thread/start ",
        " thread/start",
        "thread//start",
        "rpc.extension",
        "线程/未知",
    ];
    let unknown_before_init = state
        .dispatch(request(json!(0), unknown_methods[0], json!({})), None)
        .0;
    assert_eq!(unknown_before_init.error_code(), Some(ERR_NOT_INITIALIZED));
    // Even initialize's un-namespaced name must report bad params, not an
    // unknown method. A failed handshake cannot advance connection state.
    let invalid_init = state
        .dispatch(request(json!(1), "initialize", json!(true)), None)
        .0;
    assert_eq!(invalid_init.error_code(), Some(ERR_INVALID_PARAMS));
    assert!(!state.initialized);
    initialize(&mut state);

    for method in unknown_methods {
        for params in [
            json!({}),
            Value::Null,
            json!([]),
            json!(true),
            json!("synthetic"),
        ] {
            let id = json!(format!("unknown:{method}"));
            let (response, launch) = state.dispatch(request(id.clone(), method, params), None);
            assert_eq!(response.id, id);
            assert_eq!(
                response.error_code(),
                Some(ERR_METHOD_NOT_FOUND),
                "{method:?}"
            );
            assert!(launch.is_none());
            assert!(response.result.is_none());
        }
    }
    for method in [
        "initialize",
        "thread/start",
        "thread/resume",
        "thread/list",
        "thread/read",
        "thread/fork",
        "thread/archive",
        "thread/unarchive",
        "turn/start",
        "turn/interrupt",
        "turn/steer",
        "approval/respond",
        "event/ack",
        "tool/list",
        "config/read",
        "sandbox/status",
    ] {
        let (response, launch) = state.dispatch(request(json!(method), method, json!(true)), None);
        assert_eq!(response.error_code(), Some(ERR_INVALID_PARAMS), "{method}");
        assert!(launch.is_none());
    }
    let mut wrong_version = request(json!("version"), "future/unknown", json!({}));
    wrong_version.jsonrpc = "1.0".into();
    assert_eq!(
        state.dispatch(wrong_version, None).0.error_code(),
        Some(-32600)
    );
    assert!(state.threads.is_empty());
    assert!(state.turns.is_empty());
    assert_eq!(state.last_event_ack, 0);
    assert!(state.initialized);
    let (normal, launch) =
        state.dispatch(request(json!("control"), "thread/list", json!({})), None);
    assert!(normal.is_success());
    assert!(launch.is_none());
    assert_eq!(normal.result()["threads"], json!([]));
}

#[tokio::test]
async fn h217_real_connection_keeps_error_codes_and_accepts_the_next_valid_request() {
    let workspace = tempfile::tempdir().unwrap();
    let state = ServerState::new_for_test_with_database(
        Arc::new(Database::open_in_memory().unwrap()),
        workspace.path().to_owned(),
    );
    let (mut input, server_input) = tokio::io::duplex(8192);
    let (server_output, output) = tokio::io::duplex(8192);
    let host = tokio::spawn(run_stdio_with_io(
        state.base_chat,
        workspace.path().to_owned(),
        state.capabilities,
        server_input,
        server_output,
    ));
    let mut output = BufReader::new(output).lines();
    for (id, method, params, expected) in [
        (
            1,
            "initialize",
            json!({"clientName":"h217","clientVersion":"1","protocolVersion":PROTOCOL_VERSION}),
            None,
        ),
        (
            2,
            "future/unknown",
            json!({"synthetic":true}),
            Some(ERR_METHOD_NOT_FOUND),
        ),
        (
            3,
            "initialize",
            json!({"clientName":42}),
            Some(ERR_INVALID_PARAMS),
        ),
        (
            4,
            "thread/start",
            json!({"cwd":42}),
            Some(ERR_INVALID_PARAMS),
        ),
        (5, "thread/list", json!({}), None),
    ] {
        input
            .write_all(
                format!(
                    "{}\n",
                    serde_json::to_string(&request(json!(id), method, params)).unwrap()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        input.flush().await.unwrap();
        let line = tokio::time::timeout(Duration::from_secs(5), output.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let response: RpcResponse = serde_json::from_str(&line).unwrap();
        assert_eq!(response.jsonrpc, JSONRPC_VERSION);
        assert_eq!(response.id, json!(id));
        assert_eq!(response.error_code(), expected, "{method}");
        assert_eq!(response.is_success(), expected.is_none());
    }
    drop(input);
    tokio::time::timeout(Duration::from_secs(5), host)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(output.next_line().await.unwrap().is_none());
}
