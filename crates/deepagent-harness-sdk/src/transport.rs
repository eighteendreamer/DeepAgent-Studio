//! Wire formatting for a line-delimited JSON-RPC 2.0 channel.
//!
//! The app-server reads one JSON-RPC request per line on stdin and writes one
//! response per line on stdout; streamed notifications for harness events are
//! interleaved on the same stdout channel. Both envelopes are JSON-RPC 2.0, so a
//! single `untagged` decode distinguishes them by their shapes (`id` ⇒ response,
//! `method` ⇒ notification).

use deepagent_harness_protocol::{
    HarnessEvent, RpcNotification, RpcRequest, RpcResponse, JSONRPC_VERSION,
};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ServerLineError {
    #[error("{0}")]
    Version(&'static str),
    #[error("invalid JSON-RPC stdout frame")]
    Decode(serde_json::Error),
}

#[cfg(test)]
impl ServerLineError {
    fn is_data(&self) -> bool {
        match self {
            Self::Version(_) => true,
            Self::Decode(error) => error.is_data(),
        }
    }
}

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
        jsonrpc: JSONRPC_VERSION.into(),
        id: serde_json::Value::from(id),
        method: method.to_string(),
        params: params.clone(),
    };
    serde_json::to_string(&request)
}

/// Decode one stdout line into a raw response or notification envelope.
///
/// This preserves wire fields, including unsupported version strings and unknown
/// notification methods. It is not the client's validated dispatch boundary.
pub fn decode_line(line: &str) -> Result<WireMessage, serde_json::Error> {
    serde_json::from_str(line)
}

/// The client's single inbound envelope gate, before any pending/event mutation.
pub(crate) fn decode_server_line(line: &str) -> Result<WireMessage, ServerLineError> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(ServerLineError::Decode)?;
    // H-212 rejects wrong/missing versions without consuming pending replies.
    // Validate this independently of the DTO's string field deserialization so
    // wrong-type versions retain that contract rather than becoming fatal JSON.
    if let Some(object) = value.as_object() {
        if (object.contains_key("id") || object.contains_key("method"))
            && object.get("jsonrpc").and_then(|value| value.as_str()) != Some(JSONRPC_VERSION)
        {
            return Err(ServerLineError::Version(if object.contains_key("id") {
                "unsupported JSON-RPC response version"
            } else {
                "unsupported JSON-RPC notification version"
            }));
        }
        // Presence, not Option truthiness: error:null is not an error object,
        // and result:null is still a success member. Validate before the DTO
        // decoder erases the distinction between absent and null fields.
        if object.contains_key("id")
            && (object.contains_key("result") == object.contains_key("error")
                || object.get("error").is_some_and(|error| !error.is_object()))
        {
            return Err(ServerLineError::Decode(
                <serde_json::Error as serde::de::Error>::custom(
                    "invalid JSON-RPC response members",
                ),
            ));
        }
        if object.get("method").and_then(|method| method.as_str()) == Some("harness/event")
            && !event_version_compatible(&value["params"])
        {
            return Err(ServerLineError::Decode(
                <serde_json::Error as serde::de::Error>::custom(
                    "unsupported harness event protocol version",
                ),
            ));
        }
    }
    // Keep the original DTO decoder (including duplicate-field rejection).
    // A Value round trip here would silently collapse repeated wire fields.
    let mut message = decode_line(line).map_err(ServerLineError::Decode)?;
    if let WireMessage::Response(response) = &mut message {
        if value.get("result").is_some_and(serde_json::Value::is_null) {
            response.result = Some(serde_json::Value::Null);
        }
    }
    Ok(message)
}

/// The single notification-to-event projection used by the client reader.
/// Envelope version is validated by `decode_server_line` before dispatch.
/// Errors are fixed diagnostic reasons: never include untrusted payload data.
pub(crate) fn notification_event(
    notification: RpcNotification,
) -> Result<crate::HarnessNotification, &'static str> {
    if notification.method != "harness/event" {
        return Err("unsupported notification method");
    }
    if !event_version_compatible(&notification.params) {
        return Err("unsupported harness event protocol version");
    }
    let event = serde_json::from_value::<HarnessEvent>(notification.params)
        .map_err(|_| "invalid harness event payload")?;
    Ok(crate::HarnessNotification {
        event,
        event_sequence: notification.event_sequence,
        sequence_status: crate::SequenceStatus::Unavailable,
    })
}

fn event_version_compatible(params: &serde_json::Value) -> bool {
    params.get("protocolVersion").is_none_or(|version| {
        version.as_u64() == Some(deepagent_harness_protocol::PROTOCOL_VERSION.into())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_harness_protocol::RpcError;

    #[test]
    fn h100_live_gate_checks_member_presence_without_narrowing_raw_decode() {
        for body in [
            serde_json::json!({}),
            serde_json::json!({"result":{},"error":null}),
            serde_json::json!({"result":null,"error":{"code":-32004,"message":"synthetic"}}),
            serde_json::json!({"error":null}),
        ] {
            let mut frame = body;
            frame["jsonrpc"] = "2.0".into();
            frame["id"] = 1.into();
            let line = frame.to_string();
            assert!(
                decode_line(&line).is_ok(),
                "raw DTO compatibility is unchanged"
            );
            assert!(matches!(
                decode_server_line(&line),
                Err(ServerLineError::Decode(_))
            ));
        }
        for result in [
            serde_json::Value::Null,
            serde_json::json!(false),
            serde_json::json!(0),
            serde_json::json!(""),
            serde_json::json!([]),
            serde_json::json!({}),
        ] {
            let line = serde_json::json!({"jsonrpc":"2.0","id":1,"result":result}).to_string();
            let WireMessage::Response(response) = decode_server_line(&line).unwrap() else {
                panic!("expected validated response");
            };
            assert!(response.is_success());
            assert_eq!(response.result, Some(result));
        }
        let WireMessage::Response(raw) =
            decode_line(r#"{"jsonrpc":"2.0","id":1,"result":null}"#).unwrap()
        else {
            panic!("expected raw response");
        };
        assert!(
            raw.result.is_none(),
            "raw Option DTO semantics remain unchanged"
        );
    }

    #[test]
    fn h101_gate_distinguishes_corrupt_frames_from_version_rejection_without_raw_decode_changes() {
        for line in [
            "not-json-PRIVATE_CANARY",
            "",
            "[]",
            "null",
            r#"{"jsonrpc":"2.0","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"error":"PRIVATE_CANARY"}"#,
        ] {
            assert!(
                matches!(decode_server_line(line), Err(ServerLineError::Decode(_))),
                "{line}"
            );
        }
        for line in [
            r#"{"id":1,"result":{}}"#,
            r#"{"jsonrpc":null,"id":1,"result":{}}"#,
            r#"{"jsonrpc":"1.0","method":"future/event","params":{}}"#,
        ] {
            assert!(matches!(
                decode_server_line(line),
                Err(ServerLineError::Version(_))
            ));
        }
        let duplicate = r#"{"jsonrpc":"2.0","id":1,"id":2,"result":{}}"#;
        assert!(decode_line(duplicate).is_err());
        assert!(matches!(
            decode_server_line(duplicate),
            Err(ServerLineError::Decode(_))
        ));
    }

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
                    harness.event,
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
            let WireMessage::Notification(notification) = decode_server_line(&value.to_string())
                .map_err(|error| match error {
                    ServerLineError::Version(_) => "unsupported notification version",
                    ServerLineError::Decode(_) => "unsupported harness event protocol version",
                })?
            else {
                panic!("expected notification");
            };
            notification_event(notification)
        };
        assert!(matches!(
            project(valid.clone()),
            Ok(crate::HarnessNotification {
                event: HarnessEvent::ThreadStarted { .. },
                ..
            })
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
        for version in [
            serde_json::json!(2),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            let mut value = valid.clone();
            value["params"]["protocolVersion"] = version;
            assert_eq!(
                project(value),
                Err("unsupported harness event protocol version")
            );
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

    #[test]
    fn h212_inbound_gate_rejects_invalid_response_versions_without_changing_raw_decode() {
        for body in [
            serde_json::json!({"result": {"source": "synthetic"}}),
            serde_json::json!({"error": {"code": -32004, "message": "synthetic"}}),
        ] {
            let mut valid = body;
            valid["jsonrpc"] = "2.0".into();
            valid["id"] = 17.into();
            assert!(matches!(
                decode_server_line(&valid.to_string()),
                Ok(WireMessage::Response(_))
            ));
            for version in [
                serde_json::json!("1.0"),
                serde_json::json!("2"),
                serde_json::json!("2.0 "),
                serde_json::json!(""),
                serde_json::json!("synthetic-private-version"),
                serde_json::json!(2),
                serde_json::Value::Null,
                serde_json::json!(false),
                serde_json::json!([]),
                serde_json::json!({}),
            ] {
                let mut invalid = valid.clone();
                invalid["jsonrpc"] = version.clone();
                let error = decode_server_line(&invalid.to_string()).unwrap_err();
                assert!(error.is_data());
                assert!(!error.to_string().contains("synthetic-private-version"));
                if version.is_string() {
                    let WireMessage::Response(raw) = decode_line(&invalid.to_string()).unwrap()
                    else {
                        panic!("raw response contract changed");
                    };
                    assert_eq!(raw.jsonrpc, version.as_str().unwrap());
                    assert_eq!(error.to_string(), "unsupported JSON-RPC response version");
                }
            }
            valid.as_object_mut().unwrap().remove("jsonrpc");
            assert!(decode_server_line(&valid.to_string()).is_err());
        }
    }

    #[test]
    fn h212_shared_gate_validates_notification_version_before_method_projection() {
        let mut value = serde_json::json!({
            "jsonrpc": "synthetic-private-version", "method": "other/event", "eventSequence": 91,
            "params": {"type": "turn.completed", "message": "synthetic"}
        });
        let error = decode_server_line(&value.to_string()).unwrap_err();
        assert!(error.is_data());
        assert_eq!(
            error.to_string(),
            "unsupported JSON-RPC notification version"
        );
        value["jsonrpc"] = "2.0".into();
        let WireMessage::Notification(notification) =
            decode_server_line(&value.to_string()).unwrap()
        else {
            panic!("expected notification")
        };
        assert_eq!(notification.event_sequence, Some(91));
        assert_eq!(
            notification_event(notification),
            Err("unsupported notification method")
        );
    }
}
