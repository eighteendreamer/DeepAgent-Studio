//! H292: current declarations, not saved priority rows, authorize selection.
use super::*;
use deepagent_app_core::canvas_model_gateway::{
    CanvasModelOutput, CanvasModelRequest, CanvasOperation,
};
use deepagent_runtime::workflow::{CanvasVideoPreparation, CanvasVideoRequest};

const SCENARIOS: [CanvasScenario; 6] = [
    CanvasScenario::Text,
    CanvasScenario::ImageGeneration,
    CanvasScenario::VideoGeneration,
    CanvasScenario::SpeechToText,
    CanvasScenario::TextToSpeech,
    CanvasScenario::Embedding,
];

fn bind(scenario: CanvasScenario, provider: &str, model: &str) -> CanvasScenarioBinding {
    CanvasScenarioBinding {
        scenario: scenario.as_str().into(),
        provider_id: provider.into(),
        model_id: model.into(),
        enabled: true,
    }
}

async fn dispatch(
    gateway: &CanvasModelGateway,
    scenario: CanvasScenario,
    explicit: Option<&str>,
) -> Result<(), String> {
    let work = async {
        if scenario == CanvasScenario::VideoGeneration {
            let result = gateway
                .prepare_video(CanvasVideoRequest {
                    model_ref: explicit
                        .map(|id| format!("{PROVIDER}::{id}"))
                        .unwrap_or_default(),
                    prompt: "synthetic video".into(),
                    image_url: None,
                    size: None,
                    resume_task_id: None,
                    resume_fingerprint: None,
                    timeout_ms: 3000,
                    cancel: None,
                    on_progress: None,
                })
                .await
                .map_err(|error| error.to_string())?;
            assert!(matches!(result, CanvasVideoPreparation::Deferred { .. }));
            return Ok(());
        }
        let operation = match scenario {
            CanvasScenario::Text => CanvasOperation::TextGenerate,
            CanvasScenario::ImageGeneration => CanvasOperation::ImageGenerate,
            CanvasScenario::SpeechToText => CanvasOperation::SpeechTranscribe,
            CanvasScenario::TextToSpeech => CanvasOperation::SpeechSynthesize,
            CanvasScenario::Embedding => CanvasOperation::Embedding,
            CanvasScenario::VideoGeneration => unreachable!(),
        };
        let mut request = CanvasModelRequest::text("synthetic scenario").with_operation(operation);
        request.texts = vec!["synthetic embedding".into()];
        request.audio.reference = Some("data:audio/mpeg;base64,SUQzaDI5Mg==".into());
        request.audio.format = Some("mp3".into());
        let output = match explicit {
            Some(id) => gateway.execute_on(PROVIDER, id, &request).await,
            None => gateway.execute(&request, Some("h292-workspace")).await,
        }
        .map_err(|error| error.to_string())?;
        match output {
            CanvasModelOutput::Text { text, .. } => assert!(!text.is_empty()),
            CanvasModelOutput::Image { bytes, .. } => assert!(bytes.starts_with(b"\x89PNG")),
            CanvasModelOutput::Audio { bytes, .. } => assert!(bytes.starts_with(b"ID3")),
            CanvasModelOutput::Embedding { dimensions, .. } => assert_eq!(dimensions, 2),
            other => panic!("unexpected output {other:?}"),
        }
        Ok(())
    };
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .expect("bounded real dispatch")
}

#[tokio::test]
async fn h292_all_scenarios_revoke_restore_and_explicit_selection_use_current_declarations() {
    let mut failures = Vec::new();
    for scenario in SCENARIOS {
        for provider_update in [false, true] {
            let mut wire = Fixture::start().await;
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("state.sqlite");
            let secrets = Arc::new(MemorySecretStore::new());
            let service = Arc::new(CanvasProviderService::new(
                Arc::new(Database::open(&path).unwrap()),
                secrets.clone(),
            ));
            let mut input = provider_input(&wire.base_url);
            input.models = vec![
                model(FORMER, SCENARIOS.to_vec()),
                model(HEALTHY, SCENARIOS.to_vec()),
            ];
            service.save_provider(input.clone()).unwrap();
            // Public byte APIs accept a scope; the public video bridge uses global here.
            service
                .save_bindings(
                    None,
                    vec![
                        bind(scenario, PROVIDER, FORMER),
                        bind(scenario, PROVIDER, HEALTHY),
                    ],
                )
                .unwrap();
            let bindings = vec![
                bind(scenario, PROVIDER, FORMER),
                bind(scenario, PROVIDER, HEALTHY),
            ];
            service
                .save_bindings(Some("h292-workspace"), bindings.clone())
                .unwrap();
            let gateway = CanvasModelGateway::new(service.clone());
            dispatch(&gateway, scenario, None).await.unwrap();
            let before = wire.take_seen();
            let remaining = if provider_update {
                SCENARIOS
                    .iter()
                    .copied()
                    .filter(|value| *value != scenario)
                    .collect()
            } else {
                Vec::new()
            };
            input.models[0].scenarios = remaining;
            if provider_update {
                input.api_key = None;
                service.save_provider(input).unwrap();
            } else {
                service
                    .save_model(PROVIDER, input.models[0].clone())
                    .unwrap();
            }
            drop(gateway);
            drop(service);
            let service = Arc::new(CanvasProviderService::new(
                Arc::new(Database::open(&path).unwrap()),
                secrets,
            ));
            assert_eq!(service.bindings(Some("h292-workspace")).unwrap(), bindings);
            assert!(service
                .save_bindings(Some("h292-workspace"), bindings)
                .is_err());
            let candidates = service
                .candidates_for_scenario(scenario, Some("h292-workspace"))
                .unwrap();
            let gateway = CanvasModelGateway::new(service.clone());
            let automatic = dispatch(&gateway, scenario, None).await;
            let after = wire.take_seen();
            let explicit = dispatch(&gateway, scenario, Some(FORMER)).await;
            let explicit_seen = wire.take_seen();
            service
                .save_model(PROVIDER, model(FORMER, SCENARIOS.to_vec()))
                .unwrap();
            dispatch(&gateway, scenario, Some(FORMER)).await.unwrap();
            let restored = wire.take_seen();
            wire.stop().await;
            if before.is_empty()
                || before.iter().any(|(_, id)| id != FORMER)
                || candidates
                    .iter()
                    .any(|candidate| candidate.model.id == FORMER)
                || automatic.is_err()
                || after.is_empty()
                || after.iter().any(|(_, id)| id != HEALTHY)
                || !explicit
                    .as_ref()
                    .is_err_and(|error| error.contains("not declared for scenario"))
                || !explicit_seen.is_empty()
                || restored.is_empty()
                || restored.iter().any(|(_, id)| id != FORMER)
            {
                failures.push(format!("{scenario:?}/provider_update={provider_update}: before={before:?} after={after:?} explicit={explicit:?} wire={explicit_seen:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn h292_global_workspace_alias_order_dedup_and_legacy_rows_share_one_eligibility_rule() {
    use deepagent_core::clock::Timestamp;
    use deepagent_persistence::document_store::DocumentStore;
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("state.sqlite")).unwrap());
    let service = CanvasProviderService::new(db.clone(), Arc::new(MemorySecretStore::new()));
    let mut input = provider_input("http://synthetic.invalid/v1");
    input.models = vec![
        model(FORMER, SCENARIOS.to_vec()),
        model(HEALTHY, SCENARIOS.to_vec()),
    ];
    service.save_provider(input.clone()).unwrap();
    let mut other = input.clone();
    other.id = "other-provider".into();
    other.name = "Other".into();
    other.models[0].priority = 50;
    service.save_provider(other).unwrap();
    for (scenario, alias) in
        SCENARIOS
            .into_iter()
            .zip(["text", "image-gen", "video", "stt", "tts", "vector"])
    {
        for scope in [None, Some("workspace")] {
            let mut first = bind(scenario, PROVIDER, FORMER);
            first.scenario = alias.into();
            let bindings = vec![
                first.clone(),
                first,
                bind(scenario, "other-provider", FORMER),
                bind(scenario, PROVIDER, HEALTHY),
            ];
            service.save_bindings(scope, bindings.clone()).unwrap();
            service
                .save_model(PROVIDER, model(FORMER, Vec::new()))
                .unwrap();
            // Simulate persisted old rows, including removed/unknown/disabled priorities.
            let mut legacy = bindings.clone();
            legacy.insert(0, bind(scenario, "missing-provider", "missing-model"));
            let mut unknown = bind(scenario, PROVIDER, FORMER);
            unknown.scenario = "unknown-old-scenario".into();
            legacy.push(unknown);
            DocumentStore::new(&db)
                .put(
                    "canvas",
                    &deepagent_app_core::canvas_provider_service::binding_doc_id(scope),
                    &json!({"bindings":legacy}).to_string(),
                    None,
                    Timestamp::EPOCH,
                )
                .unwrap();
            let actual = service.candidates_for_scenario(scenario, scope).unwrap();
            let identities: Vec<_> = actual
                .iter()
                .map(|candidate| (candidate.provider.id.as_str(), candidate.model.id.as_str()))
                .collect();
            assert_eq!(
                identities,
                vec![
                    ("other-provider", FORMER),
                    (PROVIDER, HEALTHY),
                    ("other-provider", HEALTHY)
                ],
                "{scenario:?}/{scope:?}"
            );
            assert_eq!(
                service.bindings(scope).unwrap(),
                legacy,
                "selection must not erase saved preferences"
            );
            service
                .save_model(PROVIDER, model(FORMER, SCENARIOS.to_vec()))
                .unwrap();
            let mut disabled = model(FORMER, SCENARIOS.to_vec());
            disabled.enabled = false;
            service.save_model(PROVIDER, disabled).unwrap();
            assert!(
                !service
                    .candidates_for_scenario(scenario, scope)
                    .unwrap()
                    .iter()
                    .any(|candidate| candidate.provider.id == PROVIDER
                        && candidate.model.id == FORMER)
            );
            service.remove_model(PROVIDER, FORMER).unwrap();
            assert!(
                !service
                    .candidates_for_scenario(scenario, scope)
                    .unwrap()
                    .iter()
                    .any(|candidate| candidate.provider.id == PROVIDER
                        && candidate.model.id == FORMER)
            );
            service
                .save_model(PROVIDER, model(FORMER, SCENARIOS.to_vec()))
                .unwrap();
            let mut disabled_provider = input.clone();
            disabled_provider.enabled = false;
            disabled_provider.api_key = None;
            service.save_provider(disabled_provider).unwrap();
            assert!(service
                .candidates_for_scenario(scenario, scope)
                .unwrap()
                .iter()
                .all(|candidate| candidate.provider.id == "other-provider"));
            let mut restored = input.clone();
            restored.api_key = None;
            service.save_provider(restored).unwrap();
        }
    }
}

#[tokio::test]
async fn h292_prepared_http_call_keeps_snapshot_but_new_calls_observe_committed_revocation() {
    let mut wire = Fixture::start().await;
    let root = tempfile::tempdir().unwrap();
    let service = Arc::new(CanvasProviderService::new(
        Arc::new(Database::open(root.path().join("state.sqlite")).unwrap()),
        Arc::new(MemorySecretStore::new()),
    ));
    service
        .save_provider(provider_input(&wire.base_url))
        .unwrap();
    service.save_bindings(None, bindings()).unwrap();
    wire.hold.store(true, Ordering::SeqCst);
    let copy = service.clone();
    let call = tokio::spawn(async move { complete(copy).await });
    tokio::time::timeout(Duration::from_secs(2), wire.arrived.notified())
        .await
        .unwrap();
    // The model call must not hold the configuration gate over its HTTP await.
    let update = service.clone();
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::task::spawn_blocking(move || update.save_model(PROVIDER, model(FORMER, Vec::new()))),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    wire.release.notify_one();
    assert_eq!(call.await.unwrap().as_deref(), Ok(FORMER));
    assert_eq!(wire.take_seen()[0].1, FORMER);
    let next = complete(service).await;
    let seen = wire.take_seen();
    wire.stop().await;
    assert_eq!(next.as_deref(), Ok(HEALTHY));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, HEALTHY);
}

#[tokio::test]
async fn h292_qualified_400_still_stops_and_no_eligible_candidate_makes_no_request() {
    let mut wire = Fixture::start().await;
    let root = tempfile::tempdir().unwrap();
    let service = Arc::new(CanvasProviderService::new(
        Arc::new(Database::open(root.path().join("state.sqlite")).unwrap()),
        Arc::new(MemorySecretStore::new()),
    ));
    service
        .save_provider(provider_input(&wire.base_url))
        .unwrap();
    service.save_bindings(None, bindings()).unwrap();
    wire.reject_former.store(true, Ordering::SeqCst);
    let rejected = complete(service.clone()).await;
    let seen = wire.take_seen();
    service
        .save_model(PROVIDER, model(FORMER, Vec::new()))
        .unwrap();
    service
        .save_model(PROVIDER, model(HEALTHY, vec![CanvasScenario::Embedding]))
        .unwrap();
    let unavailable = complete(service).await;
    let unavailable_wire = wire.take_seen();
    wire.stop().await;
    assert!(rejected.unwrap_err().contains("HTTP 400"));
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].1, FORMER);
    assert!(unavailable.unwrap_err().contains("NoCandidateModel"));
    assert!(unavailable_wire.is_empty());
}

/// Pauses a real credential read inside the existing provider gate. No native
/// credentials are touched; bounded waits also release workers on assertion failure.
struct PausedSecrets {
    inner: MemorySecretStore,
    armed: AtomicBool,
    entered: std::sync::mpsc::Sender<()>,
    release: (Mutex<bool>, std::sync::Condvar),
}

impl deepagent_app_core::SecretStore for PausedSecrets {
    fn set(&self, name: &str, value: &str) -> deepagent_core::Result<()> {
        self.inner.set(name, value)
    }
    fn get(&self, name: &str) -> deepagent_core::Result<Option<String>> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.entered.send(()).unwrap();
            let (lock, changed) = &self.release;
            let (released, waited) = changed
                .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |ready| {
                    !*ready
                })
                .unwrap();
            if waited.timed_out() && !*released {
                return Err(deepagent_core::CoreError::other(
                    "synthetic snapshot barrier timed out",
                ));
            }
        }
        self.inner.get(name)
    }
    fn delete(&self, name: &str) -> deepagent_core::Result<()> {
        self.inner.delete(name)
    }
}

#[test]
fn h292_binding_writers_join_the_existing_snapshot_gate_across_database_handles() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let (entered, observed) = std::sync::mpsc::channel();
    let secrets = Arc::new(PausedSecrets {
        inner: MemorySecretStore::new(),
        armed: AtomicBool::new(false),
        entered,
        release: (Mutex::new(false), std::sync::Condvar::new()),
    });
    let reader = Arc::new(CanvasProviderService::new(
        Arc::new(Database::open(&path).unwrap()),
        secrets.clone(),
    ));
    reader
        .save_provider(provider_input("http://synthetic.invalid/v1"))
        .unwrap();
    reader.save_bindings(None, bindings()).unwrap();
    let writer =
        CanvasProviderService::new(Arc::new(Database::open(&path).unwrap()), secrets.clone());
    secrets.armed.store(true, Ordering::SeqCst);
    let read = reader.clone();
    let prepared =
        std::thread::spawn(move || read.candidates_for_scenario(CanvasScenario::Text, None));
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let saved = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result =
            writer.save_bindings(None, vec![bind(CanvasScenario::Text, PROVIDER, HEALTHY)]);
        done_tx.send(result).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let blocked = matches!(
        done_rx.recv_timeout(Duration::from_millis(100)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );
    *secrets.release.0.lock().unwrap() = true;
    secrets.release.1.notify_all();
    let snapshot = prepared.join().unwrap().unwrap();
    saved.join().unwrap();
    if blocked {
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
    }
    assert!(
        blocked,
        "binding publication must serialize with a prepared provider snapshot"
    );
    assert_eq!(snapshot[0].model.id, FORMER);
    assert_eq!(
        reader
            .candidates_for_scenario(CanvasScenario::Text, None)
            .unwrap()[0]
            .model
            .id,
        HEALTHY
    );
}

#[test]
fn h292_revoked_declaration_is_checked_before_materializing_a_credential() {
    struct CountedSecrets {
        inner: MemorySecretStore,
        reads: std::sync::atomic::AtomicUsize,
        fail: AtomicBool,
    }
    impl deepagent_app_core::SecretStore for CountedSecrets {
        fn set(&self, name: &str, value: &str) -> deepagent_core::Result<()> {
            self.inner.set(name, value)
        }
        fn get(&self, name: &str) -> deepagent_core::Result<Option<String>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                Err(deepagent_core::CoreError::other(
                    "synthetic credential unavailable",
                ))
            } else {
                self.inner.get(name)
            }
        }
        fn delete(&self, name: &str) -> deepagent_core::Result<()> {
            self.inner.delete(name)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let secrets = Arc::new(CountedSecrets {
        inner: MemorySecretStore::new(),
        reads: std::sync::atomic::AtomicUsize::new(0),
        fail: AtomicBool::new(false),
    });
    let service = CanvasProviderService::new(
        Arc::new(Database::open(root.path().join("state.sqlite")).unwrap()),
        secrets.clone(),
    );
    let mut input = provider_input("http://synthetic.invalid/v1");
    input.models = vec![model(FORMER, Vec::new())];
    service.save_provider(input).unwrap();
    secrets.reads.store(0, Ordering::SeqCst);
    secrets.fail.store(true, Ordering::SeqCst);
    let error = service
        .resolve_model_for_scenario(PROVIDER, FORMER, CanvasScenario::Text)
        .unwrap_err();
    assert!(error.to_string().contains("not declared for scenario"));
    assert!(service
        .candidates_for_scenario(CanvasScenario::Text, None)
        .unwrap()
        .is_empty());
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 0);
    assert!(
        service.resolve_model(PROVIDER, FORMER).is_err(),
        "diagnostic identity resolution still reads credentials"
    );
    assert_eq!(secrets.reads.load(Ordering::SeqCst), 1);
}
