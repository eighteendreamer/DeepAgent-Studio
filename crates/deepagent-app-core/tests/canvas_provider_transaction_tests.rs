//! H290 public boundary regressions: synthetic encrypted file SQLite only.
use deepagent_app_core::canvas_provider_service::{
    CanvasModelConfig, CanvasProviderInput, CanvasProviderService, CanvasScenario,
};
use deepagent_app_core::{MemorySecretStore, SecretStore, SqliteSecretStore};
use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::Database;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

const OLD: &str = "h290-synthetic-old";
const NEW: &str = "h290-synthetic-new";
const OLD_URL: &str = "http://old.invalid/v1";
const NEW_URL: &str = "http://new.invalid/v1";

struct FaultSecrets {
    inner: SqliteSecretStore,
    mode: AtomicUsize,
    fail_delete: AtomicBool,
}

impl SecretStore for FaultSecrets {
    fn set(&self, name: &str, key: &str) -> Result<()> {
        let mode = self.mode.swap(0, Ordering::SeqCst);
        if mode == 1 {
            return Err(CoreError::other("synthetic pre-write failure"));
        }
        if mode == 3 {
            return Ok(());
        }
        self.inner.set(name, key)?;
        if mode == 2 {
            return Err(CoreError::other("synthetic post-write failure"));
        }
        Ok(())
    }
    fn get(&self, name: &str) -> Result<Option<String>> {
        self.inner.get(name)
    }
    fn delete(&self, name: &str) -> Result<()> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(CoreError::other("synthetic cleanup failure"));
        }
        self.inner.delete(name)
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    db: Arc<Database>,
    wrapping: Arc<MemorySecretStore>,
    secrets: Arc<FaultSecrets>,
    service: CanvasProviderService,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(directory.path().join("state.sqlite")).unwrap());
        let wrapping = Arc::new(MemorySecretStore::new());
        wrapping
            .set("deepagent_secret_wrap_key", "H290 synthetic wrapping")
            .unwrap();
        let secrets = Arc::new(FaultSecrets {
            inner: SqliteSecretStore::new(db.clone(), wrapping.clone()),
            mode: AtomicUsize::new(0),
            fail_delete: AtomicBool::new(false),
        });
        let service = CanvasProviderService::new(db.clone(), secrets.clone());
        Self {
            directory,
            db,
            wrapping,
            secrets,
            service,
        }
    }
    fn reopen(&self) -> CanvasProviderService {
        let db = Arc::new(Database::open(self.directory.path().join("state.sqlite")).unwrap());
        CanvasProviderService::new(
            db.clone(),
            Arc::new(SqliteSecretStore::new(db, self.wrapping.clone())),
        )
    }
    fn reject_document_writes(&self) {
        self.db
            .with_conn(|connection| {
                connection
                    .execute_batch(
                        "CREATE TRIGGER h290_reject BEFORE UPDATE ON documents
                WHEN NEW.collection='canvas' AND NEW.id='providers'
                BEGIN SELECT RAISE(ABORT, 'synthetic provider document failure'); END;",
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
    }
}

fn input(id: &str, url: &str, key: Option<&str>) -> CanvasProviderInput {
    CanvasProviderInput {
        id: id.into(),
        name: format!("H290 {id}"),
        protocol: "openai".into(),
        base_url: url.into(),
        enabled: true,
        logo: None,
        api_key: key.map(ToOwned::to_owned),
        models: vec![CanvasModelConfig {
            id: "model".into(),
            name: "model".into(),
            description: String::new(),
            enabled: true,
            scenarios: vec![CanvasScenario::Text],
            priority: 0,
            max_reference_images: None,
        }],
    }
}

fn assert_old_pair(service: &CanvasProviderService, version: &str) {
    let resolved = service.resolve_model("first", "model").unwrap();
    assert_eq!(resolved.provider.base_url, OLD_URL);
    assert!(
        resolved.api_key.as_deref() == Some(OLD),
        "credential remains paired with its original endpoint"
    );
    assert_eq!(resolved.provider.credential_version, version);
    let listed = service.list_providers().unwrap();
    assert_eq!(listed[0].base_url, OLD_URL);
    assert!(listed[0].api_key_set);
}

fn secret_failure_preserves_old_pair(mode: usize) {
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    let version = fixture
        .service
        .provider("first")
        .unwrap()
        .credential_version;
    fixture.secrets.mode.store(mode, Ordering::SeqCst);
    assert!(fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .is_err());
    assert_old_pair(&fixture.service, &version);
    assert_old_pair(&fixture.reopen(), &version);
}

#[test]
fn h290_secret_failure_before_write_preserves_old_pair() {
    secret_failure_preserves_old_pair(1);
}

#[test]
fn h290_secret_failure_after_write_preserves_old_pair() {
    secret_failure_preserves_old_pair(2);
}

fn failed_new_provider_is_not_published(mode: usize) {
    let fixture = Fixture::new();
    fixture.secrets.mode.store(mode, Ordering::SeqCst);
    assert!(fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .is_err());
    assert!(fixture.reopen().list_providers().unwrap().is_empty());
    assert!(fixture.reopen().resolve_model("first", "model").is_err());
}

#[test]
fn h290_failed_new_provider_secret_before_write_is_not_published() {
    failed_new_provider_is_not_published(1);
}

#[test]
fn h290_failed_new_provider_secret_after_write_is_not_published() {
    failed_new_provider_is_not_published(2);
}

#[test]
fn h290_document_failure_after_secret_preparation_keeps_old_durable_identity() {
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    let version = fixture
        .service
        .provider("first")
        .unwrap()
        .credential_version;
    fixture.reject_document_writes();
    assert!(fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .is_err());
    assert_old_pair(&fixture.reopen(), &version);
    assert!(fixture.service.set_provider_api_key("first", NEW).is_err());
    assert_old_pair(&fixture.reopen(), &version);
    assert!(fixture.service.clear_provider_api_key("first").is_err());
    assert_old_pair(&fixture.reopen(), &version);
    assert!(fixture.service.remove_provider("first").is_err());
    assert_old_pair(&fixture.reopen(), &version);
}

#[test]
fn h290_normal_no_key_same_key_replacement_clear_and_remove_controls() {
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    let version = fixture
        .service
        .provider("first")
        .unwrap()
        .credential_version;
    fixture
        .service
        .save_provider(input("first", NEW_URL, None))
        .unwrap();
    assert!(
        fixture
            .reopen()
            .provider_api_key("first")
            .unwrap()
            .as_deref()
            == Some(OLD)
    );
    assert_eq!(
        fixture
            .reopen()
            .provider("first")
            .unwrap()
            .credential_version,
        version
    );
    fixture.service.set_provider_api_key("first", OLD).unwrap();
    assert_eq!(
        fixture
            .reopen()
            .provider("first")
            .unwrap()
            .credential_version,
        version
    );
    fixture.service.set_provider_api_key("first", NEW).unwrap();
    assert!(
        fixture
            .reopen()
            .provider_api_key("first")
            .unwrap()
            .as_deref()
            == Some(NEW)
    );
    assert_ne!(
        fixture
            .reopen()
            .provider("first")
            .unwrap()
            .credential_version,
        version
    );
    fixture.service.clear_provider_api_key("first").unwrap();
    assert!(fixture
        .reopen()
        .provider_api_key("first")
        .unwrap()
        .is_none());
    fixture.service.remove_provider("first").unwrap();
    assert!(fixture.reopen().list_providers().unwrap().is_empty());
}

fn secret_count(fixture: &Fixture) -> usize {
    fixture
        .db
        .with_conn(|connection| {
            connection
                .query_row(
                    "SELECT count(*) FROM secret_records WHERE name LIKE 'canvas:%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap()
}

#[test]
fn h290_failed_stages_are_cleaned_and_success_publishes_only_once() {
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    assert_eq!(secret_count(&fixture), 1);
    for mode in [1, 2, 3] {
        fixture.secrets.mode.store(mode, Ordering::SeqCst);
        assert!(fixture
            .service
            .save_provider(input("first", NEW_URL, Some(NEW)))
            .is_err());
        assert_eq!(secret_count(&fixture), 1);
    }
    fixture.reject_document_writes();
    assert!(fixture.service.set_provider_api_key("first", NEW).is_err());
    assert_eq!(secret_count(&fixture), 1);
    fixture
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "DROP TRIGGER h290_reject;
            CREATE TABLE h290_commits (n INTEGER); INSERT INTO h290_commits VALUES (0);
            CREATE TRIGGER h290_count AFTER UPDATE ON documents
            WHEN NEW.collection='canvas' AND NEW.id='providers'
            BEGIN UPDATE h290_commits SET n=n+1; END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .unwrap();
    assert_eq!(secret_count(&fixture), 1);
    let count: usize = fixture
        .db
        .with_conn(|connection| {
            connection
                .query_row("SELECT n FROM h290_commits", [], |row| row.get(0))
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert_eq!(
        count, 1,
        "one configuration/credential generation publication"
    );
    fixture.service.clear_provider_api_key("first").unwrap();
    assert_eq!(secret_count(&fixture), 0);
}

#[test]
fn h290_new_document_insert_failure_discards_stage_without_publishing_a_provider() {
    let fixture = Fixture::new();
    fixture
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h290_reject_new BEFORE INSERT ON documents
            WHEN NEW.collection='canvas' AND NEW.id='providers'
            BEGIN SELECT RAISE(ABORT, 'synthetic first document failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .is_err());
    assert!(fixture.reopen().list_providers().unwrap().is_empty());
    assert_eq!(secret_count(&fixture), 0);
}

#[test]
fn h290_cleanup_failure_cannot_resurrect_a_revoked_or_removed_credential() {
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    fixture.secrets.fail_delete.store(true, Ordering::SeqCst);
    fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .unwrap();
    assert_eq!(
        secret_count(&fixture),
        2,
        "physical retirement failed, not hidden"
    );
    assert!(
        fixture
            .reopen()
            .provider_api_key("first")
            .unwrap()
            .as_deref()
            == Some(NEW)
    );
    fixture.service.clear_provider_api_key("first").unwrap();
    assert!(fixture
        .reopen()
        .provider_api_key("first")
        .unwrap()
        .is_none());
    fixture.service.remove_provider("first").unwrap();
    fixture
        .service
        .save_provider(input("first", OLD_URL, None))
        .unwrap();
    assert!(fixture
        .reopen()
        .provider_api_key("first")
        .unwrap()
        .is_none());
    assert!(!fixture.reopen().list_providers().unwrap()[0].api_key_set);
}

fn alter_document(fixture: &Fixture, edit: impl FnOnce(&mut serde_json::Value)) {
    use deepagent_persistence::document_store::DocumentStore;
    let store = DocumentStore::new(&fixture.db);
    let mut doc = store.get("canvas", "providers").unwrap().unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&doc.body).unwrap();
    edit(&mut value);
    doc.body = value.to_string();
    store
        .put("canvas", "providers", &doc.body, None, doc.updated_at)
        .unwrap();
}

#[test]
fn h290_legacy_document_uses_legacy_slot_only_until_successful_reference_publication() {
    use deepagent_app_core::canvas_provider_service::provider_secret_key;
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, None))
        .unwrap();
    fixture
        .secrets
        .inner
        .set(&provider_secret_key("first"), OLD)
        .unwrap();
    alter_document(&fixture, |value| {
        value["providers"][0]
            .as_object_mut()
            .unwrap()
            .remove("credential_ref");
    });
    let version = fixture
        .reopen()
        .provider("first")
        .unwrap()
        .credential_version;
    assert_old_pair(&fixture.reopen(), &version);
    fixture.secrets.mode.store(2, Ordering::SeqCst);
    assert!(fixture
        .service
        .save_provider(input("first", NEW_URL, None))
        .is_err());
    assert_old_pair(&fixture.reopen(), &version);
    fixture
        .service
        .save_provider(input("first", NEW_URL, None))
        .unwrap();
    assert!(fixture
        .secrets
        .inner
        .get(&provider_secret_key("first"))
        .unwrap()
        .is_none());
    assert!(
        fixture
            .reopen()
            .provider_api_key("first")
            .unwrap()
            .as_deref()
            == Some(OLD)
    );
    assert_eq!(
        fixture
            .reopen()
            .provider("first")
            .unwrap()
            .credential_version,
        version
    );
    assert_eq!(secret_count(&fixture), 1);
}

#[test]
fn h290_missing_or_corrupt_reference_is_an_error_not_a_legacy_fallback() {
    use deepagent_app_core::canvas_provider_service::provider_secret_key;
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(NEW)))
        .unwrap();
    fixture
        .secrets
        .inner
        .set(&provider_secret_key("first"), OLD)
        .unwrap();
    fixture
        .db
        .with_conn(|connection| {
            connection
                .execute(
                    "DELETE FROM secret_records WHERE name LIKE 'canvas:credential:v1:%'",
                    [],
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
    assert!(fixture.reopen().resolve_model("first", "model").is_err());
    assert!(fixture.reopen().list_providers().is_err());
    assert!(fixture
        .reopen()
        .candidates_for_scenario(CanvasScenario::Text, None)
        .is_err());
    assert!(fixture.reopen().provider_credentials("first").is_err());
    for reference in [
        serde_json::json!({"kind":"secret", "id":"invalid"}),
        serde_json::json!({"kind":"H290_PRIVATE_MARKER"}),
    ] {
        alter_document(&fixture, |value| {
            value["providers"][0]["credential_ref"] = reference;
        });
        let error = fixture
            .reopen()
            .resolve_model("first", "model")
            .unwrap_err();
        assert!(!error.to_string().contains("H290_PRIVATE_MARKER"));
    }
}

#[test]
fn h290_conflicting_noncooperating_document_edit_is_not_overwritten_by_prepared_secret() {
    struct Conflict {
        inner: Arc<FaultSecrets>,
        db: Arc<Database>,
    }
    impl SecretStore for Conflict {
        fn set(&self, name: &str, key: &str) -> Result<()> {
            self.inner.set(name, key)?;
            self.db.with_conn(|connection| {
                connection.execute("UPDATE documents SET body=json_set(body, '$.providers[0].name', 'independent edit') WHERE collection='canvas' AND id='providers'", [])
                    .map_err(|error| CoreError::Persistence(error.to_string()))?;
                Ok(())
            })
        }
        fn get(&self, name: &str) -> Result<Option<String>> {
            self.inner.get(name)
        }
        fn delete(&self, name: &str) -> Result<()> {
            self.inner.delete(name)
        }
    }
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    let version = fixture
        .reopen()
        .provider("first")
        .unwrap()
        .credential_version;
    let service = CanvasProviderService::new(
        fixture.db.clone(),
        Arc::new(Conflict {
            inner: fixture.secrets.clone(),
            db: fixture.db.clone(),
        }),
    );
    assert!(service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .is_err());
    assert_old_pair(&fixture.reopen(), &version);
    assert_eq!(
        fixture.reopen().provider("first").unwrap().name,
        "independent edit"
    );
    assert_eq!(secret_count(&fixture), 1);
}

#[path = "canvas_provider_transaction/process.rs"]
mod process;

#[path = "canvas_provider_transaction/wire.rs"]
mod wire;

#[test]
fn h290_gate_io_failure_is_reported_without_mutating_provider_or_secret() {
    let fixture = Fixture::new();
    let path = fixture.db.data_root().join("canvas-provider-commit.lock");
    std::fs::create_dir(&path).unwrap();
    let error = fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("acquire canvas provider commit gate"));
    assert_eq!(secret_count(&fixture), 0);
    assert!(fixture.service.list_providers().is_err());
    std::fs::remove_dir(path).unwrap();
    assert!(fixture.service.list_providers().unwrap().is_empty());
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
}

#[test]
fn h290_contended_gate_has_a_bounded_error_and_no_partial_side_effects() {
    use deepagent_persistence::fs_commit::lock_operation_file;
    use std::time::{Duration, Instant};
    let fixture = Fixture::new();
    let guard =
        lock_operation_file(&fixture.db.data_root().join("canvas-provider-commit.lock")).unwrap();
    let started = Instant::now();
    let error = fixture
        .service
        .save_provider(input("first", NEW_URL, Some(NEW)))
        .unwrap_err();
    assert!(error.to_string().contains("file operation is busy"));
    assert!(started.elapsed() >= Duration::from_secs(5));
    assert!(started.elapsed() < Duration::from_secs(15));
    assert_eq!(secret_count(&fixture), 0);
    drop(guard);
    assert!(fixture.service.list_providers().unwrap().is_empty());
}

#[test]
fn h290_reader_snapshot_finishes_before_a_writer_can_retire_its_credential() {
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;
    struct PausedRead {
        inner: Arc<FaultSecrets>,
        armed: AtomicBool,
        ready: mpsc::Sender<()>,
        resume: Mutex<mpsc::Receiver<()>>,
    }
    impl SecretStore for PausedRead {
        fn set(&self, name: &str, key: &str) -> Result<()> {
            self.inner.set(name, key)
        }
        fn delete(&self, name: &str) -> Result<()> {
            self.inner.delete(name)
        }
        fn get(&self, name: &str) -> Result<Option<String>> {
            if self.armed.swap(false, Ordering::SeqCst) {
                self.ready.send(()).unwrap();
                self.resume
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
            }
            self.inner.get(name)
        }
    }
    let fixture = Fixture::new();
    fixture
        .service
        .save_provider(input("first", OLD_URL, Some(OLD)))
        .unwrap();
    let (ready, began) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let reader = CanvasProviderService::new(
        fixture.db.clone(),
        Arc::new(PausedRead {
            inner: fixture.secrets.clone(),
            armed: AtomicBool::new(true),
            ready,
            resume: Mutex::new(wait),
        }),
    );
    let read = std::thread::spawn(move || reader.provider_credentials("first"));
    began.recv_timeout(Duration::from_secs(10)).unwrap();
    let writer = fixture.reopen();
    let (completed, completion) = mpsc::channel();
    let write = std::thread::spawn(move || {
        let result = writer.save_provider(input("first", NEW_URL, Some(NEW)));
        completed.send(()).unwrap();
        result
    });
    let blocked = completion.recv_timeout(Duration::from_millis(150)).is_err();
    resume.send(()).unwrap();
    let (provider, key) = read.join().unwrap().unwrap();
    write.join().unwrap().unwrap();
    assert!(
        blocked,
        "writer cannot publish or retire while reader resolves its pair"
    );
    assert_eq!(provider.base_url, OLD_URL);
    assert!(key.as_deref() == Some(OLD));
    let (provider, key) = fixture.reopen().provider_credentials("first").unwrap();
    assert_eq!(provider.base_url, NEW_URL);
    assert!(key.as_deref() == Some(NEW));
}
