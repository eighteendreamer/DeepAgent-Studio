use super::*;

fn config() -> Map<String, Value> {
    json!({
        "codeLanguage":"javascript",
        "codeScript":"function main(inputs) { return { result: '{{#unclosed' }; }",
        "codeInputVariables":[{"name":"input","value":"{{#start.input#}}"}],
        "codeOutputVariables":[{"name":"result","type":"string","description":"UI metadata"}],
        "label":"code",
    })
    .as_object()
    .unwrap()
    .clone()
}

fn invocation() -> CodeInvocation {
    CodeInvocation {
        request_id: "opaque".into(),
        language: CodeLanguage::Javascript,
        script_sha256: "a".repeat(64),
    }
}

fn response() -> CodeResponse {
    CodeResponse {
        version: CODE_VERSION,
        outputs: json!({"result":"computed"}).as_object().unwrap().clone(),
        receipt: CodeReceipt {
            language: CodeLanguage::Javascript,
            script_sha256: invocation().script_sha256,
            runtime_version: "24.21.0".into(),
            isolation: CodeIsolation::WindowsAppcontainerV1,
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        },
    }
}

fn declarations() -> Vec<CodeOutput> {
    vec![CodeOutput {
        name: "result".into(),
        value_type: "string".into(),
    }]
}

#[test]
fn source_is_literal_and_only_input_bindings_are_resolved() {
    let config = config();
    let previous = BTreeMap::from([("start".into(), json!({"input":{"nested":[true,1]}}))]);
    let request = CodeRequest::from_config(&config, &previous).unwrap();
    assert_eq!(request.script, config["codeScript"]);
    assert_eq!(request.input_object()["input"], json!({"nested":[true,1]}));
    assert_eq!(request.outputs, declarations());
    let encoded = serde_json::to_value(&request).unwrap();
    assert_eq!(encoded["version"], CODE_VERSION);
    assert_eq!(encoded["codeInputVariables"][0]["name"], "input");
    assert_eq!(
        serde_json::from_value::<CodeRequest>(encoded)
            .unwrap()
            .script,
        request.script
    );
}

#[test]
fn empty_unknown_language_wrong_shapes_and_oversized_source_fail_without_echo() {
    for (field, value) in [
        ("codeScript", json!(" \n\t")),
        ("codeScript", json!("x".repeat(MAX_CODE_BYTES + 1))),
        ("codeLanguage", json!("ruby_PRIVATE")),
        ("codeInputVariables", json!({"PRIVATE":"value"})),
        ("codeOutputVariables", json!("PRIVATE")),
    ] {
        let mut config = config();
        config.insert(field.into(), value);
        let error = CodeRequest::from_config(&config, &BTreeMap::new())
            .err()
            .unwrap();
        assert!(!error.to_string().contains("PRIVATE"));
    }
}

#[test]
fn duplicate_and_reserved_names_are_not_silently_overwritten() {
    for entries in [
        json!([{"name":"same","value":1},{"name":"same","value":2}]),
        json!([{"name":"__proto__","value":1}]),
        json!([{"name":"constructor","value":1}]),
    ] {
        let mut config = config();
        config.insert("codeInputVariables".into(), entries);
        assert!(CodeRequest::from_config(&config, &BTreeMap::new()).is_err());
    }
    let mut config = config();
    config.insert(
        "codeOutputVariables".into(),
        json!([
            {"name":"result","type":"string"},{"name":"result","type":"number"}
        ]),
    );
    assert!(CodeRequest::from_config(&config, &BTreeMap::new()).is_err());
}

#[test]
fn legacy_fields_cannot_substitute_for_the_published_source() {
    let config = json!({"code":"function main() {}", "codeVariables":[]})
        .as_object()
        .unwrap()
        .clone();
    assert!(CodeRequest::from_config(&config, &BTreeMap::new()).is_err());
}

#[test]
fn input_payload_and_nesting_use_existing_workflow_value_limits() {
    let mut config = config();
    config.insert(
        "codeInputVariables".into(),
        json!([{"name":"input","value":"x".repeat(1024*1024)}]),
    );
    assert!(CodeRequest::from_config(&config, &BTreeMap::new()).is_err());
    let mut nested = json!(1);
    for _ in 0..65 {
        nested = json!([nested]);
    }
    config.insert(
        "codeInputVariables".into(),
        json!([{"name":"input","value":nested}]),
    );
    assert!(CodeRequest::from_config(&config, &BTreeMap::new()).is_err());
}

#[test]
fn only_zero_exit_correlated_versioned_receipts_are_successful() {
    response().validate(&invocation(), &declarations()).unwrap();
    for mutation in 0..6 {
        let mut result = response();
        match mutation {
            0 => result.version += 1,
            1 => result.receipt.language = CodeLanguage::Python,
            2 => result.receipt.script_sha256 = "b".repeat(64),
            3 => result.receipt.exit_code = 1,
            4 => result.receipt.runtime_version.clear(),
            _ => result.receipt.stdout = "x".repeat(MAX_CODE_LOG_BYTES + 1),
        }
        assert!(result.validate(&invocation(), &declarations()).is_err());
    }
}

#[test]
fn missing_extra_null_and_wrongly_typed_outputs_never_get_default_values() {
    for output in [
        json!({}),
        json!({"result":null}),
        json!({"result":0}),
        json!({"result":"ok","extra":1}),
    ] {
        let mut result = response();
        result.outputs = output.as_object().unwrap().clone();
        assert!(result.validate(&invocation(), &declarations()).is_err());
    }
}

#[test]
fn declared_json_types_are_exact_not_coerced() {
    for (kind, value) in [
        ("string", json!("text")),
        ("number", json!(4.5)),
        ("boolean", json!(false)),
        ("object", json!({"a":1})),
        ("array", json!([null, 2])),
        ("file", json!("artifact://existing")),
    ] {
        let mut result = response();
        result.outputs.insert("result".into(), value);
        result
            .validate(
                &invocation(),
                &[CodeOutput {
                    name: "result".into(),
                    value_type: kind.into(),
                }],
            )
            .unwrap();
    }
    let mut result = response();
    result
        .outputs
        .insert("result".into(), json!("C:\\private\\file.txt"));
    assert!(result
        .validate(
            &invocation(),
            &[CodeOutput {
                name: "result".into(),
                value_type: "file".into()
            }]
        )
        .is_err());
}

#[test]
fn unknown_request_versions_and_unknown_wire_fields_fail() {
    let config = config();
    let previous = BTreeMap::from([("start".into(), json!({"input":1}))]);
    let mut request = CodeRequest::from_config(&config, &previous).unwrap();
    request.version += 1;
    assert!(request.validate().is_err());
    let mut value = serde_json::to_value(response()).unwrap();
    value["pretend_sandbox"] = json!(true);
    assert!(serde_json::from_value::<CodeResponse>(value).is_err());
}
