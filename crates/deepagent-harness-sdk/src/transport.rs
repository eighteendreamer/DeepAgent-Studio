//! Wire formatting for a line-delimited JSON-RPC 2.0 channel.
//!
//! The app-server reads one JSON-RPC request per line on stdin and writes one
//! response per line on stdout; streamed notifications for harness events are
//! interleaved on the same stdout channel. Both envelopes are JSON-RPC 2.0, so a
//! single `untagged` decode distinguishes them by their shapes (`id` ⇒ response,
//! `method` ⇒ notification).

use deepagent_harness_protocol::{HarnessEvent, RpcNotification, RpcRequest, RpcResponse};
use serde::Deserialize;

/// One envelope read from the server's stdout.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum WireMessage {
    /// A JSON-RPC response matched to an in-flight request by `id`.
    Response(RpcResponse),
    /// A raw JSON-RPC notification; its method and payload are not yet validated.
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

/// Decode one stdout line into a raw response or notification envelope.
///
/// This preserves wire fields, including unknown notification methods. It does
/// not authorize the envelope as a Harness event; the client validates that at
/// the notification projection boundary before publishing to subscribers.
pub fn decode_line(line: &str) -> Result<WireMessage, serde_json::Error> {
    serde_json::from_str(line)
}

/// The single notification-to-event projection used by the client reader.
/// Errors are fixed diagnostic reasons: never include untrusted payload data.
pub(crate) fn notification_event(
    notification: RpcNotification,
) -> Result<HarnessEvent, &'static str> {
    if notification.jsonrpc != "2.0" {
        return Err("unsupported notification version");
    }
    if notification.method != "harness/event" {
        return Err("unsupported notification method");
    }
    serde_json::from_value(notification.params).map_err(|_| "invalid harness event payload")
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_harness_protocol::RpcError;

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
                let harness = notification_event(notification).unwrap();
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

    #[test]
    fn h213_notification_projection_validates_namespace_version_and_schema() {
        let valid = serde_json::json!({
            "jsonrpc": "2.0", "method": "harness/event", "eventSequence": 17,
            "params": {"type": "thread.started", "threadId": "synthetic", "protocolVersion": 1}
        });
        let project = |value: serde_json::Value| {
            let WireMessage::Notification(notification) = decode_line(&value.to_string()).unwrap()
            else {
                panic!("expected notification");
            };
            notification_event(notification)
        };
        assert!(matches!(
            project(valid.clone()),
            Ok(HarnessEvent::ThreadStarted { .. })
        ));
        for method in [
            "",
            "other/event",
            "harness/Event",
            "harness/event ",
            "prefix/harness/event",
        ] {
            let mut value = valid.clone();
            value["method"] = method.into();
            assert_eq!(project(value), Err("unsupported notification method"));
        }
        for version in ["", "1.0", "2.1"] {
            let mut value = valid.clone();
            value["jsonrpc"] = version.into();
            assert_eq!(project(value), Err("unsupported notification version"));
        }
        for params in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!("invalid"),
            serde_json::json!({"type": "future.event"}),
            serde_json::json!({"type": "thread.started", "threadId": 17, "protocolVersion": 1}),
        ] {
            let mut value = valid.clone();
            value["params"] = params;
            assert_eq!(project(value), Err("invalid harness event payload"));
        }
        for field in ["method", "jsonrpc"] {
            for replacement in [None, Some(serde_json::Value::Null), Some(17.into())] {
                let mut value = valid.clone();
                match replacement {
                    None => {
                        value.as_object_mut().unwrap().remove(field);
                    }
                    Some(replacement) => value[field] = replacement,
                }
                assert!(decode_line(&value.to_string()).is_err());
            }
        }
    }

    #[test]
    fn h213_raw_decode_keeps_method_and_sequence_without_authorizing_the_event() {
        let WireMessage::Notification(notification) = decode_line(
            r#"{"jsonrpc":"2.0","method":"other/event","eventSequence":91,"params":{"type":"turn.completed","message":"synthetic"}}"#,
        ).unwrap() else { panic!("expected raw notification") };
        assert_eq!(notification.method, "other/event");
        assert_eq!(notification.event_sequence, Some(91));
        assert_eq!(
            notification_event(notification),
            Err("unsupported notification method")
        );
    }
}
