//! Shared adapter frame validation. This module does not execute code and its
//! synthetic tests are not evidence of SDK execution or OS containment.

use deepagent_core::error::{CoreError, Result};
use deepagent_runtime::workflow::code::{
    CodeInvocation, CodeIsolation, CodeReceipt, CodeRequest, CodeResponse, CODE_VERSION,
    MAX_CODE_LOG_BYTES, MAX_CODE_RESPONSE_BYTES,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

pub(super) struct Capture {
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

#[derive(Deserialize)]
struct Frame {
    ok: bool,
    outputs: Option<Map<String, Value>>,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    stdout_truncated: bool,
    #[serde(default)]
    stderr_truncated: bool,
    kind: Option<String>,
}

pub(super) fn decode(
    request: &CodeRequest,
    version: String,
    isolation: CodeIsolation,
    exit: u32,
    stdout: Capture,
    stderr: Capture,
) -> Result<CodeResponse> {
    if stdout.truncated || stdout.bytes.len() > MAX_CODE_RESPONSE_BYTES {
        return Err(CoreError::invalid("CodeResultInvalid: oversized SDK frame"));
    }
    let frame: Frame = serde_json::from_slice(&stdout.bytes).map_err(|_| {
        CoreError::invalid(format!(
            "CodeExecutionFailed: SDK exit={exit}, missing/invalid result frame"
        ))
    })?;
    if exit != 0 || !frame.ok {
        let kind = match frame.kind.as_deref() {
            Some("syntax") => "syntax",
            _ => "runtime",
        };
        return Err(CoreError::other(format!(
            "CodeExecutionFailed: script {kind} error (exit={exit})"
        )));
    }
    let outputs = frame
        .outputs
        .ok_or_else(|| CoreError::invalid("CodeResultInvalid: main must return an object"))?;
    let mut stderr_text = frame.stderr;
    stderr_text.push_str(&String::from_utf8_lossy(&stderr.bytes));
    let stderr_truncated =
        frame.stderr_truncated || stderr.truncated || stderr_text.len() > MAX_CODE_LOG_BYTES;
    while stderr_text.len() > MAX_CODE_LOG_BYTES {
        stderr_text.pop();
    }
    let digest = format!("{:x}", Sha256::digest(request.script.as_bytes()));
    let response = CodeResponse {
        version: CODE_VERSION,
        outputs,
        receipt: CodeReceipt {
            language: request.language,
            script_sha256: digest.clone(),
            runtime_version: version,
            isolation,
            exit_code: 0,
            stdout: frame.stdout,
            stderr: stderr_text,
            stdout_truncated: frame.stdout_truncated,
            stderr_truncated,
        },
    };
    response.validate(
        &CodeInvocation {
            request_id: String::new(),
            language: request.language,
            script_sha256: digest,
        },
        &request.outputs,
    )?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_runtime::workflow::code::{CodeLanguage, CodeOutput};
    use serde_json::json;

    fn request() -> CodeRequest {
        CodeRequest {
            version: CODE_VERSION,
            language: CodeLanguage::Javascript,
            script: "PRIVATE_SOURCE".into(),
            inputs: Vec::new(),
            outputs: vec![CodeOutput {
                name: "result".into(),
                value_type: "number".into(),
            }],
        }
    }

    fn capture(bytes: Vec<u8>) -> Capture {
        Capture {
            bytes,
            truncated: false,
        }
    }

    fn parse(request: &CodeRequest, exit: u32, frame: Value) -> Result<CodeResponse> {
        decode(
            request,
            "24.21.0".into(),
            CodeIsolation::WindowsLpacV1,
            exit,
            capture(serde_json::to_vec(&frame).unwrap()),
            capture(Vec::new()),
        )
    }

    #[test]
    fn a_success_frame_requires_real_declared_values_without_coercion() {
        let response = parse(&request(), 0, json!({"ok":true,"outputs":{"result":42}})).unwrap();
        assert_eq!(response.outputs["result"], 42);
        assert_eq!(
            response.receipt.script_sha256,
            format!("{:x}", Sha256::digest(b"PRIVATE_SOURCE"))
        );
        for outputs in [
            json!({}),
            json!({"result":"42"}),
            json!({"result":false}),
            json!({"result":42,"extra":0}),
            json!([]),
            Value::Null,
        ] {
            assert!(parse(&request(), 0, json!({"ok":true,"outputs":outputs})).is_err());
        }
    }

    #[test]
    fn an_empty_declaration_still_requires_an_object_from_main() {
        let mut request = request();
        request.outputs.clear();
        assert!(parse(&request, 0, json!({"ok":true})).is_err());
        assert!(parse(&request, 0, json!({"ok":true,"outputs":null})).is_err());
        assert!(parse(&request, 0, json!({"ok":true,"outputs":{}})).is_ok());
    }

    #[test]
    fn exit_and_error_frames_can_never_fabricate_success_or_echo_source() {
        for (exit, frame) in [
            (1, json!({"ok":true,"outputs":{"result":42}})),
            (0, json!({"ok":false,"kind":"PRIVATE_SOURCE"})),
            (1, json!({"ok":false,"kind":"syntax"})),
            (0, json!({"PRIVATE_SOURCE":true})),
        ] {
            let error = parse(&request(), exit, frame)
                .err()
                .expect("must fail")
                .to_string();
            assert!(!error.contains("PRIVATE_SOURCE"));
            assert!(error.contains("CodeExecutionFailed"));
        }
    }

    #[test]
    fn malformed_or_truncated_protocol_is_never_a_result() {
        for bytes in [
            Vec::new(),
            b"PRIVATE_SOURCE".to_vec(),
            vec![b' '; MAX_CODE_RESPONSE_BYTES + 1],
        ] {
            assert!(decode(
                &request(),
                "24.21.0".into(),
                CodeIsolation::WindowsLpacV1,
                0,
                capture(bytes),
                capture(Vec::new())
            )
            .is_err());
        }
        assert!(decode(
            &request(),
            "24.21.0".into(),
            CodeIsolation::WindowsLpacV1,
            0,
            Capture {
                bytes: b"{\"ok\":true,\"outputs\":{\"result\":42}}".to_vec(),
                truncated: true
            },
            capture(Vec::new())
        )
        .is_err());
    }

    #[test]
    fn combined_native_and_adapter_stderr_is_utf8_bounded_and_marked() {
        let bytes = serde_json::to_vec(
            &json!({"ok":true,"outputs":{"result":42},"stderr":"界".repeat(MAX_CODE_LOG_BYTES)}),
        )
        .unwrap();
        let result = decode(
            &request(),
            "24.21.0".into(),
            CodeIsolation::WindowsLpacV1,
            0,
            capture(bytes),
            capture(vec![0xff; MAX_CODE_LOG_BYTES]),
        )
        .unwrap();
        assert!(result.receipt.stderr.len() <= MAX_CODE_LOG_BYTES);
        assert!(result.receipt.stderr_truncated);
    }

    #[test]
    fn oversized_adapter_stdout_is_rejected_not_silently_normalized() {
        assert!(parse(
            &request(),
            0,
            json!({"ok":true,"outputs":{"result":42},
            "stdout":"x".repeat(MAX_CODE_LOG_BYTES+1)})
        )
        .is_err());
    }
}
