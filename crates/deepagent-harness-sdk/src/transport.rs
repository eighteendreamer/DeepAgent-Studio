//! Wire formatting for a line-delimited JSON-RPC 2.0 channel.
//!
//! The app-server reads one JSON-RPC request per line on stdin and writes one
//! response per line on stdout; streamed notifications for harness events are
//! interleaved on the same stdout channel. Both envelopes are JSON-RPC 2.0, so a
//! single `untagged` decode distinguishes them by their shapes (`id` ⇒ response,
//! `method` ⇒ notification).

use deepagent_harness_protocol::{RpcNotification, RpcRequest, RpcResponse};
use serde::Deserialize;

/// One envelope read from the server's stdout.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum WireMessage {
    /// A JSON-RPC response matched to an in-flight request by `id`.
    Response(RpcResponse),
    /// A JSON-RPC notification (`harness/event`) carrying a harness event.
    Notification(RpcNotification),
}

/// Build a JSON-RPC request line for `method`/`params` under `id`.
pub fn request_line(
    id: i64,
    method: &str,
    params: &serde_json::Value,
) -> Result<String, serde_json::Error> {
    let request = RpcRequest {
        jsonrpc: "2.0".into(),
        id: serde_json::Value::from(id),
        method: method.to_string(),
        params: params.clone(),
    };
    serde_json::to_string(&request)
}

/// Decode one stdout line into a response or notification envelope.
pub fn decode_line(line: &str) -> Result<WireMessage, serde_json::Error> {
    serde_json::from_str(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_harness_protocol::{HarnessEvent, RpcError};

    #[test]
    fn decodes_response_envelope() {
        let line = r#"{"jsonrpc":"2.0","id":3,"result":{"threadId":"t-1","status":"ready"}}"#;
        match decode_line(line).unwrap() {
            WireMessage::Response(response) => {
                assert_eq!(response.id, serde_json::Value::from(3));
                assert_eq!(
                    response.result().get("threadId").and_then(|v| v.as_str()),
                    Some("t-1")
                );
            }
            WireMessage::Notification(_) => panic!("expected response"),
        }
    }

    #[test]
    fn decodes_error_response_envelope() {
        let line = r#"{"jsonrpc":"2.0","id":9,"error":{"code":-32004,"message":"turn not found"}}"#;
        match decode_line(line).unwrap() {
            WireMessage::Response(response) => {
                assert_eq!(
                    response.error,
                    Some(RpcError {
                        code: -32004,
                        message: "turn not found".into(),
                        data: None,
                    })
                );
                assert!(response.error_code() == Some(-32004));
            }
            WireMessage::Notification(_) => panic!("expected response"),
        }
    }

    #[test]
    fn decodes_notification_envelope() {
        let event = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "harness/event",
            "eventSequence": 7,
            "params": {
                "type": "thread.started",
                "threadId": "t-1",
                "protocolVersion": 1
            }
        });
        match decode_line(&event.to_string()).unwrap() {
            WireMessage::Notification(notification) => {
                assert_eq!(notification.event_sequence, Some(7));
                let harness: HarnessEvent = serde_json::from_value(notification.params).unwrap();
                assert!(matches!(
                    harness,
                    HarnessEvent::ThreadStarted { thread_id, .. } if thread_id == "t-1"
                ));
            }
            WireMessage::Response(_) => panic!("expected notification"),
        }
    }

    #[test]
    fn request_line_has_jsonrpc_id_and_method() {
        let line = request_line(5, "thread/start", &serde_json::json!({"cwd": "G:/w"})).unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], 5);
        assert_eq!(value["method"], "thread/start");
        assert_eq!(value["params"]["cwd"], "G:/w");
    }
}
