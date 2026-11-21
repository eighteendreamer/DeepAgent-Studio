//! H167: real image transport, existing one-shot broker and artifact staging.
use super::*;
use crate::canvas_artifact_service::CanvasArtifactService;
use crate::canvas_provider_service::CanvasProviderInput;
use crate::MemorySecretStore;
use deepagent_persistence::Database;
use std::path::Path;

// Proposed local output policy, not an input-attachment or provider standard.
const IMAGE_LIMIT: usize = 32 * 1024 * 1024;
const PRIVATE_MARKER: &str = "H167_SYNTHETIC_PRIVATE_BODY";

use super::media_test_support::{Framing, MediaWire as ImageWire};

fn image_bytes(size: usize) -> Vec<u8> {
    let mut body = vec![b'I'; size];
    body[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    body
}

fn host(
    root: &Path,
    wire: &ImageWire,
) -> (Arc<CanvasModelGateway>, Arc<CanvasArtifactService>, String) {
    host_with_limit(root, wire, DEFAULT_IMAGE_OUTPUT_LIMIT)
}

fn host_with_limit(
    root: &Path,
    wire: &ImageWire,
    max_bytes: u64,
) -> (Arc<CanvasModelGateway>, Arc<CanvasArtifactService>, String) {
    let db = Arc::new(Database::open(root.join("image.db")).unwrap());
    let artifacts =
        Arc::new(CanvasArtifactService::new(root.join("artifacts"), db.clone()).unwrap());
    let providers = Arc::new(CanvasProviderService::new(
        db,
        Arc::new(MemorySecretStore::new()),
    ));
    let provider = providers
        .save_provider(CanvasProviderInput {
            name: "Synthetic H167".into(),
            protocol: "openai".into(),
            base_url: format!("http://{}/v1", wire.address),
            enabled: true,
            api_key: Some("synthetic-h167-key".into()),
            models: vec![CanvasModelConfig {
                id: "synthetic-image".into(),
                name: "Synthetic image".into(),
                description: String::new(),
                enabled: true,
                scenarios: vec![CanvasScenario::ImageGeneration],
                priority: 0,
                max_reference_images: None,
            }],
            ..Default::default()
        })
        .unwrap();
    let gateway = Arc::new(
        CanvasModelGateway::new(providers)
            .with_artifacts(artifacts.clone())
            .with_public_fixture_route(wire.address)
            .with_image_output_limit(max_bytes)
            .unwrap(),
    );
    (
        gateway.scoped_for_run(Some("h167-owner")).unwrap(),
        artifacts,
        provider.id,
    )
}

#[tokio::test]
async fn h167_exact_download_limit_and_larger_config_preserve_bytes_after_reopen() {
    for (size, framing) in [
        (64, Framing::Length(64)),
        (64, Framing::Chunked),
        (64, Framing::Close),
        (IMAGE_LIMIT, Framing::Length(IMAGE_LIMIT)),
        (IMAGE_LIMIT + 1, Framing::Chunked),
    ] {
        let body = image_bytes(size);
        let mut wire = ImageWire::start(body.clone(), framing, false, false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, _) = host_with_limit(root.path(), &wire, size as u64);
        assert_eq!(
            gateway.image_output_limit, size as u64,
            "run scope preserves limit"
        );
        let response = gateway
            .image_fetch_tool()
            .invoke(arguments(&gateway, &wire))
            .await
            .unwrap();
        wire.close().await;
        let uri = response.value["artifactUri"].as_str().unwrap();
        let id = uri.trim_start_matches("artifact://");
        assert_eq!(
            artifacts
                .read_bytes(id, Some("h167-owner"))
                .unwrap()
                .unwrap(),
            body
        );
        assert!(artifacts.record(id, None).unwrap().is_none());
        drop(gateway);
        drop(artifacts);
        let reopened = CanvasArtifactService::new(
            root.path().join("artifacts"),
            Arc::new(Database::open(root.path().join("image.db")).unwrap()),
        )
        .unwrap();
        assert_eq!(
            reopened
                .read_bytes(id, Some("h167-owner"))
                .unwrap()
                .unwrap(),
            body
        );
        assert!(std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")));
    }
}

#[tokio::test]
async fn h167_size_refusal_does_not_dispatch_a_fallback_provider() {
    let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes(65));
    let raw = serde_json::to_vec(&json!({"data":[{"b64_json":encoded}]})).unwrap();
    let mut first = ImageWire::start(raw, Framing::Chunked, true, false).await;
    let mut fallback = ImageWire::start(Vec::new(), Framing::Length(0), true, false).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, first_id) = host_with_limit(root.path(), &first, 64);
    let second = gateway
        .providers
        .save_provider(CanvasProviderInput {
            name: "Synthetic fallback".into(),
            protocol: "openai".into(),
            base_url: format!("http://{}/v1", fallback.address),
            enabled: true,
            api_key: Some("synthetic-h167-fallback".into()),
            models: vec![CanvasModelConfig {
                id: "fallback-image".into(),
                name: "Fallback".into(),
                description: String::new(),
                enabled: true,
                scenarios: vec![CanvasScenario::ImageGeneration],
                priority: -1,
                max_reference_images: None,
            }],
            ..Default::default()
        })
        .unwrap();
    let candidates = gateway
        .providers
        .candidates_for_scenario(CanvasScenario::ImageGeneration, Some("h167-owner"))
        .unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].provider.id, first_id);
    assert_eq!(candidates[1].provider.id, second.id);
    let request =
        CanvasModelRequest::text("synthetic image").with_operation(CanvasOperation::ImageGenerate);
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        gateway.execute(&request, Some("h167-owner")),
    )
    .await
    .unwrap()
    .unwrap_err();
    first.close().await;
    fallback.close().await;
    assert_eq!(error.code, CanvasErrorCode::ResponseBodyTooLarge);
    assert_eq!(first.requests.load(Ordering::SeqCst), 1);
    assert_eq!(fallback.requests.load(Ordering::SeqCst), 0);
    assert_empty(&artifacts, root.path());
}

#[tokio::test]
async fn h167_overflow_during_another_active_download_cannot_remove_its_staging() {
    let mut held = ImageWire::start(image_bytes(12), Framing::Length(64), false, true).await;
    let mut large = ImageWire::start(image_bytes(65), Framing::Chunked, false, false).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, _) = host_with_limit(root.path(), &held, 64);
    let other = Arc::new(
        CanvasModelGateway::new(gateway.providers.clone())
            .with_artifacts(artifacts.clone())
            .with_public_fixture_route(large.address)
            .with_image_output_limit(64)
            .unwrap(),
    )
    .scoped_for_run(Some("h167-owner"))
    .unwrap();
    let context = ToolExecutionContext::default();
    let token = context.cancellation_token();
    let tool = gateway.image_fetch_tool();
    let work = tool.invoke_with_context(arguments(&gateway, &held), context);
    tokio::pin!(work);
    let staged = async {
        loop {
            if std::fs::read_dir(root.path().join("artifacts"))
                .unwrap()
                .count()
                == 1
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::select! { _ = staged => {}, result = &mut work => panic!("expected held download {result:?}") }
    let other_tool = other.image_fetch_tool();
    let refusal = other_tool.invoke(arguments(&other, &large));
    tokio::pin!(refusal);
    let error = tokio::select! {
        result = &mut refusal => result.unwrap_err(),
        result = &mut work => panic!("held download unexpectedly ended {result:?}")
    };
    assert!(error.to_string().contains("ImageArtifactTooLarge"));
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        1
    );
    token.cancel();
    assert!(tokio::time::timeout(Duration::from_secs(2), &mut work)
        .await
        .unwrap()
        .is_err());
    held.close().await;
    large.close().await;
    assert_empty(&artifacts, root.path());
}

#[tokio::test]
async fn h167_partial_image_cancel_deadline_and_truncation_remove_staging() {
    for mode in ["cancel", "deadline", "truncated"] {
        let mut wire = ImageWire::start(
            image_bytes(12),
            Framing::Length(64),
            false,
            mode != "truncated",
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, _) = host_with_limit(root.path(), &wire, 64);
        let args = arguments(&gateway, &wire);
        let tool = gateway.image_fetch_tool();
        let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(400));
        let token = context.cancellation_token();
        let work = tool.invoke_with_context(args.clone(), context);
        tokio::pin!(work);
        if mode != "truncated" {
            tokio::select! {_ = wire.sent.notified() => {}, result = &mut work => panic!("early response {result:?}") }
            let staged = async {
                loop {
                    if std::fs::read_dir(root.path().join("artifacts"))
                        .unwrap()
                        .count()
                        == 1
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            };
            tokio::select! { _ = staged => {}, result = &mut work => panic!("expected staging {result:?}") }
            if mode == "cancel" {
                token.cancel();
            }
        }
        let error = tokio::time::timeout(Duration::from_secs(2), &mut work)
            .await
            .unwrap()
            .unwrap_err();
        if mode != "truncated" {
            assert!(
                error.to_string().contains(if mode == "cancel" {
                    "cancelled"
                } else {
                    "deadline exceeded"
                }),
                "{error}"
            );
        }
        tokio::time::timeout(Duration::from_secs(2), wire.closed.notified())
            .await
            .unwrap();
        wire.close().await;
        assert!(tool.invoke(args).await.is_err());
        assert_empty(&artifacts, root.path());
    }
}

#[tokio::test]
async fn h167_actual_staging_create_and_sql_publication_failures_leave_no_artifact() {
    for fail_index in [false, true] {
        let mut wire = ImageWire::start(image_bytes(64), Framing::Length(64), false, false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, _) = host_with_limit(root.path(), &wire, 64);
        if fail_index {
            let db = Database::open(root.path().join("image.db")).unwrap();
            db.with_conn(|connection| {
                connection
                    .execute_batch(
                        "CREATE TRIGGER h167_reject_publication BEFORE INSERT ON artifacts
                 BEGIN SELECT RAISE(ABORT, 'synthetic image publication failure'); END;",
                    )
                    .map_err(|error| deepagent_core::CoreError::other(error.to_string()))
            })
            .unwrap();
        } else {
            // Real filesystem error, without changing ACLs or filling a disk.
            std::fs::remove_dir(root.path().join("artifacts")).unwrap();
            std::fs::write(root.path().join("artifacts"), b"synthetic path conflict").unwrap();
        }
        let result = gateway
            .image_fetch_tool()
            .invoke(arguments(&gateway, &wire))
            .await;
        wire.close().await;
        assert!(result.is_err());
        if !fail_index {
            std::fs::remove_file(root.path().join("artifacts")).unwrap();
            std::fs::create_dir(root.path().join("artifacts")).unwrap();
        }
        assert_empty(&artifacts, root.path());
        assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn h167_base64_padding_invalid_input_and_config_overflow_are_explicit() {
    for size in 1..=9 {
        let bytes = vec![b'x'; size];
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decode_provider_image(&encoded, size as u64).unwrap(), bytes);
        assert_eq!(
            decode_provider_image(&encoded, size as u64 - 1)
                .unwrap_err()
                .code,
            CanvasErrorCode::ResponseBodyTooLarge
        );
    }
    assert_eq!(
        decode_provider_image("????", 64).unwrap_err().code,
        CanvasErrorCode::ProviderFailure
    );
    for limit in [0, u64::MAX] {
        assert!(image_response_limit(limit).is_err());
    }
    assert_eq!(
        image_response_limit(64).unwrap(),
        88 + IMAGE_RESPONSE_ENVELOPE_BYTES as usize
    );
}

#[tokio::test]
async fn h167_inline_exact_limit_protocol_shapes_and_oversize_before_decode() {
    for (size, ok) in [(64, true), (65, false)] {
        for protocol in [CanvasProtocol::OpenAi, CanvasProtocol::Gemini] {
            let body = image_bytes(size);
            let encoded = base64::engine::general_purpose::STANDARD.encode(&body);
            let value = match protocol {
                CanvasProtocol::Gemini => {
                    json!({"candidates":[{"content":{"parts":[{"inline_data":{"data":encoded,"mimeType":"image/png"}}]}}]})
                }
                _ => json!({"data":[{"b64_json":encoded}]}),
            };
            let raw = serde_json::to_vec(&value).unwrap();
            let mut wire = ImageWire::start(raw, Framing::Chunked, true, false).await;
            let root = tempfile::tempdir().unwrap();
            let (gateway, artifacts, provider_id) = host_with_limit(root.path(), &wire, 64);
            let candidate = gateway
                .providers
                .resolve_model(&provider_id, "synthetic-image")
                .unwrap();
            let output =
                parse_output(&candidate, CanvasRequestKind::ImageGenerate, &value, 64).await;
            if ok {
                assert!(matches!(output,Ok(CanvasModelOutput::Image{bytes,..}) if bytes==body));
            } else {
                assert_eq!(
                    output.unwrap_err().code,
                    CanvasErrorCode::ResponseBodyTooLarge
                );
            }
            // Run the real HTTP/provider/bridge persistence path as well.
            let result = gateway
                .prepare_image(CanvasImageRequest {
                    model_ref: format!("{provider_id}::synthetic-image"),
                    prompt: "synthetic inline".into(),
                    reference_images: Vec::new(),
                    size: None,
                    operation: "image_generate".into(),
                })
                .await;
            wire.close().await;
            if ok {
                let CanvasImagePreparation::Ready(image) = result.unwrap() else {
                    panic!("inline must be ready");
                };
                assert_eq!(
                    artifacts
                        .read_bytes(
                            image.artifact_uri.trim_start_matches("artifact://"),
                            Some("h167-owner")
                        )
                        .unwrap()
                        .unwrap(),
                    body
                );
            } else {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("ImageArtifactTooLarge"));
                assert_empty(&artifacts, root.path());
            }
        }
    }
}

#[tokio::test]
async fn h167_image_json_envelope_accepts_exact_bound_and_rejects_one_more() {
    for extra in [0, 1] {
        let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes(64));
        let mut value = json!({"data":[{"b64_json":encoded}],"metadata":""});
        let cap = image_response_limit(64).unwrap();
        let remaining = cap - serde_json::to_vec(&value).unwrap().len() + extra;
        value["metadata"] = json!("x".repeat(remaining));
        let raw = serde_json::to_vec(&value).unwrap();
        assert_eq!(raw.len(), cap + extra);
        let mut wire = ImageWire::start(raw, Framing::Chunked, true, false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, id) = host_with_limit(root.path(), &wire, 64);
        let result = gateway
            .execute_on(
                &id,
                "synthetic-image",
                &CanvasModelRequest::text("synthetic image")
                    .with_operation(CanvasOperation::ImageGenerate),
            )
            .await;
        wire.close().await;
        if extra == 0 {
            assert!(matches!(result,Ok(CanvasModelOutput::Image{bytes,..}) if bytes.len()==64));
        } else {
            assert_eq!(
                result.unwrap_err().code,
                CanvasErrorCode::ResponseBodyTooLarge
            );
        }
        assert_empty(&artifacts, root.path()); // Low-level execute does not publish.
    }
}

#[tokio::test]
async fn h167_provider_formats_and_local_imports_keep_their_existing_contract() {
    use deepagent_persistence::artifact_store::ArtifactKind;
    for prefix in [
        b"\x89PNG\r\n\x1a\n".as_slice(),
        b"\xff\xd8\xff",
        b"GIF89a",
        b"RIFF1234WEBP",
    ] {
        let mut bytes = vec![0u8; 64];
        bytes[..prefix.len()].copy_from_slice(prefix);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        assert_eq!(decode_provider_image(&encoded, 64).unwrap(), bytes);
    }
    let root = tempfile::tempdir().unwrap();
    let service = CanvasArtifactService::new(
        root.path().join("artifacts"),
        Arc::new(Database::open(root.path().join("image.db")).unwrap()),
    )
    .unwrap();
    let body = image_bytes(IMAGE_LIMIT + 1);
    let record = service
        .import_bytes(ArtifactKind::Image, Some("image/png"), &body, None)
        .unwrap();
    assert_eq!(
        record.byte_size,
        body.len() as i64,
        "provider policy must not cap local imports"
    );
    assert_eq!(service.read_bytes(&record.id, None).unwrap().unwrap(), body);
}

fn arguments(gateway: &CanvasModelGateway, wire: &ImageWire) -> Value {
    let CanvasImagePreparation::Deferred { request_id, origin } = gateway
        .stage_image_url(
            wire.url(),
            "synthetic-provider".into(),
            "synthetic-image".into(),
            "generate".into(),
        )
        .unwrap()
    else {
        panic!("URL must be deferred");
    };
    json!({"request_id": request_id, "origin": origin})
}

fn assert_empty(artifacts: &CanvasArtifactService, root: &Path) {
    assert!(artifacts.list(Some("h167-owner"), None).unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(root.join("artifacts")).unwrap().count(),
        0
    );
}

async fn refuses_before_eof(body: Vec<u8>, framing: Framing) {
    let mut wire = ImageWire::start(body, framing, false, true).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, _) = host(root.path(), &wire);
    let tool = gateway.image_fetch_tool();
    let args = arguments(&gateway, &wire);
    let context = ToolExecutionContext::default().with_timeout(Duration::from_secs(5));
    let result = wire
        .before_eof(tool.invoke_with_context(args.clone(), context))
        .await;
    wire.close().await;
    let error = result
        .expect("image limit must refuse without EOF")
        .unwrap_err();
    assert!(
        error.to_string().contains("ImageArtifactTooLarge"),
        "{error}"
    );
    assert!(!error.to_string().contains("synthetic-h167"));
    assert!(!error.to_string().contains("budget")); // Kernel must report failed, not budget_exceeded.
    assert!(
        tool.invoke(args).await.is_err(),
        "one-shot refusal cannot replay"
    );
    assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    assert_empty(&artifacts, root.path());
}

#[tokio::test]
async fn h167_declared_image_overflow_refuses_before_body_or_eof() {
    refuses_before_eof(Vec::new(), Framing::Length(IMAGE_LIMIT + 1)).await;
}

#[tokio::test]
async fn h167_chunked_image_overflow_refuses_before_eof() {
    refuses_before_eof(image_bytes(IMAGE_LIMIT + 1), Framing::Chunked).await;
}

#[tokio::test]
async fn h167_unknown_length_image_overflow_refuses_before_eof() {
    refuses_before_eof(image_bytes(IMAGE_LIMIT + 1), Framing::Close).await;
}

#[tokio::test]
async fn valid_image_withheld_eof_reaches_the_request_deadline_instead_of_size_refusal() {
    let mut wire = ImageWire::start(image_bytes(64), Framing::Chunked, false, true).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, _) = host(root.path(), &wire);
    let tool = gateway.image_fetch_tool();
    let context = ToolExecutionContext::default().with_timeout(Duration::from_millis(400));
    let error = wire
        .before_eof(tool.invoke_with_context(arguments(&gateway, &wire), context))
        .await
        .expect("the request must keep its own deadline while EOF is withheld")
        .unwrap_err();
    assert!(error.to_string().contains("deadline exceeded"), "{error}");
    assert!(!error.to_string().contains("ImageArtifactTooLarge"));
    tokio::time::timeout(Duration::from_secs(2), wire.closed.notified())
        .await
        .unwrap();
    wire.close().await;
    assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    assert_empty(&artifacts, root.path());
}

#[tokio::test]
async fn h167_successful_image_json_body_is_bounded_before_eof_and_parsing() {
    let raw_limit = IMAGE_LIMIT.div_ceil(3) * 4 + 1024 * 1024;
    let mut raw = vec![b'X'; raw_limit + 1];
    raw[..PRIVATE_MARKER.len()].copy_from_slice(PRIVATE_MARKER.as_bytes());
    let mut wire = ImageWire::start(raw, Framing::Chunked, true, true).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, provider_id) = host(root.path(), &wire);
    let mut request =
        CanvasModelRequest::text("synthetic image").with_operation(CanvasOperation::ImageGenerate);
    request.timeout_ms = 5000;
    let result = wire
        .before_eof(gateway.execute_on(&provider_id, "synthetic-image", &request))
        .await;
    wire.close().await;
    let error = result
        .expect("raw image JSON budget must not wait for EOF")
        .unwrap_err();
    assert_eq!(error.code, CanvasErrorCode::ResponseBodyTooLarge);
    assert!(
        !error.is_transient(),
        "size policy is not candidate fallback"
    );
    assert!(!error.to_string().contains(PRIVATE_MARKER));
    assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    assert_empty(&artifacts, root.path());
}
