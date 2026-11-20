//! H-87: independent hosts must not use a cached settings projection as a
//! writable baseline. Only temporary SQLite files and synthetic credentials.

use super::*;
use crate::secret_store::MemorySecretStore;
use deepagent_models::transport::MockTransport;

pub(super) const MODELS: &str = r#"{"data":[
    {"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"},
    {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek"}
]}"#;

pub(super) fn host(path: &std::path::Path) -> SettingsService {
    SettingsService::new(
        Arc::new(Database::open(path).unwrap()),
        Arc::new(MockTransport::with_get_json(MODELS)),
        Arc::new(MemorySecretStore::new()),
    )
}

async fn pair() -> (tempfile::TempDir, SettingsService, SettingsService) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.sqlite");
    let first = host(&path);
    crate::registry_fixture::configure(&first, "h87-synthetic-credential")
        .await
        .unwrap();
    let second = host(&path);
    assert!(second.load().unwrap().is_some());
    (directory, first, second)
}

#[tokio::test]
async fn h87_warm_host_observes_other_hosts_committed_restriction() {
    let (_directory, first, second) = pair().await;
    assert_eq!(first.sandbox_mode().unwrap(), SandboxMode::WorkspaceWrite);
    second.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
    assert_eq!(first.sandbox_mode().unwrap(), SandboxMode::ReadOnly);
    assert_eq!(first.view().unwrap().unwrap().sandbox_mode, "read_only");
}

#[tokio::test]
async fn h87_disjoint_edits_from_warm_hosts_preserve_all_committed_fields() {
    let (directory, first, second) = pair().await;
    first.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
    second.set_hooks_json(r#"{"hooks":{}}"#).unwrap();
    first
        .set_permission_rules(PermissionRules {
            deny: vec!["Bash(rm:*)".into()],
            ..Default::default()
        })
        .unwrap();
    second.set_welcome_name("H87 fixture").unwrap();
    first.set_reasoning_effort(ReasoningEffort::Max).unwrap();
    second.set_skill_catalog_char_budget(1024).unwrap();
    let persisted = host(&directory.path().join("settings.sqlite"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(persisted.sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(persisted.hooks_json, r#"{"hooks":{}}"#);
    assert_eq!(persisted.permission_rules.deny, ["Bash(rm:*)"]);
    assert_eq!(persisted.welcome_name, "H87 fixture");
    assert_eq!(first.reasoning_effort().unwrap(), ReasoningEffort::Max);
    assert_eq!(persisted.skill_catalog_char_budget, 1024);
}

#[tokio::test]
async fn h87_corrupt_current_document_is_not_replaced_from_a_warm_cache() {
    let (_directory, first, _second) = pair().await;
    let raw = r#"{"H87_PRIVATE_TRUNCATED":"#;
    let store = DocumentStore::new(&first.db);
    store
        .put(
            SETTINGS_COLLECTION,
            SETTINGS_ID,
            raw,
            None,
            Timestamp::from_millis(1),
        )
        .unwrap();
    let loaded = first.load();
    let edited = first.set_sandbox_mode(SandboxMode::ReadOnly);
    let refreshed = first.harness_models().fetch_models("deepseek").await;
    assert!(
        loaded.is_err(),
        "cached settings must not conceal current corruption"
    );
    assert!(edited.is_err());
    assert!(
        refreshed.is_ok(),
        "model refresh no longer reads general settings"
    );
    assert_eq!(
        store
            .get(SETTINGS_COLLECTION, SETTINGS_ID)
            .unwrap()
            .unwrap()
            .body,
        raw
    );
    for error in [loaded.unwrap_err(), edited.unwrap_err()] {
        assert!(!error.to_string().contains("H87_PRIVATE_TRUNCATED"));
    }
}

#[tokio::test]
async fn h87_sequential_refresh_keeps_normal_preferences_and_prices() {
    let (_directory, first, _second) = pair().await;
    first.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
    first.set_welcome_name("normal control").unwrap();
    first
        .harness_models()
        .fetch_models("deepseek")
        .await
        .unwrap();
    let view = first.view().unwrap().unwrap();
    assert!(view.configured);
    assert_eq!(view.sandbox_mode, "read_only");
    assert!(!view.pricing_catalog.models.is_empty());
    assert_eq!(first.welcome_name().unwrap(), "normal control");
}

#[tokio::test]
async fn h87_missing_cache_observes_other_host_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("settings.sqlite");
    let first = host(&path);
    assert!(first.load().unwrap().is_some());
    assert!(!first.view().unwrap().unwrap().configured);
    let second = host(&path);
    crate::registry_fixture::configure(&second, "h87-synthetic-credential")
        .await
        .unwrap();
    first.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
    assert_eq!(second.sandbox_mode().unwrap(), SandboxMode::ReadOnly);
}

#[tokio::test]
async fn h87_competing_creator_is_merged_instead_of_replaced() {
    let (_directory, first, second) = pair().await;
    let initial = first.load().unwrap().unwrap();
    DocumentStore::new(&first.db)
        .delete(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap();
    let attempts = std::cell::Cell::new(0);
    let saved = first
        .edit_settings(|prior| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                assert!(prior.is_none());
                second.edit_settings(|other| {
                    assert!(other.is_none());
                    let mut settings = initial.clone();
                    settings.sandbox_mode = SandboxMode::ReadOnly;
                    Ok(settings)
                })?;
            }
            let mut settings = prior.unwrap_or_else(|| initial.clone());
            settings.welcome_name = "creator merge".into();
            Ok(settings)
        })
        .unwrap();
    assert_eq!(attempts.get(), 2);
    assert_eq!(saved.sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(second.welcome_name().unwrap(), "creator merge");
}

#[tokio::test]
async fn h87_cas_conflict_reapplies_only_this_edit_to_the_new_current_document() {
    let (_directory, first, second) = pair().await;
    let attempts = std::cell::Cell::new(0);
    let saved = first
        .update_settings(|settings| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                // A real independent SQLite writer commits after the owner read,
                // before its conditional UPDATE. No fake repository/CAS outcome.
                second.set_sandbox_mode(SandboxMode::ReadOnly)?;
            }
            settings.welcome_name = "retried edit".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(attempts.get(), 2);
    assert_eq!(saved.sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(second.welcome_name().unwrap(), "retried edit");
}

#[tokio::test]
async fn h87_conflict_budget_returns_an_error_without_publishing_an_uncommitted_edit() {
    let (_directory, first, second) = pair().await;
    let attempts = std::cell::Cell::new(0);
    let error = first
        .update_settings(|settings| {
            attempts.set(attempts.get() + 1);
            second.set_skill_catalog_char_budget(1000 + attempts.get())?;
            settings.welcome_name = "H87_UNCOMMITTED".into();
            Ok(())
        })
        .unwrap_err();
    assert_eq!(attempts.get(), persistence::MAX_COMMIT_ATTEMPTS);
    assert!(error.to_string().contains("retry the operation"));
    assert_eq!(first.welcome_name().unwrap(), "");
    assert_eq!(
        first.skill_catalog_char_budget().unwrap(),
        1000 + attempts.get()
    );
}

#[tokio::test]
async fn h87_database_failure_does_not_publish_or_report_a_successful_edit() {
    let (_directory, first, second) = pair().await;
    first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER h87_reject BEFORE UPDATE ON documents
             WHEN NEW.collection='settings' AND NEW.id='app'
             BEGIN SELECT RAISE(ABORT, 'synthetic H87 write failure'); END;",
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    assert!(first.set_welcome_name("H87_UNCOMMITTED").is_err());
    assert_eq!(first.welcome_name().unwrap(), "");
    assert_eq!(second.welcome_name().unwrap(), "");
    first
        .db
        .with_conn(|connection| {
            connection
                .execute_batch("DROP TRIGGER h87_reject")
                .map_err(|error| CoreError::Persistence(error.to_string()))
        })
        .unwrap();
    first.set_welcome_name("committed control").unwrap();
    assert_eq!(second.welcome_name().unwrap(), "committed control");
}

#[tokio::test]
async fn h87_current_fencing_rule_cannot_be_replaced_by_a_warm_prefix_grant() {
    let (_directory, first, second) = pair().await;
    second
        .set_permission_rules(PermissionRules {
            deny: vec!["Bash(git:*)".into()],
            ..Default::default()
        })
        .unwrap();
    assert!(first.grant_command_prefix("git status").is_err());
    let rules = second.permission_rules().unwrap();
    assert_eq!(rules.deny, ["Bash(git:*)"]);
    assert!(rules.allow.is_empty());
}

#[tokio::test]
async fn h87_unrecognized_top_level_fields_survive_setters_and_refresh() {
    let (_directory, first, _second) = pair().await;
    let store = DocumentStore::new(&first.db);
    let document = store
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap();
    let mut raw: serde_json::Value = serde_json::from_str(&document.body).unwrap();
    raw["futureSettings"] = serde_json::json!({"keep":[true, "extension"]});
    store
        .put(
            SETTINGS_COLLECTION,
            SETTINGS_ID,
            &raw.to_string(),
            None,
            Timestamp::from_millis(1),
        )
        .unwrap();
    first.set_welcome_name("extension control").unwrap();
    first
        .harness_models()
        .fetch_models("deepseek")
        .await
        .unwrap();
    let saved = store
        .get(SETTINGS_COLLECTION, SETTINGS_ID)
        .unwrap()
        .unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved.body).unwrap();
    assert_eq!(saved["futureSettings"], raw["futureSettings"]);
}

#[tokio::test]
async fn h87_permission_profile_reads_coupled_fields_from_one_committed_snapshot() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (_directory, first, second) = pair().await;
    let start = Arc::new(std::sync::Barrier::new(2));
    let done = Arc::new(AtomicBool::new(false));
    let writer = {
        let start = start.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            start.wait();
            for index in 0..200 {
                let preset = if index % 2 == 0 {
                    PermissionPreset::FullAccess
                } else {
                    PermissionPreset::Default
                };
                second.set_active_permission_preset(preset).unwrap();
                std::thread::yield_now();
            }
            done.store(true, Ordering::Release);
        })
    };
    start.wait();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut observations = 0;
    while !done.load(Ordering::Acquire) || observations < 200 {
        let observed = first.effective_permission_profile().unwrap();
        assert!(
            observed == PermissionPreset::Default.to_effective_profile()
                || observed == PermissionPreset::FullAccess.to_effective_profile(),
            "one profile must not combine different committed presets"
        );
        observations += 1;
        assert!(
            std::time::Instant::now() < deadline,
            "profile sampling timeout"
        );
    }
    writer.join().unwrap();
}
