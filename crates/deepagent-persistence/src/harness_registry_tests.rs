//! Harness model registry storage (plan §3). Independent of the canvas
//! provider registry, which lives in the document store.

use crate::harness_registry::import_legacy_deepseek_provider;
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
fn legacy_deepseek_key_becomes_one_provider_row_and_import_is_idempotent() {
    let db = Database::open_in_memory().unwrap();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO secret_records (name, ciphertext, nonce, key_version, updated_at)
             VALUES ('deepseek_api_key', x'00', x'00', 1, 1)",
            [],
        )
        .unwrap();
        import_legacy_deepseek_provider(&conn).unwrap();
        import_legacy_deepseek_provider(&conn).unwrap();
    }

    let conn = db.conn.lock().unwrap();
    let (count, api_key_ref, model_url, protocol): (i64, String, String, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(api_key_ref), MAX(model_url), MAX(protocol)
             FROM harness_providers WHERE id = 'deepseek'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(count, 1, "import must not duplicate the provider row");
    assert_eq!(
        api_key_ref, "deepseek_api_key",
        "key is referenced, never copied"
    );
    assert_eq!(model_url, "https://api.deepseek.com");
    assert_eq!(protocol, "responses");
}

#[test]
fn import_without_a_legacy_key_creates_no_provider_row() {
    let db = Database::open_in_memory().unwrap();
    let conn = db.conn.lock().unwrap();

    import_legacy_deepseek_provider(&conn).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM harness_providers", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}
