//! Fault isolation across rows in the persisted scheduled-task document.
//! Poll callbacks only record ids: no model call, command or scheduled run starts.

use super::*;
use crate::{MemorySecretStore, SettingsService};
use deepagent_builtins::cron_tools::{CronCreateTool, CronDeleteTool};
use deepagent_models::transport::MockTransport;
use deepagent_persistence::Database;
use deepagent_runtime::schedule::ScheduledTaskStore;
use deepagent_tools::Tool;

enum RowFault {
    None,
    InvalidCron,
    MissingPrompt,
    WrongCreatedAt,
    WrongOptionalAnchor,
}

fn fixture() -> (tempfile::TempDir, Arc<CronService>) {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let transport = Arc::new(MockTransport::new(Vec::<String>::new()));
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        transport.clone(),
        Arc::new(MemorySecretStore::new()),
    ));
    let chat = Arc::new(ChatService::new(db, settings, transport, root.path()));
    let cron = Arc::new(CronService::new(root.path(), chat));
    (root, cron)
}

async fn check_row_isolation(label: &str, fault: RowFault) {
    let (root, cron) = fixture();
    let mut healthy = Vec::new();
    for prompt in ["AUDIT_CRON_HEALTHY_A_270", "AUDIT_CRON_HEALTHY_B_270"] {
        healthy.push(
            cron.create("* * * * *".into(), prompt.into(), true)
                .await
                .unwrap(),
        );
    }
    assert_eq!(cron.list().await.unwrap().len(), 2);
    let path = cron.scheduler.store().path();
    let mut document: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut bad = serde_json::json!({
        "id": "audit-bad-row-270", "cron": "* * * * *",
        "prompt": "AUDIT_BAD_ROW_270", "createdAt": 0,
    });
    match fault {
        RowFault::None => {}
        RowFault::InvalidCron => bad["cron"] = serde_json::json!("not a cron"),
        RowFault::MissingPrompt => {
            bad.as_object_mut().unwrap().remove("prompt");
        }
        RowFault::WrongCreatedAt => bad["createdAt"] = serde_json::json!("not an epoch"),
        RowFault::WrongOptionalAnchor => {
            bad["lastFiredAt"] = serde_json::json!("not an epoch");
        }
    }
    if !matches!(fault, RowFault::None) {
        document["tasks"].as_array_mut().unwrap().push(bad.clone());
    }
    // Test-owned document only. The surrounding JSON and the two healthy rows
    // remain valid; this is not a truncated document or a filesystem failure.
    std::fs::write(path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let before = std::fs::read(path).unwrap();
    let visible = cron.list().await.unwrap();
    let listed = visible.len();
    assert!(visible.iter().all(|task| healthy.contains(&task.id)));
    assert_eq!(
        std::fs::read(path).unwrap(),
        before,
        "list itself is read-only"
    );

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let due_at = deepagent_runtime::schedule::cron::epoch_ms_to_local(now_ms).unwrap()
        + time::Duration::minutes(2);
    let mut fired = Vec::new();
    cron.scheduler
        .poll(due_at, |task| fired.push(task.id.clone()))
        .unwrap();
    let mut sorted_fired = fired.clone();
    sorted_fired.sort();
    healthy.sort();
    let raw_before_create: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(healthy.iter().all(|id| raw_before_create["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"].as_str() == Some(id))));
    if !matches!(fault, RowFault::None) {
        assert!(raw_before_create["tasks"]
            .as_array()
            .unwrap()
            .contains(&bad));
    }

    // The ordinary model-facing tool uses the real CronService backend.
    let created = CronCreateTool::new(cron.clone())
        .invoke(serde_json::json!({
            "cron": "* * * * *", "prompt": "AUDIT_CRON_NEW_270", "recurring": true,
        }))
        .await
        .unwrap();
    assert!(created.ok && created.value["scheduled"] == true);
    let new_id = created.value["id"].as_str().unwrap().to_owned();
    drop(cron);

    let reopened = ScheduledTaskStore::new(root.path());
    let remaining = reopened.read().unwrap();
    let raw_after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(reopened.path()).unwrap()).unwrap();
    let healthy_on_disk = healthy.iter().all(|id| {
        raw_after["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"].as_str() == Some(id))
    });
    let healthy_on_reopen = healthy
        .iter()
        .all(|id| remaining.iter().any(|task| &task.id == id));
    if !matches!(fault, RowFault::None) {
        assert!(raw_after["tasks"].as_array().unwrap().contains(&bad));
    }
    assert!(remaining.iter().any(|task| task.id == new_id));
    eprintln!(
        "{label}: listed={listed}, due_callbacks={}, create_ok={}, reopened={}, healthy_raw_after_create={healthy_on_disk}, healthy_reopened={healthy_on_reopen}",
        fired.len(), created.ok, remaining.len(),
    );
    assert!(
        listed == 2 && sorted_fired == healthy && healthy_on_disk && healthy_on_reopen,
        "one invalid row must not hide healthy schedules and let a later successful create erase them"
    );
}

#[tokio::test]
async fn healthy_schedule_rows_survive_poll_and_create() {
    check_row_isolation("healthy", RowFault::None).await;
}

#[tokio::test]
async fn invalid_cron_row_does_not_poison_healthy_schedules() {
    check_row_isolation("invalid-cron-control", RowFault::InvalidCron).await;
}

#[tokio::test]
async fn audit_missing_prompt_row_is_isolated() {
    check_row_isolation("missing-prompt", RowFault::MissingPrompt).await;
}

#[tokio::test]
async fn audit_wrong_created_at_row_is_isolated() {
    check_row_isolation("wrong-createdAt", RowFault::WrongCreatedAt).await;
}

#[tokio::test]
async fn audit_wrong_optional_anchor_row_is_isolated() {
    check_row_isolation("wrong-lastFiredAt", RowFault::WrongOptionalAnchor).await;
}

#[tokio::test]
async fn h270_corrupt_document_is_reported_by_real_tools_without_overwriting_it() {
    for raw in [r#"{"tasks":["PRIVATE_UNCLOSED""#, r#"{"tasks":{}}"#, "\n"] {
        let (root, cron) = fixture();
        let id = cron
            .create("* * * * *".into(), "healthy".into(), true)
            .await
            .unwrap();
        let path = cron.scheduler.store().path();
        std::fs::write(path, raw).unwrap();
        for output in [
            CronCreateTool::new(cron.clone())
                .invoke(serde_json::json!({"cron":"* * * * *", "prompt":"new"}))
                .await
                .unwrap(),
            CronDeleteTool::new(cron.clone())
                .invoke(serde_json::json!({"operation":"list"}))
                .await
                .unwrap(),
            CronDeleteTool::new(cron.clone())
                .invoke(serde_json::json!({"operation":"delete", "task_ids":[id]}))
                .await
                .unwrap(),
        ] {
            assert!(
                !output.ok,
                "a corrupt existing document must not report success"
            );
            assert!(!serde_json::to_string(&output.value)
                .unwrap()
                .contains("PRIVATE_UNCLOSED"));
            assert_eq!(std::fs::read_to_string(path).unwrap(), raw);
        }
        assert!(ScheduledTaskStore::new(root.path()).read().is_err());
    }
}
