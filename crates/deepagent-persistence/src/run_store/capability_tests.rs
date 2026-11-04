//! H-219: one immutable snapshot in the existing run ledger, not another store.
use super::*;
use deepagent_core::clock::Timestamp;
use std::sync::{Arc, Barrier};

fn snapshot(run: &str, session: &str) -> serde_json::Value {
    serde_json::json!({"runId":run, "threadId":session, "snapshotId":"synthetic", "version":1})
}

#[test]
fn h219_snapshot_binding_rejects_foreign_missing_terminal_and_duplicate_owners() {
    let db = Database::open_in_memory().unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    let session = session.to_string();
    let store = RunStore::new(&db);
    store.create("active", &session, None, 1).unwrap();
    store.create("terminal", &session, None, 1).unwrap();
    store
        .finish_with_event(
            "terminal",
            "failed",
            None,
            &serde_json::json!({"kind":"failed"}),
            None,
            2,
        )
        .unwrap();
    for (run, data) in [
        ("missing", snapshot("missing", &session)),
        ("terminal", snapshot("terminal", &session)),
        ("active", snapshot("other", &session)),
        ("active", snapshot("active", "foreign")),
        ("active", serde_json::json!({})),
    ] {
        assert!(store.bind_capability_snapshot(run, &data, 3).is_err());
    }
    let data = snapshot("active", &session);
    store.bind_capability_snapshot("active", &data, 3).unwrap();
    assert!(store.bind_capability_snapshot("active", &data, 4).is_err());
    let events = store.events_after("active", None).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].data, data);
    assert_eq!(events[1].sequence, 1);
}

#[test]
fn h219_snapshot_insert_failure_rolls_back_and_retry_is_not_poisoned() {
    let db = Database::open_in_memory().unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    let store = RunStore::new(&db);
    store
        .create("failure", &session.to_string(), None, 1)
        .unwrap();
    db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_capability BEFORE INSERT ON run_events WHEN NEW.event_type='capability.snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic bind failure'); END;").map_err(map_sqlite)).unwrap();
    let data = snapshot("failure", &session.to_string());
    assert!(store.bind_capability_snapshot("failure", &data, 2).is_err());
    assert_eq!(store.events_after("failure", None).unwrap().len(), 1);
    store
        .create("legacy-failure", &session.to_string(), None, 1)
        .unwrap();
    let fingerprint = RegistryCapabilityFingerprint {
        tool_count: 1,
        tool_schema_hash: "synthetic-registry".into(),
    };
    assert!(store
        .ensure_registry_capabilities("legacy-failure", &fingerprint, 2)
        .is_err());
    assert_eq!(store.events_after("legacy-failure", None).unwrap().len(), 1);
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_capability;")
            .map_err(map_sqlite)
    })
    .unwrap();
    store.bind_capability_snapshot("failure", &data, 3).unwrap();
    assert_eq!(store.events_after("failure", None).unwrap()[1].sequence, 1);
    store
        .ensure_registry_capabilities("legacy-failure", &fingerprint, 3)
        .unwrap();
    assert_eq!(
        store.events_after("legacy-failure", None).unwrap()[1].sequence,
        1
    );
}

#[test]
fn h219_independent_connections_cannot_bind_twice_or_append_after_terminal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("capabilities.sqlite");
    let db = Database::open(&path).unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    let session = session.to_string();
    for round in 0..8 {
        let run = format!("race-{round}");
        RunStore::new(&db).create(&run, &session, None, 1).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let connections = (0..3)
            .map(|_| Database::open(&path).unwrap())
            .collect::<Vec<_>>();
        let threads = connections
            .into_iter()
            .enumerate()
            .map(|(index, connection)| {
                let run = run.clone();
                let data = snapshot(&run, &session);
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let store = RunStore::new(&connection);
                    if index == 2 {
                        store
                            .finish_with_event(
                                &run,
                                "failed",
                                None,
                                &serde_json::json!({"kind":"failed"}),
                                None,
                                3,
                            )
                            .unwrap();
                        false
                    } else {
                        store.bind_capability_snapshot(&run, &data, 2).is_ok()
                    }
                })
            })
            .collect::<Vec<_>>();
        let accepted = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert!(accepted <= 1);
        let events = RunStore::new(&db).events_after(&run, None).unwrap();
        let snapshots: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == "capability.snapshot")
            .collect();
        assert_eq!(snapshots.len(), accepted);
        let terminal = events
            .iter()
            .find(|e| e.event_type == "run_terminal")
            .unwrap();
        if let Some(snapshot) = snapshots.first() {
            assert!(snapshot.sequence < terminal.sequence);
        }
        assert!(RunStore::new(&db)
            .bind_capability_snapshot(&run, &snapshot(&run, &session), 4)
            .is_err());
    }
}

#[test]
fn h219_registry_verification_preserves_rich_evidence_and_rejects_invalid_owners() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("registry.sqlite");
    let db = Database::open(&path).unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    let session = session.to_string();
    let fingerprint = RegistryCapabilityFingerprint {
        tool_count: 1,
        tool_schema_hash: "synthetic-schema".into(),
    };
    let store = RunStore::new(&db);
    for case in [
        "matching",
        "foreign-run",
        "foreign-thread",
        "missing-owner",
        "stripped-owner",
        "unknown-version",
        "schema",
        "duplicate",
        "malformed",
    ] {
        store.create(case, &session, None, 1).unwrap();
        let mut data = serde_json::to_value(&fingerprint).unwrap();
        data["version"] = serde_json::json!(1);
        data["runId"] = serde_json::json!(case);
        data["threadId"] = serde_json::json!(session);
        data["snapshotId"] = serde_json::json!("keep-this-identity");
        match case {
            "foreign-run" => data["runId"] = serde_json::json!("other"),
            "foreign-thread" => data["threadId"] = serde_json::json!("other"),
            "missing-owner" => {
                data.as_object_mut().unwrap().remove("threadId");
            }
            "stripped-owner" => {
                for key in ["version", "runId", "threadId"] {
                    data.as_object_mut().unwrap().remove(key);
                }
            }
            "unknown-version" => data["version"] = serde_json::json!(2),
            "schema" => data["toolSchemaHash"] = serde_json::json!("changed-same-count"),
            "malformed" => {
                data.as_object_mut().unwrap().remove("toolCount");
            }
            _ => {}
        }
        // Inject corrupt historical evidence through the generic ledger seam.
        // Normal AppCore binding already rejects foreign or duplicate owners.
        for _ in 0..if case == "duplicate" { 2 } else { 1 } {
            store
                .append_event(
                    case,
                    2,
                    "preparing",
                    "completed",
                    "capability.snapshot",
                    &data,
                )
                .unwrap();
        }
        let before = store.events_after(case, None).unwrap();
        assert_eq!(
            store
                .ensure_registry_capabilities(case, &fingerprint, 3)
                .is_ok(),
            case == "matching",
            "{case}"
        );
        assert_eq!(store.events_after(case, None).unwrap(), before, "{case}");
    }
    assert!(store
        .ensure_registry_capabilities("missing", &fingerprint, 4)
        .is_err());
    store
        .finish_with_event(
            "matching",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded"}),
            None,
            4,
        )
        .unwrap();
    assert!(store
        .ensure_registry_capabilities("matching", &fingerprint, 5)
        .is_err());
    let saved = store.events_after("matching", None).unwrap();
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("matching", None)
            .unwrap(),
        saved
    );
}

#[test]
fn h219_concurrent_legacy_registry_checks_publish_one_snapshot_without_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("concurrent-registry.sqlite");
    let db = Database::open(&path).unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    for conflicting in [false, true] {
        let run = if conflicting { "conflicting" } else { "same" };
        RunStore::new(&db)
            .create(run, &session.to_string(), None, 1)
            .unwrap();
        let connections = (0..16)
            .map(|_| Database::open(&path).unwrap())
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(connections.len()));
        let threads = connections
            .into_iter()
            .enumerate()
            .map(|(index, connection)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let fingerprint = RegistryCapabilityFingerprint {
                        tool_count: 1,
                        tool_schema_hash: if conflicting && index % 2 == 0 {
                            "other".into()
                        } else {
                            "same".into()
                        },
                    };
                    barrier.wait();
                    let accepted = RunStore::new(&connection)
                        .ensure_registry_capabilities(run, &fingerprint, 2)
                        .is_ok();
                    (accepted, fingerprint)
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        let events = RunStore::new(&db).events_after(run, None).unwrap();
        assert_eq!(events.len(), 2);
        let saved: RegistryCapabilityFingerprint =
            serde_json::from_value(events[1].data.clone()).unwrap();
        assert!(
            events[1].data.get("runId").is_none(),
            "legacy must not invent rich identity"
        );
        assert_eq!(
            results.iter().filter(|(ok, _)| *ok).count(),
            if conflicting { 8 } else { 16 }
        );
        for (accepted, fingerprint) in results {
            assert_eq!(accepted, fingerprint == saved);
        }
    }
}

fn schema(number: u64, hash: &str) -> serde_json::Value {
    serde_json::json!({"generation":number, "schemaHash":hash, "tools":[]})
}

fn admit_schema(db: &Database, run: &str, session: &str) {
    let mut data = snapshot(run, session);
    data["modelSchema"] = schema(0, "initial");
    assert!(RunStore::new(db)
        .admit(
            RunAdmission {
                run_id: run,
                session_id: session,
                task_id: None,
                replaces_run_id: None,
                capability_snapshot: Some(&data),
            },
            1
        )
        .unwrap());
}

#[test]
fn h219_schema_transactions_validate_owner_generation_retry_and_terminal_without_blocking_receipts()
{
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("generations.sqlite");
    let db = Database::open(&path).unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    admit_schema(&db, "generation", &session.to_string());
    let store = RunStore::new(&db);
    let initial = schema(0, "initial");
    store
        .record_capability_schema(
            "generation",
            "synthetic",
            CapabilitySchema::Model(&initial),
            2,
        )
        .unwrap();
    for (run, identity, value) in [
        ("missing", "synthetic", initial.clone()),
        ("generation", "foreign", initial.clone()),
        ("generation", "", initial.clone()),
        ("generation", "synthetic", schema(2, "skipped")),
        (
            "generation",
            "synthetic",
            serde_json::json!({"generation":1}),
        ),
        (
            "generation",
            "synthetic",
            serde_json::json!({"generation":-1,"schemaHash":"negative"}),
        ),
    ] {
        assert!(store
            .record_capability_schema(run, identity, CapabilitySchema::Model(&value), 2)
            .is_err());
    }
    let next = schema(1, "next");
    db.with_conn(|conn| conn.execute_batch("CREATE TRIGGER reject_generation BEFORE INSERT ON run_events WHEN NEW.event_type='capability_schema_snapshot' BEGIN SELECT RAISE(ABORT, 'synthetic generation failure'); END;").map_err(map_sqlite)).unwrap();
    assert!(store
        .record_capability_schema("generation", "synthetic", CapabilitySchema::Model(&next), 2)
        .is_err());
    assert_eq!(store.events_after("generation", None).unwrap().len(), 2);
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER reject_generation;")
            .map_err(map_sqlite)
    })
    .unwrap();
    for _ in 0..2 {
        store
            .record_capability_schema("generation", "synthetic", CapabilitySchema::Model(&next), 3)
            .unwrap();
    }
    assert!(store
        .record_capability_schema(
            "generation",
            "synthetic",
            CapabilitySchema::Model(&schema(1, "conflict")),
            3
        )
        .is_err());
    assert_eq!(store.events_after("generation", None).unwrap().len(), 3);
    let wire = schema(0, "wire");
    for _ in 0..2 {
        store
            .record_capability_schema("generation", "synthetic", CapabilitySchema::Wire(&wire), 4)
            .unwrap();
    }
    assert_eq!(store.events_after("generation", None).unwrap().len(), 4);
    store
        .finish_with_event(
            "generation",
            "succeeded",
            None,
            &serde_json::json!({"kind":"succeeded"}),
            None,
            5,
        )
        .unwrap();
    for projection in [
        CapabilitySchema::Model(&next),
        CapabilitySchema::Wire(&wire),
    ] {
        assert!(store
            .record_capability_schema("generation", "synthetic", projection, 6)
            .is_err());
    }
    // Capability publication is closed, but usage/outbox receipts are not.
    store
        .append_event(
            "generation",
            6,
            "terminal",
            "completed",
            "synthetic_receipt",
            &serde_json::json!({}),
        )
        .unwrap();
    let events = store.events_after("generation", None).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (0..6).collect::<Vec<_>>()
    );
    let saved = store.capability_snapshot("generation").unwrap().unwrap();
    assert_eq!(saved["modelSchema"], next);
    assert_eq!(saved["wireSchema"], wire);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .capability_snapshot("generation")
            .unwrap(),
        Some(saved)
    );
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("generation", None)
            .unwrap(),
        events
    );
}

#[test]
fn h219_independent_schema_writers_reuse_one_generation_or_reject_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("schema-writers.sqlite");
    let db = Database::open(&path).unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    for conflicting in [false, true] {
        let run = if conflicting { "conflicting" } else { "same" };
        admit_schema(&db, run, &session.to_string());
        let connections = (0..16)
            .map(|_| Database::open(&path).unwrap())
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(connections.len()));
        let threads = connections
            .into_iter()
            .enumerate()
            .map(|(index, connection)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let proposed = schema(
                        1,
                        if conflicting && index % 2 == 0 {
                            "other"
                        } else {
                            "same"
                        },
                    );
                    barrier.wait();
                    let accepted = RunStore::new(&connection)
                        .record_capability_schema(
                            run,
                            "synthetic",
                            CapabilitySchema::Model(&proposed),
                            2,
                        )
                        .is_ok();
                    (accepted, proposed)
                })
            })
            .collect::<Vec<_>>();
        let results = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        let events = RunStore::new(&db).events_after(run, None).unwrap();
        assert_eq!(events.len(), 3);
        let published = &events[2].data["modelSchema"];
        assert_eq!(
            results.iter().filter(|(ok, _)| *ok).count(),
            if conflicting { 8 } else { 16 }
        );
        for (accepted, proposed) in results {
            assert_eq!(accepted, proposed == *published);
        }
    }
}

#[test]
fn h219_schema_publication_races_terminal_in_the_original_ledger_transaction() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("schema-terminal.sqlite");
    let db = Database::open(&path).unwrap();
    let session = deepagent_core::id::SessionId::new();
    crate::event_store::EventStore::new(&db)
        .create_session(session, None, Timestamp::from_millis(1))
        .unwrap();
    for round in 0..8 {
        let run = format!("race-{round}");
        admit_schema(&db, &run, &session.to_string());
        let connections = (0..3)
            .map(|_| Database::open(&path).unwrap())
            .collect::<Vec<_>>();
        let barrier = Arc::new(Barrier::new(3));
        let threads = connections
            .into_iter()
            .enumerate()
            .map(|(index, connection)| {
                let barrier = barrier.clone();
                let run = run.clone();
                std::thread::spawn(move || {
                    let store = RunStore::new(&connection);
                    barrier.wait();
                    if index == 2 {
                        assert!(store
                            .finish_with_event(
                                &run,
                                "cancelled",
                                None,
                                &serde_json::json!({"kind":"cancelled"}),
                                None,
                                3
                            )
                            .unwrap());
                        false
                    } else {
                        store
                            .record_capability_schema(
                                &run,
                                "synthetic",
                                CapabilitySchema::Model(&schema(
                                    1,
                                    if index == 0 { "left" } else { "right" },
                                )),
                                2,
                            )
                            .is_ok()
                    }
                })
            })
            .collect::<Vec<_>>();
        let accepted = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert!(accepted <= 1);
        let store = RunStore::new(&db);
        let events = store.events_after(&run, None).unwrap();
        let terminal = events
            .iter()
            .find(|event| event.event_type == "run_terminal")
            .unwrap();
        let schemas = events
            .iter()
            .filter(|event| event.event_type == "capability_schema_snapshot")
            .collect::<Vec<_>>();
        assert_eq!(schemas.len(), accepted);
        assert!(schemas
            .iter()
            .all(|event| event.sequence < terminal.sequence));
        assert!(store
            .record_capability_schema(
                &run,
                "synthetic",
                CapabilitySchema::Wire(&schema(0, "late")),
                4
            )
            .is_err());
    }
}
