//! Harness model registry storage (plan §3). Independent of the canvas
//! provider registry, which lives in the document store.

use crate::Database;

fn table_columns(db: &Database, table: &str) -> Vec<String> {
    let conn = db.conn.lock().unwrap();
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(|column| column.unwrap())
        .collect()
}

fn insert_provider(db: &Database, id: &str) {
    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO harness_providers
            (id, channel, model_url, protocol, api_key_ref, created_at, updated_at)
         VALUES (?1, 'test', 'https://example.test/v1', 'responses', ?1, 1, 1)",
        [id],
    )
    .unwrap();
}

#[test]
fn harness_registry_tables_have_the_planned_columns() {
    let db = Database::open_in_memory().unwrap();

    let providers = table_columns(&db, "harness_providers");
    for expected in [
        "id",
        "channel",
        "logo",
        "website_url",
        "model_url",
        "protocol",
        "api_key_ref",
        "balance_kind",
        "status",
        "revision",
        "created_at",
        "updated_at",
    ] {
        assert!(
            providers.iter().any(|column| column == expected),
            "harness_providers is missing `{expected}`: {providers:?}"
        );
    }

    let models = table_columns(&db, "harness_models");
    for expected in [
        "id",
        "provider_id",
        "model_name",
        "display_name",
        "protocol",
        "context_window",
        "output_limit",
        "capabilities",
        "status",
        "updated_at",
    ] {
        assert!(
            models.iter().any(|column| column == expected),
            "harness_models is missing `{expected}`: {models:?}"
        );
    }
}

#[test]
fn model_name_is_unique_within_a_provider_but_not_across_providers() {
    let db = Database::open_in_memory().unwrap();
    insert_provider(&db, "provider-a");
    insert_provider(&db, "provider-b");
    let conn = db.conn.lock().unwrap();

    conn.execute(
        "INSERT INTO harness_models (id, provider_id, model_name, display_name, updated_at)
         VALUES ('m1', 'provider-a', 'shared-name', 'Shared', 1)",
        [],
    )
    .unwrap();

    let duplicate = conn.execute(
        "INSERT INTO harness_models (id, provider_id, model_name, display_name, updated_at)
         VALUES ('m2', 'provider-a', 'shared-name', 'Shared again', 1)",
        [],
    );
    assert!(
        duplicate.is_err(),
        "same provider must reject a duplicate model_name"
    );

    conn.execute(
        "INSERT INTO harness_models (id, provider_id, model_name, display_name, updated_at)
         VALUES ('m3', 'provider-b', 'shared-name', 'Shared elsewhere', 1)",
        [],
    )
    .expect("another provider may use the same model_name");
}

#[test]
fn builtin_providers_have_persisted_responses_routes_branding_and_websites() {
    let db = Database::open_in_memory().unwrap();
    let providers = db.list_harness_providers().unwrap();
    assert_eq!(providers.len(), 2);
    for (id, url, website, logo) in [
        (
            "layso",
            "https://api.layso.ai/v1",
            "https://layso.ai/",
            "/providers/layso.webp",
        ),
        (
            "deepseek",
            "https://api.deepseek.com",
            "https://www.deepseek.com/",
            "/providers/deepseek.svg",
        ),
    ] {
        let provider = providers.iter().find(|provider| provider.id == id).unwrap();
        assert_eq!(provider.model_url, url);
        assert_eq!(provider.protocol, "responses");
        assert_eq!(provider.website_url, website);
        assert_eq!(provider.logo, logo);
        assert!(db.list_harness_models(id).unwrap().is_empty());
    }
    assert!(db.harness_chat_selection().unwrap().is_none());
}

#[test]
fn builtin_seed_preserves_edits_and_deletion_on_cold_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("builtins.sqlite");
    let saved;
    {
        let db = Database::open(&path).unwrap();
        let mut provider = db.list_harness_providers().unwrap().remove(0);
        assert_eq!(provider.id, "deepseek");
        provider.model_url = "https://custom.test/v1".into();
        provider.website_url = "https://custom.test/".into();
        provider.protocol = "messages".into();
        provider.logo = "custom-brand".into();
        provider.enabled = false;
        db.save_harness_provider(&provider).unwrap();
        db.remove_harness_provider("layso").unwrap();
        saved = db.list_harness_providers().unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.list_harness_providers().unwrap(), saved);
}

#[test]
fn reopen_never_imports_a_model_or_rebinds_a_provider_from_old_credentials() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("registry.sqlite");
    {
        let db = Database::open(&path).unwrap();
        db.with_conn(|conn| conn.execute("INSERT INTO secret_records(name,ciphertext,nonce,key_version,updated_at) VALUES('deepseek_api_key',x'00',x'00',1,1)", []).map(|_| ()).map_err(crate::map_sqlite)).unwrap();
    }
    let db = Database::open(&path).unwrap();
    let providers = db.list_harness_providers().unwrap();
    assert_eq!(
        providers
            .iter()
            .find(|provider| provider.id == "deepseek")
            .unwrap()
            .api_key_ref,
        "harness_provider:deepseek"
    );
    assert!(db.list_harness_models("deepseek").unwrap().is_empty());
    assert!(db.harness_chat_model_selection().unwrap().is_none());
}
