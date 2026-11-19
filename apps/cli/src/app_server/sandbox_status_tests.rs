use super::*;
use serde_json::json;

#[test]
fn sandbox_status_reports_actual_host_and_coordinator_without_admission() {
    let mut state = ServerState::new_for_test();
    let request = |id, method: &str, params| RpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id: json!(id),
        method: method.into(),
        params,
    };
    assert_eq!(
        state
            .dispatch(request(1, "sandbox/status", json!({})), None)
            .0
            .error_code(),
        Some(ERR_NOT_INITIALIZED)
    );
    assert!(state.dispatch(request(2, "initialize", json!({"clientName":"sandbox-test","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})), None).0.is_success());
    let expected = serde_json::to_value(&state.capabilities).unwrap();
    let (response, launch) = state.dispatch(request(3, "sandbox/status", json!({})), None);
    assert!(response.is_success());
    for (key, value) in expected.as_object().unwrap() {
        assert_eq!(&response.result()[key], value);
    }
    assert!(response.result()["coordinator"]["ready"].is_boolean());
    assert_eq!(response.result()["coordinator"]["activeRuns"], 0);
    assert!(launch.is_none());
    assert!(state.turns.is_empty());
    assert!(state.threads.is_empty());
}
