use deepagent_harness_protocol::{InitializeResult, PROTOCOL_VERSION};
use serde_json::Value;

use crate::SdkError;

/// This gate runs only at the existing handshake boundary, before Ready.
/// Never attach arbitrary server JSON or serde diagnostics to a public error.
pub(crate) fn validate(value: &Value) -> Result<InitializeResult, SdkError> {
    let actual = value.get("protocolVersion").and_then(Value::as_u64).ok_or(
        SdkError::InvalidInitializeResult {
            field: "protocolVersion",
        },
    )?;
    if actual != u64::from(PROTOCOL_VERSION) {
        return Err(SdkError::IncompatibleProtocolVersion {
            expected: PROTOCOL_VERSION,
            actual,
        });
    }
    let result: InitializeResult = serde_json::from_value(value.clone())
        .map_err(|_| SdkError::InvalidInitializeResult { field: "schema" })?;
    for (field, identity) in [
        ("serverName", &result.server_name),
        ("serverVersion", &result.server_version),
    ] {
        if identity.trim().is_empty() {
            return Err(SdkError::InvalidInitializeResult { field });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numeric_mismatch_is_structured_but_invalid_types_are_not_echoed() {
        for actual in [0, 2, 999, u64::MAX] {
            assert!(matches!(
                validate(&json!({"protocolVersion":actual})),
                Err(SdkError::IncompatibleProtocolVersion { expected: PROTOCOL_VERSION, actual: received }) if received == actual
            ));
        }
        for actual in [json!("PRIVATE_PAYLOAD"), json!(-1), json!(1.5), Value::Null] {
            let error = validate(&json!({"protocolVersion":actual})).unwrap_err();
            assert!(matches!(
                error,
                SdkError::InvalidInitializeResult {
                    field: "protocolVersion"
                }
            ));
            assert!(!error.to_string().contains("PRIVATE_PAYLOAD"));
        }
    }
}
