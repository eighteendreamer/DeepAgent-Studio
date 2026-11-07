//! H248 real provider responses through the existing speech bridge and store.
use super::media_test_support::{Framing, MediaWire};
use super::*;
use crate::canvas_artifact_service::CanvasArtifactService;
use crate::canvas_provider_service::CanvasProviderInput;
use crate::MemorySecretStore;
use deepagent_persistence::Database;
use std::path::Path;

const LOCAL_SPEECH_LIMIT: usize = 32 * 1024 * 1024;

fn audio_bytes(size: usize) -> Vec<u8> {
    let mut bytes = vec![b'A'; size];
    bytes[..3].copy_from_slice(b"ID3");
    bytes
}

fn host(
    root: &Path,
    wire: &MediaWire,
) -> (Arc<CanvasModelGateway>, Arc<CanvasArtifactService>, String) {
    configured_host(root, wire, |gateway, _| gateway)
}

fn configured_host(
    root: &Path,
    wire: &MediaWire,
    configure: impl FnOnce(CanvasModelGateway, &str) -> CanvasModelGateway,
) -> (Arc<CanvasModelGateway>, Arc<CanvasArtifactService>, String) {
    let db = Arc::new(Database::open(root.join("speech.db")).unwrap());
    let artifacts =
        Arc::new(CanvasArtifactService::new(root.join("artifacts"), db.clone()).unwrap());
    let providers = Arc::new(CanvasProviderService::new(
        db,
        Arc::new(MemorySecretStore::new()),
    ));
    let saved = providers
        .save_provider(CanvasProviderInput {
            name: "Synthetic H248".into(),
            protocol: "openai".into(),
            base_url: format!("http://{}/v1", wire.address),
            enabled: true,
            api_key: Some("synthetic-h248-key".into()),
            models: vec![CanvasModelConfig {
                id: "synthetic-speech".into(),
                name: "Synthetic voice".into(),
                description: String::new(),
                enabled: true,
                scenarios: vec![CanvasScenario::TextToSpeech],
                priority: 0,
                max_reference_images: None,
            }],
            ..Default::default()
        })
        .unwrap();
    let gateway = Arc::new(configure(
        CanvasModelGateway::new(providers).with_artifacts(artifacts.clone()),
        &saved.id,
    ))
    .scoped_for_run(Some("h248-owner"))
    .unwrap();
    (gateway, artifacts, saved.id)
}

fn request(id: &str, format: &str) -> CanvasAudioRequest {
    CanvasAudioRequest {
        model_ref: format!("{id}::synthetic-speech"),
        operation: "speech_synthesize".into(),
        text: "synthetic speech".into(),
        audio_reference: None,
        voice: Some("synthetic-voice".into()),
        format: Some(format.into()),
        timeout_ms: 5000,
    }
}

fn assert_empty(artifacts: &CanvasArtifactService, root: &Path) {
    assert!(artifacts.list(Some("h248-owner"), None).unwrap().is_empty());
    assert_eq!(
        std::fs::read_dir(root.join("artifacts")).unwrap().count(),
        0
    );
}

async fn refuses_without_eof(body: Vec<u8>, framing: Framing) {
    let mut wire = MediaWire::media(body, framing, "audio/mpeg", true).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, id) = host(root.path(), &wire);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        gateway.run_audio(request(&id, "mp3")),
    )
    .await;
    wire.close().await;
    let error = result
        .expect("speech limit must not wait for EOF")
        .unwrap_err()
        .to_string();
    assert!(error.contains("SpeechArtifactTooLarge"), "{error}");
    assert!(!error.contains("budget"));
    assert!(!error.contains("synthetic-h248"));
    assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    assert_empty(&artifacts, root.path());
}

pub(super) async fn assert_original_oversized_response_refused() {
    refuses_without_eof(
        audio_bytes(LOCAL_SPEECH_LIMIT + 1),
        Framing::Length(LOCAL_SPEECH_LIMIT + 1),
    )
    .await;
}

#[tokio::test]
async fn h248_declared_speech_limit_stops_without_body_or_eof() {
    refuses_without_eof(Vec::new(), Framing::Length(LOCAL_SPEECH_LIMIT + 1)).await;
}

#[tokio::test]
async fn h248_chunked_speech_limit_stops_without_eof() {
    refuses_without_eof(audio_bytes(LOCAL_SPEECH_LIMIT + 1), Framing::Chunked).await;
}

#[tokio::test]
async fn h248_unknown_length_speech_limit_stops_without_eof() {
    refuses_without_eof(audio_bytes(LOCAL_SPEECH_LIMIT + 1), Framing::Close).await;
}

#[tokio::test]
async fn h248_normal_mp3_and_explicit_pcm_keep_bytes_and_namespace() {
    for (bytes, mime, format) in [
        (audio_bytes(64), "audio/mpeg", "mp3"),
        (vec![0, 1, 0, 2, 0, 3], "audio/pcm", "pcm"),
    ] {
        let mut wire =
            MediaWire::media(bytes.clone(), Framing::Length(bytes.len()), mime, false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, id) = host(root.path(), &wire);
        let output = gateway.run_audio(request(&id, format)).await.unwrap();
        wire.close().await;
        let id = output
            .audio_url
            .unwrap()
            .trim_start_matches("artifact://")
            .to_string();
        assert_eq!(
            artifacts
                .read_bytes(&id, Some("h248-owner"))
                .unwrap()
                .unwrap(),
            bytes
        );
        assert!(artifacts.record(&id, None).unwrap().is_none());
        assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn h248_speech_uses_existing_staging_before_eof_then_publishes_once() {
    let bytes = audio_bytes(64);
    let mut wire = MediaWire::media(bytes.clone(), Framing::Chunked, "audio/mpeg", true).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, id) = host(root.path(), &wire);
    let work = gateway.run_audio(request(&id, "mp3"));
    tokio::pin!(work);
    tokio::select! {_=wire.sent.notified()=>{},r=&mut work=>panic!("early response {r:?}")}
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
    let saw_part = tokio::select! {_=staged=>true,_=tokio::time::sleep(Duration::from_secs(2))=>false,
    r=&mut work=>panic!("response before EOF {r:?}")};
    assert!(artifacts.list(Some("h248-owner"), None).unwrap().is_empty());
    wire.release.notify_one();
    let output = tokio::time::timeout(Duration::from_secs(2), &mut work)
        .await
        .unwrap()
        .unwrap();
    wire.close().await;
    assert!(
        saw_part,
        "formal speech path must stream into the original staging before EOF"
    );
    let id = output
        .audio_url
        .unwrap()
        .trim_start_matches("artifact://")
        .to_string();
    assert_eq!(
        artifacts
            .read_bytes(&id, Some("h248-owner"))
            .unwrap()
            .unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn h248_non_audio_success_cannot_become_an_mp3_artifact() {
    let body = b"<html>H248_SYNTHETIC_PRIVATE_BODY</html>".to_vec();
    let mut wire = MediaWire::media(
        body.clone(),
        Framing::Length(body.len()),
        "audio/mpeg",
        false,
    )
    .await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, id) = host(root.path(), &wire);
    let result = gateway.run_audio(request(&id, "mp3")).await;
    wire.close().await;
    assert!(
        result.is_err(),
        "non-audio body must not inherit a requested mp3 label"
    );
    assert!(!result
        .unwrap_err()
        .to_string()
        .contains("H248_SYNTHETIC_PRIVATE_BODY"));
    assert_empty(&artifacts, root.path());
}

#[test]
fn h248_trusted_policy_validates_limits_scope_and_override_precedence() {
    let providers = Arc::new(CanvasProviderService::new(
        Arc::new(Database::open_in_memory().unwrap()),
        Arc::new(MemorySecretStore::new()),
    ));
    for limit in [0, u64::MAX, i64::MAX as u64 + 1] {
        assert!(CanvasModelGateway::new(providers.clone())
            .with_speech_output_limit(limit)
            .is_err());
        assert!(CanvasModelGateway::new(providers.clone())
            .with_speech_output_limit_for("p", "m", None, limit)
            .is_err());
    }
    for (p, m, f) in [
        ("", "m", None),
        ("p ", "m", None),
        ("p", " m", None),
        ("p", "m", Some(" ")),
    ] {
        assert!(CanvasModelGateway::new(providers.clone())
            .with_speech_output_limit_for(p, m, f, 64)
            .is_err());
    }
    let mut candidate = super::tests::video_candidate();
    candidate.provider.id = "p".into();
    candidate.model.id = "m".into();
    let mut request =
        CanvasModelRequest::text("synthetic").with_operation(CanvasOperation::SpeechSynthesize);
    assert_eq!(
        CanvasModelGateway::new(providers.clone())
            .speech_output_policy
            .for_request(&candidate, &request),
        DEFAULT_SPEECH_OUTPUT_LIMIT
    );
    let gateway = Arc::new(
        CanvasModelGateway::new(providers)
            .with_speech_output_limit(10)
            .unwrap()
            .with_speech_output_limit_for("p", "m", None, 20)
            .unwrap()
            .with_speech_output_limit_for("p", "m", Some(" MP3 "), 30)
            .unwrap(),
    );
    let scoped = gateway.scoped_for_run(Some("policy-owner")).unwrap();
    for (p, m, f, expected) in [
        ("p", "m", Some("mp3"), 30),
        ("p", "m", Some("WAV"), 20),
        ("p", "m", None, 20),
        ("other", "m", Some("mp3"), 10),
        ("p", "other", Some("mp3"), 10),
    ] {
        candidate.provider.id = p.into();
        candidate.model.id = m.into();
        request.audio.format = f.map(str::to_string);
        assert_eq!(
            gateway
                .speech_output_policy
                .for_request(&candidate, &request),
            expected
        );
        assert_eq!(
            scoped
                .speech_output_policy
                .for_request(&candidate, &request),
            expected
        );
    }
}

#[tokio::test]
async fn h248_exact_default_and_explicit_larger_limits_preserve_complete_bytes() {
    for size in [64, LOCAL_SPEECH_LIMIT, LOCAL_SPEECH_LIMIT + 1] {
        let bytes = audio_bytes(size);
        let mut wire = MediaWire::media(bytes.clone(), Framing::Chunked, "audio/mpeg", false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, id) = configured_host(root.path(), &wire, |gateway, id| {
            if size == 64 {
                gateway.with_speech_output_limit(64).unwrap()
            } else if size > LOCAL_SPEECH_LIMIT {
                gateway
                    .with_speech_output_limit_for(id, "synthetic-speech", Some("mp3"), size as u64)
                    .unwrap()
            } else {
                gateway
            }
        });
        let output = gateway.run_audio(request(&id, "mp3")).await.unwrap();
        wire.close().await;
        let uri = output.audio_url.unwrap();
        let stored = artifacts
            .record(uri.trim_start_matches("artifact://"), Some("h248-owner"))
            .unwrap()
            .unwrap();
        assert_eq!(stored.byte_size, size as i64);
        assert_eq!(
            artifacts
                .read_bytes(&stored.id, Some("h248-owner"))
                .unwrap()
                .unwrap(),
            bytes
        );
        assert_eq!(artifacts.list(Some("h248-owner"), None).unwrap().len(), 1);
    }
}

#[tokio::test]
async fn h248_public_byte_apis_without_artifact_service_share_the_limit() {
    for (explicit, size) in [(false, 64), (true, 64), (false, 65), (true, 65)] {
        let bytes = audio_bytes(size);
        let mut wire = MediaWire::media(bytes.clone(), Framing::Close, "audio/mpeg", false).await;
        let root = tempfile::tempdir().unwrap();
        let (host, artifacts, id) = host(root.path(), &wire);
        let gateway = CanvasModelGateway::new(host.providers.clone())
            .with_speech_output_limit(64)
            .unwrap();
        let req =
            CanvasModelRequest::text("synthetic").with_operation(CanvasOperation::SpeechSynthesize);
        let result = if explicit {
            gateway.execute_on(&id, "synthetic-speech", &req).await
        } else {
            gateway.execute(&req, None).await
        };
        wire.close().await;
        if size == 64 {
            assert!(
                matches!(result.unwrap(), CanvasModelOutput::Audio { bytes: actual, provider_id, model_id, mime } if actual == bytes && provider_id == id && model_id == "synthetic-speech" && mime == "audio/mpeg")
            );
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code, CanvasErrorCode::ResponseBodyTooLarge);
            assert!(!error.is_transient());
        }
        assert_empty(&artifacts, root.path());
    }
}

#[tokio::test]
async fn h248_container_signatures_win_and_raw_formats_require_explicit_matching_headers() {
    for (bytes, declared, format, expected) in [
        (b"ID3".to_vec(), "text/html", "wav", Some("audio/mpeg")),
        (
            b"RIFF1234WAVEpayload".to_vec(),
            "audio/mpeg",
            "mp3",
            Some("audio/wav"),
        ),
        (
            b"OggSpayload".to_vec(),
            "application/octet-stream",
            "opus",
            Some("audio/ogg"),
        ),
        (
            b"fLaCpayload".to_vec(),
            "audio/mpeg",
            "mp3",
            Some("audio/flac"),
        ),
        (
            b"1234ftypisom1234".to_vec(),
            "audio/mpeg",
            "mp3",
            Some("audio/mp4"),
        ),
        (
            vec![0, 1, 0, 2],
            "application/octet-stream",
            "PCM",
            Some("audio/pcm"),
        ),
        (vec![0, 1, 2, 3], "audio/opus", "opus", Some("audio/opus")),
        (vec![0, 1, 2, 3], "audio/pcm", "mp3", None),
        (b"<html>no audio</html>".to_vec(), "text/html", "pcm", None),
        (
            b"{\"error\":\"synthetic\"}".to_vec(),
            "application/json",
            "opus",
            None,
        ),
        (Vec::new(), "audio/mpeg", "mp3", None),
    ] {
        let mut wire =
            MediaWire::media(bytes.clone(), Framing::Length(bytes.len()), declared, false).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, id) = host(root.path(), &wire);
        let result = gateway.run_audio(request(&id, format)).await;
        wire.close().await;
        if let Some(mime) = expected {
            let uri = result.unwrap().audio_url.unwrap();
            let stored = artifacts
                .record(uri.trim_start_matches("artifact://"), Some("h248-owner"))
                .unwrap()
                .unwrap();
            assert_eq!(stored.media_type.as_deref(), Some(mime));
            assert_eq!(
                artifacts
                    .read_bytes(&stored.id, Some("h248-owner"))
                    .unwrap()
                    .unwrap(),
                bytes
            );
        } else {
            assert!(result.is_err());
            assert_empty(&artifacts, root.path());
        }
    }
}

#[tokio::test]
async fn h248_dropped_and_timed_out_partial_responses_remove_the_original_staging() {
    for deadline in [false, true] {
        let mut wire =
            MediaWire::media(audio_bytes(32), Framing::Length(64), "audio/mpeg", true).await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, id) = host(root.path(), &wire);
        let mut req = request(&id, "mp3");
        req.timeout_ms = 1000;
        {
            let work = gateway.run_audio(req);
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
            tokio::select! {_=staged=>{}, r=&mut work=>panic!("expected partial staging {r:?}"), _=tokio::time::sleep(Duration::from_secs(2))=>panic!("no staging")}
            assert!(artifacts.list(Some("h248-owner"), None).unwrap().is_empty());
            if deadline {
                let error = tokio::time::timeout(Duration::from_secs(2), &mut work)
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("remote outcome unknown") && error.contains("not replayed"),
                    "{error}"
                );
            }
        }
        tokio::time::timeout(Duration::from_secs(2), wire.closed.notified())
            .await
            .unwrap();
        wire.close().await;
        assert_empty(&artifacts, root.path());
        assert_eq!(wire.requests.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn h248_failures_do_not_redispatch_but_small_transient_statuses_still_fallback() {
    use crate::canvas_provider_service::CanvasScenarioBinding;
    for case in [
        "size",
        "format",
        "truncated",
        "staging",
        "index",
        "429",
        "503",
        "candidate-limit",
    ] {
        let fallback = matches!(case, "429" | "503" | "candidate-limit");
        let bytes = if case == "format" {
            b"<html>not speech</html>".to_vec()
        } else {
            audio_bytes(32)
        };
        let framing = Framing::Length(if case == "size" {
            65
        } else if case == "truncated" {
            64
        } else {
            bytes.len()
        });
        let mut first = MediaWire::media_status(
            bytes,
            framing,
            "audio/mpeg",
            false,
            if case == "candidate-limit" {
                503
            } else {
                case.parse().unwrap_or(200)
            },
        )
        .await;
        let second_size = if case == "candidate-limit" { 33 } else { 32 };
        let mut second = MediaWire::media(
            audio_bytes(second_size),
            Framing::Length(second_size),
            "audio/mpeg",
            false,
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let (gateway, artifacts, id) = configured_host(root.path(), &first, |gateway, _| {
            gateway.with_speech_output_limit(64).unwrap()
        });
        let model = gateway
            .providers
            .resolve_model(&id, "synthetic-speech")
            .unwrap()
            .model;
        let healthy = gateway
            .providers
            .save_provider(CanvasProviderInput {
                name: "Synthetic fallback".into(),
                protocol: "openai".into(),
                base_url: format!("http://{}/v1", second.address),
                enabled: true,
                models: vec![model],
                api_key: Some("synthetic-h248-key".into()),
                ..Default::default()
            })
            .unwrap();
        gateway
            .providers
            .save_bindings(
                Some("h248-owner"),
                vec![CanvasScenarioBinding {
                    scenario: "text_to_speech".into(),
                    provider_id: id.clone(),
                    model_id: "synthetic-speech".into(),
                    enabled: true,
                }],
            )
            .unwrap();
        let gateway = Arc::new(
            CanvasModelGateway::new(gateway.providers.clone())
                .with_artifacts(artifacts.clone())
                .with_speech_output_limit(64)
                .unwrap()
                .with_speech_output_limit_for(&healthy.id, "synthetic-speech", None, 32)
                .unwrap(),
        )
        .scoped_for_run(Some("h248-owner"))
        .unwrap();
        if case == "index" {
            Database::open(root.path().join("speech.db")).unwrap().with_conn(|connection| {
                connection.execute_batch("CREATE TRIGGER h248_reject_publication BEFORE INSERT ON artifacts BEGIN SELECT RAISE(ABORT, 'synthetic publication failure'); END;")
                    .map_err(|error| deepagent_core::CoreError::other(error.to_string()))
            }).unwrap();
        } else if case == "staging" {
            std::fs::remove_dir(root.path().join("artifacts")).unwrap();
            std::fs::write(root.path().join("artifacts"), b"synthetic path conflict").unwrap();
        }
        let mut req = request(&id, "mp3");
        req.model_ref.clear();
        let result = gateway.run_audio(req).await;
        first.close().await;
        second.close().await;
        assert_eq!(first.requests.load(Ordering::SeqCst), 1, "{case}");
        assert_eq!(
            second.requests.load(Ordering::SeqCst),
            usize::from(fallback),
            "{case}"
        );
        if fallback && case != "candidate-limit" {
            assert_eq!(result.unwrap().provider_id, healthy.id);
            assert_eq!(artifacts.list(Some("h248-owner"), None).unwrap().len(), 1);
        } else {
            let error = result.unwrap_err().to_string();
            if case == "candidate-limit" {
                assert!(error.contains("SpeechArtifactTooLarge"), "{error}");
            }
            if case == "truncated" {
                assert!(error.contains("remote outcome unknown") && error.contains("not replayed"));
            }
            if case == "staging" {
                std::fs::remove_file(root.path().join("artifacts")).unwrap();
                std::fs::create_dir(root.path().join("artifacts")).unwrap();
            }
            assert_empty(&artifacts, root.path());
        }
    }
}

#[tokio::test]
async fn h248_simultaneous_scopes_keep_completed_output_when_a_sibling_is_dropped() {
    let mut held = MediaWire::media(audio_bytes(32), Framing::Length(64), "audio/mpeg", true).await;
    let mut normal =
        MediaWire::media(audio_bytes(64), Framing::Length(64), "audio/mpeg", false).await;
    let root = tempfile::tempdir().unwrap();
    let (gateway, artifacts, first_id) = host(root.path(), &held);
    let model = gateway
        .providers
        .resolve_model(&first_id, "synthetic-speech")
        .unwrap()
        .model;
    let second = gateway
        .providers
        .save_provider(CanvasProviderInput {
            name: "Synthetic parallel voice".into(),
            protocol: "openai".into(),
            base_url: format!("http://{}/v1", normal.address),
            enabled: true,
            models: vec![model],
            api_key: Some("synthetic-h248-key".into()),
            ..Default::default()
        })
        .unwrap();
    let sibling = gateway.scoped_for_run(Some("other-owner")).unwrap();
    let uri;
    {
        let work = gateway.run_audio(request(&first_id, "mp3"));
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
        tokio::select! {_=staged=>{}, r=&mut work=>panic!("expected partial staging {r:?}"), _=tokio::time::sleep(Duration::from_secs(2))=>panic!("no staging")}
        let completed = tokio::select! {
            r=sibling.run_audio(request(&second.id, "mp3"))=>r.unwrap(),
            r=&mut work=>panic!("held sibling completed {r:?}"),
        };
        uri = completed.audio_url.unwrap();
        assert_eq!(
            std::fs::read_dir(root.path().join("artifacts"))
                .unwrap()
                .count(),
            2
        );
    }
    tokio::time::timeout(Duration::from_secs(2), held.closed.notified())
        .await
        .unwrap();
    held.close().await;
    normal.close().await;
    let id = uri.trim_start_matches("artifact://");
    assert!(artifacts.record(id, Some("h248-owner")).unwrap().is_none());
    assert!(artifacts.list(Some("h248-owner"), None).unwrap().is_empty());
    assert_eq!(
        artifacts
            .read_bytes(id, Some("other-owner"))
            .unwrap()
            .unwrap(),
        audio_bytes(64)
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("artifacts"))
            .unwrap()
            .count(),
        1
    );
}
