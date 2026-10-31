//! Durable policy regressions. All databases and failures are synthetic.

use super::*;

fn limits(daily: f64, monthly: f64) -> BudgetConfig {
    BudgetConfig {
        daily_limit: Some(daily),
        monthly_limit: Some(monthly),
    }
}

fn put_body(db: &Database, body: &str) {
    DocumentStore::new(db)
        .put(BUDGET_COLLECTION, BUDGET_ID, body, None, SystemClock.now())
        .unwrap();
}

fn body(db: &Database) -> String {
    DocumentStore::new(db)
        .get(BUDGET_COLLECTION, BUDGET_ID)
        .unwrap()
        .unwrap()
        .body
}

#[test]
fn durable_policy_is_shared_by_owners_and_survives_close_reopen_and_clear() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cost.sqlite");
    let expected = limits(12.5, 200.0);
    {
        let db = Arc::new(Database::open(&path).unwrap());
        let service = CostService::new(db.clone());
        assert_eq!(service.budget().unwrap(), BudgetConfig::default());
        assert!(DocumentStore::new(&db)
            .get(BUDGET_COLLECTION, BUDGET_ID)
            .unwrap()
            .is_none());
        service.set_budget(expected.clone()).unwrap();
        assert_eq!(CostService::new(db.clone()).budget().unwrap(), expected);
        // A separate live connection is not allowed to retain a stale policy.
        let peer = CostService::new(Arc::new(Database::open(&path).unwrap()));
        assert_eq!(peer.summary("").unwrap().budget, expected);
        peer.set_budget(limits(0.0, 100.0)).unwrap();
        assert!(service
            .check_budget()
            .unwrap_err()
            .to_string()
            .contains("daily budget exhausted"));
        service.set_budget(expected.clone()).unwrap();
        drop(peer);
        drop(service);
        assert_eq!(Arc::strong_count(&db), 1);
    }
    let service = CostService::new(Arc::new(Database::open(&path).unwrap()));
    assert_eq!(service.budget().unwrap(), expected);
    service.set_budget(BudgetConfig::default()).unwrap();
    assert_eq!(service.budget().unwrap(), BudgetConfig::default());
    let saved: serde_json::Value = serde_json::from_str(&body(&service.db)).unwrap();
    assert!(saved["daily_limit"].is_null() && saved["monthly_limit"].is_null());
    drop(service);
    assert_eq!(
        CostService::new(Arc::new(Database::open(&path).unwrap()))
            .budget()
            .unwrap(),
        BudgetConfig::default()
    );
    let other_root = tempfile::tempdir().unwrap();
    let other = CostService::new(Arc::new(
        Database::open(other_root.path().join("cost.sqlite")).unwrap(),
    ));
    assert_eq!(other.budget().unwrap(), BudgetConfig::default());
    other.set_budget(limits(1.0, 2.0)).unwrap();
    assert_eq!(
        CostService::new(Arc::new(Database::open(&path).unwrap()))
            .budget()
            .unwrap(),
        BudgetConfig::default()
    );
}

#[test]
fn rejects_invalid_limits_without_changing_the_committed_policy() {
    let service = CostService::new(Arc::new(Database::open_in_memory().unwrap()));
    let expected = limits(1.0, 2.0);
    service.set_budget(expected.clone()).unwrap();
    let saved = body(&service.db);
    for invalid in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for budget in [limits(invalid, 2.0), limits(1.0, invalid)] {
            assert!(service
                .set_budget(budget)
                .unwrap_err()
                .to_string()
                .contains("finite and non-negative"));
            assert_eq!(body(&service.db), saved);
            assert_eq!(service.budget().unwrap(), expected);
        }
    }
    service.set_budget(limits(0.0, 2.0)).unwrap();
    assert!(service
        .check_budget()
        .unwrap_err()
        .to_string()
        .contains("daily budget exhausted"));
    service
        .set_budget(BudgetConfig {
            daily_limit: None,
            monthly_limit: Some(0.0),
        })
        .unwrap();
    assert!(service
        .check_budget()
        .unwrap_err()
        .to_string()
        .contains("monthly budget exhausted"));
}

#[test]
fn corrupt_or_unknown_policies_fail_closed_without_rewriting_the_document() {
    let service = CostService::new(Arc::new(Database::open_in_memory().unwrap()));
    service.set_budget(limits(1.0, 2.0)).unwrap();
    let valid: serde_json::Value = serde_json::from_str(&body(&service.db)).unwrap();
    let mut invalid = vec!["{".to_string(), "null".into(), "{}".into()];
    for field in [
        "version",
        "currency",
        "period",
        "timezone",
        "scope",
        "daily_limit",
        "monthly_limit",
    ] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        invalid.push(missing.to_string());
        let mut unsupported = valid.clone();
        unsupported[field] = match field {
            "version" => serde_json::json!(2),
            "daily_limit" | "monthly_limit" => serde_json::json!(-0.01),
            _ => serde_json::json!("SENSITIVE_SYNTHETIC_CONFIG"),
        };
        invalid.push(unsupported.to_string());
    }
    let mut wrong_type = valid.clone();
    wrong_type["daily_limit"] = serde_json::json!("SENSITIVE_SYNTHETIC_CONFIG");
    invalid.push(wrong_type.to_string());
    let mut extra = valid.clone();
    extra["SENSITIVE_SYNTHETIC_CONFIG"] = serde_json::json!(true);
    invalid.push(extra.to_string());
    invalid.push(body(&service.db).replace("\"daily_limit\":1.0", "\"daily_limit\":1e400"));
    for bad in invalid {
        put_body(&service.db, &bad);
        for error in [
            service.budget().unwrap_err(),
            service.summary("").unwrap_err(),
            service.check_budget().unwrap_err(),
        ] {
            let error = error.to_string();
            assert!(error.contains("budget policy"), "{error}");
            assert!(
                !error.contains("SENSITIVE_SYNTHETIC_CONFIG"),
                "policy contents leaked"
            );
        }
        assert_eq!(body(&service.db), bad);
    }
    // Recovery must be an explicit valid replacement, not a silent default.
    service.set_budget(limits(3.0, 4.0)).unwrap();
    assert_eq!(service.budget().unwrap(), limits(3.0, 4.0));
}

#[test]
fn write_failure_does_not_publish_or_clear_policy_and_retry_is_possible() {
    let service = CostService::new(Arc::new(Database::open_in_memory().unwrap()));
    service
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER reject_budget_insert BEFORE INSERT ON documents
             WHEN NEW.collection = 'settings' AND NEW.id = 'cost-budget'
             BEGIN SELECT RAISE(ABORT, 'synthetic budget write failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(service
        .set_budget(limits(1.0, 2.0))
        .unwrap_err()
        .to_string()
        .contains("budget policy write"));
    assert_eq!(service.budget().unwrap(), BudgetConfig::default());
    service
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER reject_budget_insert")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    let expected = limits(1.0, 2.0);
    service.set_budget(expected.clone()).unwrap();
    service
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER reject_budget_update BEFORE UPDATE ON documents
             WHEN NEW.collection = 'settings' AND NEW.id = 'cost-budget'
             BEGIN SELECT RAISE(ABORT, 'synthetic budget write failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    for replacement in [limits(5.0, 6.0), BudgetConfig::default()] {
        let error = service.set_budget(replacement).unwrap_err().to_string();
        assert!(
            error.contains("budget policy write")
                && error.contains("synthetic budget write failure")
        );
        assert_eq!(service.budget().unwrap(), expected);
    }
    service
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER reject_budget_update")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    service.set_budget(BudgetConfig::default()).unwrap();
    assert!(service.check_budget().is_ok());
}

#[test]
fn read_failure_is_not_unlimited_and_debug_does_not_query_the_database() {
    let service = CostService::new(Arc::new(Database::open_in_memory().unwrap()));
    service.set_budget(limits(1.0, 2.0)).unwrap();
    service
        .db
        .with_conn(|connection| {
            // Isolated schema fault, not a production migration.
            connection
                .execute_batch("ALTER TABLE documents RENAME TO unavailable_documents")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    for error in [
        service.budget().unwrap_err(),
        service.summary("").unwrap_err(),
        service.check_budget().unwrap_err(),
    ] {
        assert!(error.to_string().contains("budget policy read"));
    }
    service
        .db
        .with_conn(|_| {
            // Would deadlock if Debug attempted a nested database read.
            assert_eq!(format!("{service:?}"), "CostService { .. }");
            Ok(())
        })
        .unwrap();
    service
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("ALTER TABLE unavailable_documents RENAME TO documents")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert_eq!(service.budget().unwrap(), limits(1.0, 2.0));
}

#[test]
fn independent_connections_commit_whole_policies_without_stale_caches() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("cost.sqlite");
    let initial = CostService::new(Arc::new(Database::open(&path).unwrap()));
    initial.set_budget(limits(1.0, 10.0)).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = [2.0, 3.0]
        .into_iter()
        .map(|daily| {
            let service = CostService::new(Arc::new(Database::open(&path).unwrap()));
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..16 {
                    service.set_budget(limits(daily, daily * 10.0)).unwrap();
                    let seen = service.budget().unwrap();
                    assert_eq!(
                        seen.monthly_limit,
                        seen.daily_limit.map(|value| value * 10.0)
                    );
                    assert!([Some(2.0), Some(3.0)].contains(&seen.daily_limit));
                }
            })
        })
        .collect();
    barrier.wait();
    for _ in 0..32 {
        let seen = initial.summary("").unwrap().budget;
        assert_eq!(
            seen.monthly_limit,
            seen.daily_limit.map(|value| value * 10.0)
        );
    }
    for handle in handles {
        handle.join().unwrap();
    }
    let final_policy = limits(4.0, 40.0);
    initial.set_budget(final_policy.clone()).unwrap();
    let peer = CostService::new(Arc::new(Database::open(&path).unwrap()));
    assert_eq!(peer.budget().unwrap(), final_policy);
}

#[test]
fn measured_run_usage_counts_against_budget_before_finalization() {
    let service = CostService::new(Arc::new(Database::open_in_memory().unwrap()));
    service.set_pricing(HashMap::from([(
        "model".to_string(),
        ModelPricing {
            input_cache_hit_per_million: 0.0,
            input_cache_miss_per_million: 1000.0,
            output_per_million: 0.0,
        },
    )]));
    service
        .set_budget(BudgetConfig {
            daily_limit: Some(0.5),
            monthly_limit: None,
        })
        .unwrap();
    let usage = deepagent_runtime::usage::ModelUsage {
        provider_id: None,
        model_id: "model".into(),
        usage: deepagent_runtime::agent::RunUsage {
            billed_prompt_tokens: 1_000,
            billed_cache_miss_tokens: 1_000,
            completion_tokens: 10,
            ..Default::default()
        },
        usage_complete: true,
    };

    // The run has not been finalized yet, so the durable-only check remains
    // below the limit; the in-flight check must include the measured call.
    assert!(service.check_budget().is_ok());
    let error = service
        .check_budget_with_usage(&[usage])
        .unwrap_err()
        .to_string();
    assert!(error.contains("daily budget exhausted"), "{error}");
}

#[test]
fn optional_request_admission_distinguishes_exhaustion_from_broken_policy_or_storage() {
    let service = CostService::new(Arc::new(Database::open_in_memory().unwrap()));
    assert!(service.allows_optional_request().unwrap());
    for budget in [limits(0.0, 10.0), limits(10.0, 0.0)] {
        service.set_budget(budget).unwrap();
        assert!(service.check_budget().is_err());
        assert!(!service.allows_optional_request().unwrap());
    }
    put_body(&service.db, "{");
    assert!(service.allows_optional_request().is_err());
    service
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("ALTER TABLE documents RENAME TO unavailable_documents")
                .map_err(|e| CoreError::Persistence(e.to_string()))
        })
        .unwrap();
    assert!(service.allows_optional_request().is_err());
}
