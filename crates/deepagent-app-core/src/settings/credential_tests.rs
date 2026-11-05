//! H88: late discovery must have no credential publication authority.
//! Temporary file SQLite, synthetic wrapping secret, no keychain/network.
use super::*;
use crate::{MemorySecretStore, SqliteSecretStore};
use async_trait::async_trait;
use deepagent_models::transport::{EventSink, MockTransport, TransportRequest};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

const FLASH: &str =
    r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#;
const PRO: &str = r#"{"data":[{"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}]}"#;
const OLD: &str = "h88-synthetic-old";
const NEW: &str = "h88-synthetic-new";

struct Discovery {
    armed: AtomicBool,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl HttpTransport for Discovery {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other("unexpected stream in credential fixture"))
    }

    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(FLASH.to_owned())
    }
}

struct Pair {
    first: Arc<SettingsService>,
    second: Arc<SettingsService>,
    discovery: Arc<Discovery>,
    wrapping: Arc<MemorySecretStore>,
    directory: tempfile::TempDir,
}

impl Pair {
    async fn new(initialized: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.sqlite");
        let first_db = Arc::new(Database::open(&path).unwrap());
        let wrapping = Arc::new(MemorySecretStore::new());
        let discovery = Arc::new(Discovery {
            armed: AtomicBool::new(false),
            started: Notify::new(),
            release: Notify::new(),
        });
        let first = Arc::new(SettingsService::new(
            first_db.clone(),
            discovery.clone(),
            Arc::new(SqliteSecretStore::new(first_db, wrapping.clone())),
        ));
        let second_db = Arc::new(Database::open(&path).unwrap());
        let second = Arc::new(SettingsService::new(
            second_db.clone(),
            Arc::new(MockTransport::with_get_json(PRO)),
            Arc::new(SqliteSecretStore::new(second_db, wrapping.clone())),
        ));
        if initialized {
            first.initialize(OLD).await.unwrap();
        }
        Self {
            first,
            second,
            discovery,
            wrapping,
            directory,
        }
    }

    fn reopen(&self) -> SettingsService {
        let db = Arc::new(Database::open(self.directory.path().join("settings.sqlite")).unwrap());
        SettingsService::new(
            db.clone(),
            Arc::new(MockTransport::with_get_json(PRO)),
            Arc::new(SqliteSecretStore::new(db, self.wrapping.clone())),
        )
    }

    async fn pending(&self, key: Option<&str>) -> Pending {
        self.discovery.armed.store(true, Ordering::SeqCst);
        let service = self.first.clone();
        let key = key.map(ToOwned::to_owned);
        let task = tokio::spawn(async move {
            match key {
                Some(key) => service.initialize(&key).await,
                None => service.refresh_models().await,
            }
        });
        let pending = Pending {
            task,
            discovery: self.discovery.clone(),
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.discovery.started.notified(),
        )
        .await
        .expect("discovery must reach the barrier");
        pending
    }
}

struct Pending {
    task: tokio::task::JoinHandle<Result<SettingsView>>,
    discovery: Arc<Discovery>,
}

impl Pending {
    async fn finish(mut self) -> Result<SettingsView> {
        self.discovery.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut self.task)
            .await
            .expect("late discovery must settle")
            .unwrap()
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.task.abort();
        self.discovery.release.notify_one();
    }
}

#[tokio::test]
async fn h88_same_value_relogin_rejects_old_refresh_without_replacing_new_catalog() {
    let pair = Pair::new(true).await;
    let pending = pair.pending(None).await;
    pair.second.clear_key().unwrap();
    pair.second.initialize(OLD).await.unwrap();
    assert_eq!(
        pair.second.load().unwrap().unwrap().catalog.chat_model,
        "deepseek-v4-pro"
    );
    assert!(
        pending.finish().await.is_err(),
        "same key value does not restore old authority"
    );
    let reopened = pair.reopen();
    assert_eq!(reopened.api_key().unwrap().as_deref(), Some(OLD));
    assert_eq!(
        reopened.load().unwrap().unwrap().catalog.chat_model,
        "deepseek-v4-pro"
    );
}

#[tokio::test]
async fn h88_latest_started_initialization_owns_commit_even_when_older_finishes_last() {
    let pair = Pair::new(true).await;
    let pending = pair.pending(Some("h88-older-attempt")).await;
    pair.second.initialize(NEW).await.unwrap();
    assert!(pending.finish().await.is_err());
    let reopened = pair.reopen();
    assert_eq!(reopened.api_key().unwrap().as_deref(), Some(NEW));
    assert_eq!(
        reopened.load().unwrap().unwrap().catalog.chat_model,
        "deepseek-v4-pro"
    );
}

#[tokio::test]
async fn h88_logout_revokes_initialization_before_any_settings_or_key_exists() {
    let pair = Pair::new(false).await;
    let pending = pair.pending(Some(OLD)).await;
    pair.second.clear_key().unwrap();
    assert!(pending.finish().await.is_err());
    let reopened = pair.reopen();
    assert!(reopened.api_key().unwrap().is_none());
    assert!(reopened.load().unwrap().is_none());
}

#[tokio::test]
async fn h88_failed_new_attempt_revokes_older_attempt_without_deleting_active_key() {
    let pair = Pair::new(true).await;
    let pending = pair.pending(Some("h88-older-attempt")).await;
    let service = SettingsService::new(
        pair.second.db.clone(),
        Arc::new(MockTransport::default()),
        pair.second.secrets.clone(),
    );
    assert!(service.initialize(NEW).await.is_err());
    assert_eq!(service.api_key().unwrap().as_deref(), Some(OLD));
    assert!(pending.finish().await.is_err());
    assert_eq!(pair.reopen().api_key().unwrap().as_deref(), Some(OLD));
}

#[tokio::test]
async fn h88_refresh_never_rewrites_the_persisted_credential_record() {
    let pair = Pair::new(true).await;
    let stored = || {
        pair.first
            .db
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT ciphertext, nonce, updated_at FROM secret_records WHERE name=?1",
                        [API_KEY_NAME],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>(0)?,
                                row.get::<_, Vec<u8>>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap()
    };
    let before = stored();
    pair.first.refresh_models().await.unwrap();
    assert!(
        before == stored(),
        "refresh must not write or re-encrypt its captured key"
    );
}

#[tokio::test]
async fn h88_dropped_new_attempt_does_not_restore_older_publication_authority() {
    let pair = Pair::new(true).await;
    let older = pair.pending(Some("h88-older-attempt")).await;
    let newer = pair.pending(Some(NEW)).await;
    drop(newer);
    assert!(older.finish().await.is_err());
    assert_eq!(pair.reopen().api_key().unwrap().as_deref(), Some(OLD));
}

#[tokio::test]
async fn h88_public_settings_failure_persists_recovery_required_across_reopen() {
    let pair = Pair::new(true).await;
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h88_reject_settings BEFORE UPDATE ON documents
         WHEN NEW.collection='settings' AND NEW.id='app'
         BEGIN SELECT RAISE(ABORT, 'synthetic public settings failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(pair.second.initialize(NEW).await.is_err());
    assert_eq!(
        pair.first.secrets.get(API_KEY_NAME).unwrap().as_deref(),
        Some(NEW)
    );
    let reopened = pair.reopen();
    assert!(reopened.api_key().is_err());
    assert!(reopened.view().is_err());
    assert!(reopened.refresh_models().await.is_err());
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h88_reject_settings")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    reopened.clear_key().unwrap();
    assert!(reopened.api_key().unwrap().is_none());
    reopened.initialize(NEW).await.unwrap();
    assert_eq!(pair.first.api_key().unwrap().as_deref(), Some(NEW));
}

#[tokio::test]
async fn h88_final_ready_write_failure_cannot_report_activation_success() {
    let pair = Pair::new(true).await;
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    r#"CREATE TRIGGER h88_reject_ready BEFORE UPDATE ON documents
         WHEN NEW.collection='settings' AND NEW.id='credential_lifecycle'
          AND NEW.body LIKE '%"phase":"ready"%' AND OLD.body LIKE '%"phase":"committing"%'
         BEGIN SELECT RAISE(ABORT, 'synthetic ready commit failure'); END;"#,
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(pair.second.initialize(NEW).await.is_err());
    assert_eq!(
        pair.second.load().unwrap().unwrap().catalog.chat_model,
        "deepseek-v4-pro"
    );
    assert!(pair.reopen().api_key().is_err());
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h88_reject_ready")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    pair.first.initialize(NEW).await.unwrap();
    assert_eq!(pair.reopen().api_key().unwrap().as_deref(), Some(NEW));
}

#[tokio::test]
async fn h88_failed_delete_is_not_success_and_revokes_pending_refresh() {
    let pair = Pair::new(true).await;
    let pending = pair.pending(None).await;
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h88_reject_delete BEFORE DELETE ON secret_records
         WHEN OLD.name='deepseek_api_key'
         BEGIN SELECT RAISE(ABORT, 'synthetic secret delete failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(pair.second.clear_key().is_err());
    assert!(pending.finish().await.is_err());
    assert!(pair.reopen().api_key().is_err());
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h88_reject_delete")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    pair.second.clear_key().unwrap();
    assert!(pair.reopen().api_key().unwrap().is_none());
}

#[tokio::test]
async fn h88_corrupt_lifecycle_is_not_reset_or_echoed_by_any_credential_entry() {
    for raw in [
        r#"{"H88_PRIVATE_MARKER":"#,
        r#"{"version":2,"generation":"invalid","phase":"ready"}"#,
    ] {
        let pair = Pair::new(true).await;
        let store = DocumentStore::new(&pair.first.db);
        store
            .put(
                SETTINGS_COLLECTION,
                "credential_lifecycle",
                raw,
                None,
                Timestamp::from_millis(1),
            )
            .unwrap();
        let errors = [
            pair.first.api_key().unwrap_err(),
            pair.first.view().unwrap_err(),
            pair.first.refresh_models().await.unwrap_err(),
            pair.first.initialize(NEW).await.unwrap_err(),
            pair.first.clear_key().unwrap_err(),
        ];
        for error in errors {
            assert!(!error.to_string().contains("H88_PRIVATE_MARKER"));
        }
        assert_eq!(
            store
                .get(SETTINGS_COLLECTION, "credential_lifecycle")
                .unwrap()
                .unwrap()
                .body,
            raw
        );
        assert_eq!(
            pair.first.secrets.get(API_KEY_NAME).unwrap().as_deref(),
            Some(OLD)
        );
    }
}

#[tokio::test]
async fn h88_legacy_import_cannot_resurrect_a_signed_out_key_or_overtake_sign_in() {
    let pair = Pair::new(false).await;
    let legacy = MemorySecretStore::new();
    legacy.set(API_KEY_NAME, OLD).unwrap();
    assert!(pair.first.import_legacy_api_key(&legacy).unwrap());
    assert!(legacy.get(API_KEY_NAME).unwrap().is_none());
    assert_eq!(pair.second.api_key().unwrap().as_deref(), Some(OLD));
    pair.second.clear_key().unwrap();
    legacy.set(API_KEY_NAME, OLD).unwrap();
    assert!(!pair.first.import_legacy_api_key(&legacy).unwrap());
    assert!(pair.reopen().api_key().unwrap().is_none());
    let pending = pair.pending(Some(NEW)).await;
    assert!(!pair.second.import_legacy_api_key(&legacy).unwrap());
    pending.finish().await.unwrap();
    assert_eq!(pair.reopen().api_key().unwrap().as_deref(), Some(NEW));
}

#[tokio::test]
async fn h88_secret_write_failure_is_fail_closed_until_explicit_recovery() {
    let pair = Pair::new(true).await;
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h88_reject_secret BEFORE UPDATE ON secret_records
         WHEN NEW.name='deepseek_api_key'
         BEGIN SELECT RAISE(ABORT, 'synthetic credential failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(pair.first.initialize(NEW).await.is_err());
    assert!(
        pair.reopen().api_key().is_err(),
        "incomplete credential commit must not be usable"
    );
    assert!(pair.reopen().view().is_err());
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h88_reject_secret")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    pair.second.initialize(NEW).await.unwrap();
    assert_eq!(pair.reopen().api_key().unwrap().as_deref(), Some(NEW));
}

#[tokio::test]
async fn h88_normal_refresh_failure_and_sequential_login_logout_controls() {
    let pair = Pair::new(true).await;
    let failing = SettingsService::new(
        pair.first.db.clone(),
        Arc::new(MockTransport::default()),
        pair.first.secrets.clone(),
    );
    let before = pair.first.load().unwrap();
    assert!(failing.refresh_models().await.is_err());
    assert_eq!(pair.first.load().unwrap(), before);
    assert_eq!(pair.first.api_key().unwrap().as_deref(), Some(OLD));
    pair.second.initialize(NEW).await.unwrap();
    pair.second.refresh_models().await.unwrap();
    assert_eq!(pair.first.api_key().unwrap().as_deref(), Some(NEW));
    pair.first.clear_key().unwrap();
    assert!(pair.second.api_key().unwrap().is_none());
    assert!(!pair.second.view().unwrap().unwrap().configured);
    assert!(
        pair.second.load().unwrap().is_some(),
        "logout keeps public preferences/catalog"
    );
}

#[tokio::test]
async fn h88_existing_database_without_lifecycle_remains_readable_without_refresh_rekey() {
    let pair = Pair::new(true).await;
    pair.first.db.with_conn(|connection| {
        connection.execute("DELETE FROM documents WHERE collection='settings' AND id='credential_lifecycle'", [])
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        Ok(())
    }).unwrap();
    assert!(pair.reopen().api_key().unwrap().as_deref() == Some(OLD));
    // This is the successful legacy-credential control, not model removal.
    // Keep the independent second connection but discover the selected Flash;
    // the second host's usual Pro-only fixture now correctly rejects refresh.
    let refresh_host = SettingsService::new(
        pair.second.db.clone(),
        pair.discovery.clone(),
        pair.second.secrets.clone(),
    );
    refresh_host.refresh_models().await.unwrap();
    assert!(DocumentStore::new(&pair.first.db)
        .get(SETTINGS_COLLECTION, "credential_lifecycle")
        .unwrap()
        .is_none());
    let pending = pair.pending(None).await;
    pair.second.clear_key().unwrap();
    assert!(pending.finish().await.is_err());
    assert!(pair.reopen().api_key().unwrap().is_none());
}

#[tokio::test]
async fn h88_initial_metadata_failure_cannot_change_secrets_or_publish_settings() {
    let pair = Pair::new(false).await;
    pair.first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h88_reject_generation BEFORE INSERT ON documents
            WHEN NEW.collection='settings' AND NEW.id='credential_lifecycle'
            BEGIN SELECT RAISE(ABORT, 'synthetic generation failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(pair.second.initialize(NEW).await.is_err());
    assert!(pair.second.clear_key().is_err());
    assert!(pair.first.secrets.get(API_KEY_NAME).unwrap().is_none());
    assert!(pair.reopen().load().unwrap().is_none());
}

#[tokio::test]
async fn h88_read_only_environment_store_never_reports_a_different_key_or_logout_success() {
    // Unique variable: no production environment or real injected credential.
    let variable = format!("H88_SYNTHETIC_{}", uuid::Uuid::new_v4().simple());
    struct Variable(String);
    impl Drop for Variable {
        fn drop(&mut self) {
            std::env::remove_var(&self.0);
        }
    }
    let _owner = Variable(variable.clone());
    std::env::set_var(&variable, OLD);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let service = SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(FLASH)),
        Arc::new(crate::EnvSecretStore::with_var(&variable)),
    );
    service.initialize(OLD).await.unwrap();
    assert!(service.initialize(NEW).await.is_err());
    assert!(service.api_key().is_err());
    service.initialize(OLD).await.unwrap();
    assert!(service.clear_key().is_err());
    assert!(service.api_key().is_err());
    std::env::remove_var(&variable);
    service.clear_key().unwrap();
    assert!(service.api_key().unwrap().is_none());
}

#[tokio::test]
async fn h88_gate_io_failure_and_bounded_contention_are_errors_without_mutation() {
    let pair = Pair::new(false).await;
    let path = pair.first.db.data_root().join("settings-credentials.lock");
    std::fs::create_dir(&path).unwrap();
    assert!(pair.first.initialize(OLD).await.is_err());
    assert!(pair.first.api_key().is_err());
    assert!(pair.first.clear_key().is_err());
    std::fs::remove_dir(&path).unwrap();
    let gate = deepagent_persistence::fs_commit::lock_operation_file(&path).unwrap();
    let started = std::time::Instant::now();
    assert!(pair.second.clear_key().is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(DocumentStore::new(&pair.first.db)
        .get(SETTINGS_COLLECTION, "credential_lifecycle")
        .unwrap()
        .is_none());
    assert!(pair.first.secrets.get(API_KEY_NAME).unwrap().is_none());
    drop(gate);
    pair.second.initialize(NEW).await.unwrap();
    assert!(pair.first.api_key().unwrap().as_deref() == Some(NEW));
}

#[tokio::test]
async fn h88_failed_legacy_cleanup_is_reported_and_never_reimports_after_logout() {
    struct Source(MemorySecretStore);
    impl SecretStore for Source {
        fn set(&self, name: &str, key: &str) -> Result<()> {
            self.0.set(name, key)
        }
        fn get(&self, name: &str) -> Result<Option<String>> {
            self.0.get(name)
        }
        fn delete(&self, _: &str) -> Result<()> {
            Err(CoreError::other("synthetic legacy cleanup failure"))
        }
    }
    let pair = Pair::new(false).await;
    let source = Source(MemorySecretStore::new());
    source.set(API_KEY_NAME, OLD).unwrap();
    assert!(pair.first.import_legacy_api_key(&source).is_err());
    // Destination has actually committed, but source cleanup has not. The
    // caller sees an error, and a subsequent sign-out is a durable tombstone.
    assert!(pair.reopen().api_key().unwrap().as_deref() == Some(OLD));
    assert!(source.get(API_KEY_NAME).unwrap().is_some());
    pair.second.clear_key().unwrap();
    assert!(!pair.first.import_legacy_api_key(&source).unwrap());
    assert!(pair.reopen().api_key().unwrap().is_none());
}
