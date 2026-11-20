use super::*;

fn v25() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    for script in &MIGRATIONS[..25] {
        conn.execute_batch(script).unwrap();
    }
    conn.execute_batch("PRAGMA user_version=25;").unwrap();
    conn
}

#[test]
fn cutover_removes_old_model_documents_without_importing_or_changing_registry() {
    let conn = v25();
    conn.execute_batch(r#"
        INSERT INTO documents (collection,id,body,created_at,updated_at) VALUES
        ('settings','app','{"catalog":{"chat_model":"old-only"},"discovered_at":12,"thinking_depth":"deep","reasoning_effort":"max","reasoning_effort_explicit":true,"language":"chinese","custom":{"keep":true}}',1,2),
        ('settings','credential_lifecycle','{"phase":"committing"}',1,2),
        ('migrations','harness_registry_seed_v1','{}',1,2),
        ('harness_selection','chat','{"provider_id":"custom","model_name":"current"}',1,2);
        INSERT INTO harness_providers (id,channel,model_url,protocol,api_key_ref,created_at,updated_at)
        VALUES ('custom','Custom','https://custom.test/v1','messages','kept-ref',10,20);
        INSERT INTO harness_models (id,provider_id,model_name,display_name,capabilities,updated_at)
        VALUES ('custom:current','custom','current','Current','{"custom":true}',20);
    "#).unwrap();
    run(&conn).unwrap();
    let body: String = conn
        .query_row(
            "SELECT body FROM documents WHERE collection='settings' AND id='app'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let settings: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(settings.get("catalog").is_none());
    assert!(settings.get("discovered_at").is_none());
    assert!(settings.get("thinking_depth").is_none());
    assert!(settings.get("reasoning_effort").is_none());
    assert!(settings.get("reasoning_effort_explicit").is_none());
    assert_eq!(settings["language"], "chinese");
    assert_eq!(settings["custom"]["keep"], true);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM harness_models", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT api_key_ref FROM harness_providers WHERE id='custom'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "kept-ref"
    );
    assert_eq!(
        conn.query_row("SELECT updated_at FROM harness_models", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        20
    );
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM documents WHERE id IN ('credential_lifecycle','harness_registry_seed_v1')", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn timestamps_cover_all_application_tables_and_preserve_domain_clocks() {
    let conn = v25();
    conn.execute_batch("INSERT INTO sessions (id,created_at,updated_at) VALUES('s',100,200);
        INSERT INTO events (id,session_id,sequence,kind,timestamp,payload) VALUES('e','s',1,'synthetic',123,'{}');
        INSERT INTO execution_leases (lease_id,resource_kind,resource_id,owner,epoch,fencing_token_hash,acquired_at,expires_at)
        VALUES('l','test','r','owner',1,'hash',100,999);").unwrap();
    run(&conn).unwrap();
    let tables = conn.prepare("SELECT name FROM pragma_table_list WHERE schema='main' AND type='table' AND name NOT LIKE 'sqlite_%'").unwrap()
        .query_map([], |row| row.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    for table in tables {
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name IN ('created_at','updated_at') AND type='INTEGER' AND \"notnull\"=1"), [], |row| row.get(0)).unwrap();
        assert_eq!(count, 2, "{table}");
    }
    assert_eq!(
        conn.query_row(
            "SELECT created_at,updated_at,timestamp FROM events WHERE id='e'",
            [],
            |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?
            ))
        )
        .unwrap(),
        (123, 123, 123)
    );
    assert_eq!(
        conn.query_row(
            "SELECT created_at,updated_at FROM execution_leases WHERE lease_id='l'",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (100, 100)
    );
    conn.execute_batch("INSERT INTO events(id,session_id,sequence,kind,timestamp,payload) VALUES('next','s',2,'synthetic',456,'{}');
        UPDATE events SET payload='{\"edited\":true}' WHERE id='next';
        INSERT INTO session_search_docs (id,session_id,sequence,role,timestamp,content_hash) VALUES(1,'s',1,'user',321,'hash');
        INSERT INTO session_search_cjk_bigrams(doc_id,gram) VALUES(1,'test');").unwrap();
    let (created, updated): (i64, i64) = conn
        .query_row(
            "SELECT created_at,updated_at FROM events WHERE id='next'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(created, 456);
    assert!(updated > created);
    assert!(conn
        .query_row(
            "SELECT created_at>0 AND updated_at>=created_at FROM session_search_cjk_bigrams",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    run(&conn).unwrap();
    assert_eq!(
        conn.query_row("SELECT updated_at FROM events WHERE id='next'", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        updated
    );
    assert_eq!(
        conn.query_row(
            "SELECT created_at,updated_at FROM sessions WHERE id='s'",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (100, 200)
    );
}

#[test]
fn search_projection_repair_keeps_timestamp_contract() {
    let conn = v25();
    run(&conn).unwrap();
    conn.execute_batch("DROP TABLE session_search_cursors;")
        .unwrap();
    run(&conn).unwrap();
    for table in [
        "session_search_docs",
        "session_search_cjk_bigrams",
        "session_search_cursors",
    ] {
        let count: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name IN ('created_at','updated_at')"), [], |r| r.get(0)).unwrap();
        assert_eq!(count, 2, "{table}");
    }
    conn.execute_batch("PRAGMA user_version=22;").unwrap();
    run(&conn).unwrap();
}

#[test]
fn timestamp_migration_rolls_back_on_failure() {
    let conn = v25();
    conn.execute_batch("DROP TABLE costs;").unwrap();
    assert!(run(&conn).is_err());
    assert_eq!(current_version(&conn).unwrap(), 26);
    let added: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('events') WHERE name='created_at'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(added, 0, "V27 must not be partially applied");
}
