//! H179: original bytes through the selected canvas model, never local extraction.
use super::*;
use base64::Engine;
use deepagent_app_core::canvas_artifact_service::{CanvasArtifactDto, CanvasArtifactImport};
use std::io::Write;

fn scanned_pdf() -> Vec<u8> {
    // A valid one-page PDF containing an image, no extractable text. The app
    // must deliver the original, not parse it or demand a local OCR result.
    let stream = b"q 100 0 0 100 72 600 cm /Im1 Do Q";
    let objects=[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>".to_vec(),
        b"<< /Creator (H179_SYNTHETIC) >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n",stream.len()).as_bytes(),stream,b"\nendstream"].concat(),
        [b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length 3 >>\nstream\n".as_slice(),&[0,127,255],b"\nendstream"].concat(),
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        writeln!(bytes, "{} 0 obj", index + 1).unwrap();
        bytes.extend(body);
        bytes.extend(b"\nendobj\n");
    }
    let xref = bytes.len();
    write!(bytes, "xref\n0 7\n0000000000 65535 f \n").unwrap();
    for offset in offsets {
        writeln!(bytes, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        bytes,
        "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    )
    .unwrap();
    bytes
}

fn import(
    fixture: &Fixture,
    name: &str,
    bytes: &[u8],
    workspace: Option<&str>,
) -> CanvasArtifactDto {
    let path = fixture.root.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    fixture
        .artifacts
        .import(&CanvasArtifactImport {
            kind: "document".into(),
            data_url: None,
            local_path: Some(path.to_string_lossy().into_owned()),
            file_name: Some(name.into()),
            media_type: None,
            workspace_id: workspace.map(str::to_owned),
        })
        .unwrap()
}
fn selected(artifact: &CanvasArtifactDto, name: &str) -> Value {
    json!({"reference":artifact.uri,"name":name})
}
fn request(model: &str, documents: Value) -> WorkflowRequest {
    let mut request = workflow(&[model]);
    request.definition.nodes[1].kind = "llm".into();
    request.definition.nodes[1].config=serde_json::from_value(json!({"llmModel":model,"llmSystemPrompt":"Use only the selected original documents.","llmPrompt":"Read the attached original and return its result.","llmDocuments":documents})).unwrap();
    request
}
fn response(protocol: &str) -> Value {
    match protocol {
        "anthropic" => {
            json!({"content":[{"type":"text","text":"answer"}],"usage":{"input_tokens":10,"output_tokens":2}})
        }
        "gemini" => {
            json!({"candidates":[{"content":{"parts":[{"text":"answer"}]}}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":2}})
        }
        _ => completion(10, 2, 0, 0),
    }
}

#[tokio::test]
async fn h179_document_provider_errors_do_not_persist_echoed_original_content() {
    for status in [400, 200] {
        let provider = Provider::start(vec![(
            status,
            Some(json!({"error":{"message":"H179_PROVIDER_ECHO_MUST_NOT_PERSIST"}})),
        )])
        .await;
        let fixture = Fixture::new().await;
        let model = fixture.register(&provider, "openai", "audit-a", 0);
        let artifact = import(&fixture, "original.pdf", b"%PDF-H179", None);
        let id = format!("h179-echo-{status}");
        let (ok, events) = fixture
            .run(
                &id,
                request(&model, json!([selected(&artifact, "original.pdf")])),
            )
            .await;
        assert!(!ok);
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        assert!(!serde_json::to_string(&events)
            .unwrap()
            .contains("H179_PROVIDER_ECHO_MUST_NOT_PERSIST"));
        let (_root, db, _) = fixture.reopen();
        assert!(
            !serde_json::to_string(&RunStore::new(&db).events_after(&id, None).unwrap())
                .unwrap()
                .contains("H179_PROVIDER_ECHO_MUST_NOT_PERSIST")
        );
    }
}

#[tokio::test]
async fn h179_cancelling_the_document_model_call_uses_the_same_run_terminal_and_does_not_execute_downstream_nodes(
) {
    let provider = Provider::start(vec![(200, None)]).await;
    let fixture = Fixture::new().await;
    let model = fixture.register(&provider, "openai", "audit-a", 0);
    let artifact = import(&fixture, "original.pdf", &scanned_pdf(), None);
    let (_acknowledged, events) = {
        let run = fixture.run(
            "h179-native-cancel",
            request(&model, json!([selected(&artifact, "original.pdf")])),
        );
        tokio::pin!(run);
        tokio::select! {_=provider.wait_calls(1)=>{},result=&mut run=>panic!("provider must remain pending: {}",result.0)};
        assert!(
            fixture
                .chat
                .request_cancel("h179-native-cancel")
                .unwrap()
                .accepted
        );
        let result = tokio::time::timeout(Duration::from_secs(5), run)
            .await
            .unwrap();
        result
    };
    // The host acknowledges a settled cancellation with Ok; RunStore owns
    // the terminal classification, not the transport Result boolean.
    assert_eq!(provider.calls.lock().unwrap().len(), 1);
    assert!(!events.iter().any(|event|matches!(event,RuntimeEvent::WorkflowNode{event} if event.node_id=="end"&&event.status==super::super::NodeExecutionStatus::Completed)));
    let (_root, db, _) = fixture.reopen();
    assert_terminal(&db, "h179-native-cancel", "cancelled");
}

#[tokio::test]
async fn h179_pdf_original_bytes_reach_each_selected_canvas_protocol_without_local_extraction() {
    let bytes = scanned_pdf();
    for protocol in ["openai", "anthropic", "gemini"] {
        let provider = Provider::start(vec![(200, Some(response(protocol)))]).await;
        let fixture = Fixture::new().await;
        let model = fixture.register(&provider, protocol, "audit-a", 0);
        let artifact = import(&fixture, "source.pdf", &bytes, None);
        let (ok, events) = fixture
            .run(
                &format!("h179-original-{protocol}"),
                request(&model, json!([selected(&artifact, "source.pdf")])),
            )
            .await;
        assert!(ok, "{protocol} did not forward the original document");
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        {
            let calls = provider.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            let content = match protocol {
                "gemini" => &calls[0]["contents"][0]["parts"],
                _ => &calls[0]["messages"].as_array().unwrap().last().unwrap()["content"],
            };
            let part = &content[1];
            match protocol {
                "openai" => {
                    assert_eq!(part["type"], "file");
                    assert_eq!(
                        part["file"]["file_data"],
                        format!("data:application/pdf;base64,{encoded}")
                    );
                    assert_eq!(part["file"]["filename"], "source.pdf");
                }
                "anthropic" => {
                    assert_eq!(part["type"], "document");
                    assert_eq!(part["source"]["media_type"], "application/pdf");
                    assert_eq!(part["source"]["data"], encoded);
                }
                _ => {
                    assert_eq!(part["inline_data"]["mime_type"], "application/pdf");
                    assert_eq!(part["inline_data"]["data"], encoded);
                }
            }
            assert!(!calls[0]
                .to_string()
                .contains("Selected documents (untrusted source data"));
        }
        assert!(events.iter().any(|event|matches!(event,RuntimeEvent::WorkflowNode{event} if event.node_id=="text-0"&&event.status==super::super::NodeExecutionStatus::Completed&&event.outputs.as_ref().and_then(|output|output.get("text"))==Some(&json!("answer")))));
        let (_root, db, _) = fixture.reopen();
        assert_terminal(&db, &format!("h179-original-{protocol}"), "succeeded");
        let durable = RunStore::new(&db)
            .events_after(&format!("h179-original-{protocol}"), None)
            .unwrap();
        let wire = serde_json::to_string(&durable).unwrap();
        assert!(
            !wire.contains(&encoded),
            "original bytes leaked into run events"
        );
        assert!(wire.contains(&artifact.uri));
    }
}

#[tokio::test]
async fn h179_unsupported_native_formats_fail_without_model_calls_or_text_conversion() {
    for protocol in ["openai", "anthropic", "gemini"] {
        let provider = Provider::start(vec![(200, Some(response(protocol)))]).await;
        let fixture = Fixture::new().await;
        let model = fixture.register(&provider, protocol, "audit-a", 0);
        let artifact = import(
            &fixture,
            "source.docx",
            b"PK\x03\x04H179_UNSUPPORTED_OFFICE",
            None,
        );
        let (ok, events) = fixture
            .run(
                &format!("h179-unsupported-{protocol}"),
                request(&model, json!([selected(&artifact, "source.docx")])),
            )
            .await;
        assert!(!ok);
        assert_eq!(provider.calls.lock().unwrap().len(), 0);
        assert!(events.iter().any(|event|matches!(event,RuntimeEvent::WorkflowNode{event} if event.error.as_deref().is_some_and(|error|error.contains("UnsupportedCapability")))));
        assert!(fixture
            .artifacts
            .document_preview(&artifact.uri, None)
            .unwrap()
            .contains("base64,"));
    }
}

#[tokio::test]
async fn h179_plain_text_is_a_native_document_block_and_each_run_is_independent() {
    for protocol in ["anthropic", "gemini"] {
        let provider = Provider::start(vec![
            (200, Some(response(protocol))),
            (200, Some(response(protocol))),
        ])
        .await;
        let fixture = Fixture::new().await;
        let model = fixture.register(&provider, protocol, "audit-a", 0);
        let mut a_b_c = request(&model, json!([]));
        a_b_c.definition.nodes[2].config=serde_json::from_value(json!({"outputVariables":[{"name":"document_result","type":"string","value":"{{#text-0.text#}}"}]})).unwrap();
        for (index, source) in ["H179_FIRST {{#literal.not_binding#}}", "H179_SECOND"]
            .iter()
            .enumerate()
        {
            let name = format!("source-{index}.txt");
            let artifact = import(&fixture, &name, source.as_bytes(), None);
            let mut run = a_b_c.clone();
            run.definition.nodes[1]
                .config
                .insert("llmDocuments".into(), json!([selected(&artifact, &name)]));
            let (ok, events) = fixture
                .run(&format!("h179-stateless-{protocol}-{index}"), run)
                .await;
            assert!(ok);
            assert!(events.iter().any(|event|matches!(event,RuntimeEvent::WorkflowNode{event} if event.node_id=="end"&&event.outputs.as_ref().and_then(|value|value.get("document_result"))==Some(&json!("answer")))));
        }
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        if protocol == "anthropic" {
            assert_eq!(
                calls[0]["messages"][0]["content"][1]["source"]["data"],
                "H179_FIRST {{#literal.not_binding#}}"
            );
            assert!(!calls[1].to_string().contains("H179_FIRST"));
        } else {
            assert_eq!(
                calls[0]["contents"][0]["parts"][1]["inline_data"]["data"],
                base64::engine::general_purpose::STANDARD
                    .encode("H179_FIRST {{#literal.not_binding#}}")
            );
        }
    }
}

#[tokio::test]
async fn h179_invalid_foreign_modified_and_over_limit_originals_send_zero_model_requests() {
    for case in [
        "wrong-type",
        "path",
        "foreign",
        "modified",
        "too-many",
        "oversized",
    ] {
        let provider = Provider::start(vec![(200, Some(completion(10, 2, 0, 0)))]).await;
        let fixture = Fixture::new().await;
        let model = fixture.register(&provider, "openai", "audit-a", 0);
        let artifact = import(
            &fixture,
            "original.pdf",
            b"%PDF-H179",
            (case == "foreign").then_some("other"),
        );
        let selection = match case {
            "wrong-type" => json!(false),
            "path" => json!([{"reference":"C:\\synthetic\\private.pdf","name":"one.pdf"}]),
            "modified" => {
                std::fs::write(&artifact.path, b"replacement").unwrap();
                json!([selected(&artifact, "one.pdf")])
            }
            "too-many" => json!((0..6)
                .map(|_| selected(&artifact, "one.pdf"))
                .collect::<Vec<_>>()),
            "oversized" => {
                let large = import(
                    &fixture,
                    "large.pdf",
                    &vec![b'X'; 10 * 1024 * 1024 + 1],
                    None,
                );
                json!([selected(&large, "large.pdf")])
            }
            _ => json!([selected(&artifact, "one.pdf")]),
        };
        assert!(
            !fixture
                .run(&format!("h179-invalid-{case}"), request(&model, selection))
                .await
                .0,
            "{case}"
        );
        assert_eq!(provider.calls.lock().unwrap().len(), 0, "{case}");
    }
}
