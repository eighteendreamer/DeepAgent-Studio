//! H66: search configuration uses the existing document CAS and never turns a
//! rejected publication into a successful cache value. Synthetic SQLite only.
use super::*;
use crate::secret_store::MemorySecretStore;
use deepagent_models::transport::MockTransport;

fn service_at(db: Arc<Database>, secrets: Arc<MemorySecretStore>) -> SettingsService {
    SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash"}]}"#,
        )),
        secrets,
    )
}

#[tokio::test]
async fn web_search_rejected_write_keeps_exact_document_and_cache_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("search-settings.sqlite3");
    let db = Arc::new(Database::open(&path).unwrap());
    let secrets = Arc::new(MemorySecretStore::new());
    let settings = service_at(db.clone(), secrets.clone());
    crate::registry_fixture::configure(&settings, "synthetic-search-login")
        .await
        .unwrap();
    let previous = settings.web_search_settings().unwrap();
    let body = DocumentStore::new(&db)
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap()
        .body;
    db.with_conn(|connection| {
        connection
            .execute_batch(
                "CREATE TRIGGER reject_search_settings BEFORE UPDATE ON documents
             WHEN NEW.collection='settings' AND NEW.id='app'
             BEGIN SELECT RAISE(ABORT, 'synthetic search settings failure'); END;",
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    let mut replacement = previous.clone();
    replacement.strategy = WebSearchStrategy::Verify;
    replacement.engines.reverse();
    for engine in &mut replacement.engines {
        engine.enabled = false;
    }
    assert!(settings
        .set_web_search_settings(replacement.clone())
        .is_err());
    assert_eq!(settings.web_search_settings().unwrap(), previous);
    assert_eq!(
        DocumentStore::new(&db)
            .get(SETTINGS_COLLECTION, SETTINGS_ID)
            .unwrap()
            .unwrap()
            .body,
        body
    );
    drop(settings);
    drop(db);

    let db = Arc::new(Database::open(&path).unwrap());
    let settings = service_at(db.clone(), secrets);
    assert_eq!(settings.web_search_settings().unwrap(), previous);
    db.with_conn(|connection| {
        connection
            .execute_batch("DROP TRIGGER reject_search_settings")
            .map_err(|error| CoreError::Persistence(error.to_string()))
    })
    .unwrap();
    settings
        .set_web_search_settings(replacement.clone())
        .unwrap();
    assert_eq!(settings.web_search_settings().unwrap(), replacement);
}

#[tokio::test]
async fn web_search_concurrent_service_updates_preserve_other_settings_and_cold_priority_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("search-concurrent.sqlite3");
    let secrets = Arc::new(MemorySecretStore::new());
    let first = service_at(Arc::new(Database::open(&path).unwrap()), secrets.clone());
    crate::registry_fixture::configure(&first, "synthetic-search-login")
        .await
        .unwrap();
    let second = service_at(Arc::new(Database::open(&path).unwrap()), secrets.clone());
    let mut config = WebSearchSettings::default();
    for round in 0..8 {
        config.engines.rotate_right(1);
        config.engines[0].enabled = round % 2 == 0;
        config.strategy = if round % 2 == 0 {
            WebSearchStrategy::Verify
        } else {
            WebSearchStrategy::Fallback
        };
        let name = format!("synthetic-{round}");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let barrier = &barrier;
            let search = scope.spawn(|| {
                barrier.wait();
                first.set_web_search_settings(config.clone())
            });
            let unrelated = scope.spawn(|| {
                barrier.wait();
                second.set_welcome_name(&name)
            });
            search.join().unwrap().unwrap();
            unrelated.join().unwrap().unwrap();
        });
        assert_eq!(first.web_search_settings().unwrap(), config);
        assert_eq!(first.welcome_name().unwrap(), name);
        assert_eq!(second.web_search_settings().unwrap(), config);
    }
    drop(first);
    drop(second);
    let reopened = service_at(Arc::new(Database::open(&path).unwrap()), secrets);
    assert_eq!(reopened.web_search_settings().unwrap(), config);
    assert_eq!(reopened.welcome_name().unwrap(), "synthetic-7");
}
