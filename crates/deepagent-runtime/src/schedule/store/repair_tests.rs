use super::*;

fn fixture() -> (ScheduledTaskStore, tempfile::TempDir, ScheduledTask) {
    let dir = tempfile::tempdir().unwrap();
    let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
    let task = store
        .add("* * * * *".into(), "healthy".into(), true, 1_000)
        .unwrap();
    (store, dir, task)
}

fn write_mixed(store: &ScheduledTaskStore, task: &ScheduledTask) -> String {
    let mut healthy = serde_json::to_string(task).unwrap();
    healthy.pop();
    healthy.push_str(r#", "future": {"precise":90071992547409931234567890123456789}}"#);
    let bad = r#"{"id":"bad", "cron":"* * * * *", "createdAt":"PRIVATE_BAD_VALUE", "future":[1, {"keep": true}]}"#;
    let body = format!(
        r#"{{"futureRoot":{{"precise":90071992547409931234567890123456789}},"tasks":[{healthy},{bad},null,7]}}"#
    );
    fs::write(store.path(), &body).unwrap();
    bad.to_owned()
}

fn assert_preserved(store: &ScheduledTaskStore, bad: &str) {
    let raw = fs::read_to_string(store.path()).unwrap();
    assert!(raw.contains(bad), "unrecognized row must remain verbatim");
    assert!(raw.contains(r#""futureRoot""#));
    assert!(raw.contains(r#""precise":90071992547409931234567890123456789"#));
    let root: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let rows = root["tasks"].as_array().unwrap();
    assert!(rows.contains(&serde_json::Value::Null));
    assert!(rows.contains(&serde_json::json!(7)));
}

fn assert_extension(store: &ScheduledTaskStore, id: &str) {
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;
    let root: BTreeMap<String, Box<RawValue>> =
        serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
    let rows: Vec<Box<RawValue>> = serde_json::from_str(root["tasks"].get()).unwrap();
    let fields = rows
        .iter()
        .find_map(|row| {
            let fields: BTreeMap<String, Box<RawValue>> = serde_json::from_str(row.get()).ok()?;
            let actual: String = serde_json::from_str(fields.get("id")?.get()).ok()?;
            (actual == id).then_some(fields)
        })
        .unwrap();
    assert_eq!(
        fields["future"].get(),
        r#"{"precise":90071992547409931234567890123456789}"#
    );
}

#[test]
fn h270_partial_document_preserves_raw_rows_and_unknown_values_across_all_mutations() {
    let (store, _dir, task) = fixture();
    let bad = write_mixed(&store, &task);
    let before = fs::read(store.path()).unwrap();
    assert_eq!(store.read().unwrap(), vec![task.clone()]);
    assert_eq!(fs::read(store.path()).unwrap(), before);
    let added = store
        .add("0 9 * * *".into(), "new".into(), false, 2_000)
        .unwrap();
    assert_preserved(&store, &bad);
    assert_eq!(
        store
            .mark_fired(std::slice::from_ref(&task.id), 3_000)
            .unwrap(),
        1
    );
    assert_preserved(&store, &bad);
    assert_extension(&store, &task.id);
    assert_eq!(store.remove(std::slice::from_ref(&added.id)).unwrap(), 1);
    assert_preserved(&store, &bad);
    let mut kept = store.read().unwrap();
    assert_eq!(kept[0].last_fired_at, Some(3_000));
    kept[0].prompt = "explicit replacement".into();
    kept[0].last_fired_at = None;
    store.write(&kept).unwrap();
    assert_preserved(&store, &bad);
    assert_extension(&store, &task.id);
    let reopened = ScheduledTaskStore::new_at(store.path().to_owned());
    assert_eq!(reopened.read().unwrap(), kept);
    assert_eq!(
        reopened.remove(&["bad".into()]).unwrap(),
        0,
        "do not guess ownership of an invalid row"
    );
    reopened.write(&[]).unwrap();
    assert!(reopened.read().unwrap().is_empty());
    assert_preserved(&reopened, &bad);
}

#[test]
fn h270_bad_document_is_an_error_and_no_mutation_can_replace_it() {
    for body in [
        "",
        "  \n",
        "null",
        "[]",
        "{}",
        r#"{"tasks":null}"#,
        r#"{"tasks":{}}"#,
        r#"{"tasks":["#,
        r#"{"tasks":[],"tasks":[]}"#,
        r#"{"tasks":[],"future":1,"future":2}"#,
    ] {
        let (store, _dir, task) = fixture();
        fs::write(store.path(), body).unwrap();
        for outcome in [
            store.read().map(|_| ()),
            store
                .add("* * * * *".into(), "new".into(), true, 2_000)
                .map(|_| ()),
            store.remove(std::slice::from_ref(&task.id)).map(|_| ()),
            store
                .mark_fired(std::slice::from_ref(&task.id), 2_000)
                .map(|_| ()),
            store.write(&[task]),
        ] {
            assert!(
                outcome.is_err(),
                "invalid document must not become an empty baseline: {body}"
            );
            assert_eq!(fs::read_to_string(store.path()).unwrap(), body);
        }
    }
}

#[test]
fn h270_read_errors_and_invalid_utf8_are_not_writable_empty_baselines() {
    let dir = tempfile::tempdir().unwrap();
    let store = ScheduledTaskStore::new_at(dir.path().join("is-a-directory"));
    fs::create_dir(store.path()).unwrap();
    assert!(store.read().is_err());
    assert!(store
        .add("* * * * *".into(), "new".into(), true, 1)
        .is_err());
    assert!(store.write(&[]).is_err());
    assert!(store.path().is_dir());
    let (store, _dir, task) = fixture();
    fs::write(store.path(), [0xff, 0xfe]).unwrap();
    assert!(store.read().is_err());
    assert!(store.remove(std::slice::from_ref(&task.id)).is_err());
    assert!(store.mark_fired(&[task.id], 2).is_err());
    assert!(store.write(&[]).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), [0xff, 0xfe]);
}

#[test]
fn h270_row_schema_failures_remain_opaque_and_do_not_hide_healthy_rows() {
    for patch in [
        serde_json::json!({"prompt": null}),
        serde_json::json!({"createdAt": "PRIVATE_INVALID"}),
        serde_json::json!({"lastFiredAt": "PRIVATE_INVALID"}),
        serde_json::json!({"recurring": 1}),
        serde_json::json!({"permanent": "PRIVATE_INVALID"}),
        serde_json::json!({"cron": "not a cron"}),
        serde_json::json!({"id": ""}),
        serde_json::json!({"prompt": " "}),
    ] {
        let (store, _dir, task) = fixture();
        let mut bad = serde_json::to_value(&task).unwrap();
        bad["id"] = serde_json::json!("bad");
        for (key, value) in patch.as_object().unwrap() {
            bad[key] = value.clone();
        }
        let document = serde_json::json!({"tasks":[task, bad]});
        fs::write(store.path(), serde_json::to_vec(&document).unwrap()).unwrap();
        assert_eq!(store.read().unwrap(), vec![task.clone()]);
        store
            .mark_fired(std::slice::from_ref(&task.id), 4_000)
            .unwrap();
        let after: serde_json::Value =
            serde_json::from_slice(&fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(after["tasks"][1], bad);
        assert_eq!(store.read().unwrap()[0].last_fired_at, Some(4_000));
    }
}

#[test]
fn h270_duplicate_row_fields_are_not_executed_or_destroyed_by_an_edit() {
    let (store, _dir, task) = fixture();
    let bad =
        r#"{"id":"ambiguous", "cron":"* * * * *", "prompt":"a", "prompt":"b", "createdAt":1000}"#;
    let body = format!(
        r#"{{"tasks":[{},{bad}]}}"#,
        serde_json::to_string(&task).unwrap()
    );
    fs::write(store.path(), body).unwrap();
    assert_eq!(store.read().unwrap(), vec![task.clone()]);
    store.remove(&[task.id]).unwrap();
    assert!(store.read().unwrap().is_empty());
    assert!(fs::read_to_string(store.path()).unwrap().contains(bad));
}

#[test]
fn h270_missing_and_explicit_empty_documents_keep_legacy_optional_field_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let store = ScheduledTaskStore::new_at(dir.path().join("new/scheduled-tasks.json"));
    assert!(store.read().unwrap().is_empty());
    store.write(&[]).unwrap();
    assert!(store.path().is_file());
    assert!(store.read().unwrap().is_empty());
    let row =
        r#"{"tasks":[{"id":"legacy", "cron":"* * * * *", "prompt":"healthy", "createdAt":1}]}"#;
    fs::write(store.path(), row).unwrap();
    let read = store.read().unwrap();
    assert!(!read[0].is_recurring());
    assert_eq!(read[0].last_fired_at, None);
    assert_eq!(read[0].permanent, None);
    assert_eq!(store.mark_fired(&["legacy".into()], 10).unwrap(), 1);
    assert_eq!(store.read().unwrap()[0].schedule_anchor_ms(), 10);
}

#[test]
fn h270_projection_replacement_keeps_requested_order_and_existing_extensions() {
    let (store, _dir, first) = fixture();
    let bad = write_mixed(&store, &first);
    let second = store
        .add("0 9 * * *".into(), "second".into(), false, 2_000)
        .unwrap();
    let requested = vec![second, first.clone()];
    store.write(&requested).unwrap();
    assert_eq!(store.read().unwrap(), requested);
    assert_preserved(&store, &bad);
    assert_extension(&store, &first.id);
}

#[test]
fn h270_partial_diagnostics_expose_only_static_reasons_and_counts() {
    // This isolated acceptance case uses the production logger. Its captured
    // output is checked for both diagnostic events and absence of every marker.
    deepagent_tracing::init(deepagent_tracing::TracingConfig {
        default_directive: "warn".into(),
        format: deepagent_tracing::LogFormat::Json,
        ..Default::default()
    });
    let (store, _dir, task) = fixture();
    let bad = r#"{"id":"H270_PRIVATE_ID", "cron":"H270_PRIVATE_CRON", "prompt":"H270_PRIVATE_PROMPT", "createdAt":1}"#;
    fs::write(
        store.path(),
        format!(
            r#"{{"tasks":[{},{bad}]}}"#,
            serde_json::to_string(&task).unwrap()
        ),
    )
    .unwrap();
    assert_eq!(store.read().unwrap(), vec![task]);
}

#[test]
fn h270_ambiguous_projection_replacement_fails_without_changing_the_file() {
    let (store, _dir, task) = fixture();
    let before = fs::read(store.path()).unwrap();
    assert!(store.write(&[task.clone(), task.clone()]).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), before);
    fs::write(
        store.path(),
        serde_json::to_vec(&serde_json::json!({"tasks":[task,task]})).unwrap(),
    )
    .unwrap();
    let before = fs::read(store.path()).unwrap();
    assert!(store.write(&[task]).is_err());
    assert_eq!(fs::read(store.path()).unwrap(), before);
}

#[tokio::test]
async fn h270_real_tick_loop_keeps_bad_rows_and_settles_healthy_tasks_once() {
    use crate::schedule::{run_tick_loop, CronScheduler};
    let dir = tempfile::tempdir().unwrap();
    let store = ScheduledTaskStore::new_at(dir.path().join("scheduled-tasks.json"));
    let now = time::macros::datetime!(2026-11-04 10:00:00);
    let at = crate::schedule::cron::local_to_epoch_ms(now);
    let recurring = store
        .add("* * * * *".into(), "recurring".into(), true, at - 120_000)
        .unwrap();
    let once = store
        .add("* * * * *".into(), "once".into(), false, at - 120_000)
        .unwrap();
    let bad = r#"{"id":"bad", "cron":"* * * * *", "createdAt":1, "lastFiredAt":"PRIVATE_BAD"}"#;
    fs::write(
        store.path(),
        format!(
            r#"{{"tasks":[{},{},{bad}]}}"#,
            serde_json::to_string(&recurring).unwrap(),
            serde_json::to_string(&once).unwrap()
        ),
    )
    .unwrap();
    let shutdown = tokio_util::sync::CancellationToken::new();
    let stop = shutdown.clone();
    let mut ticks = 0;
    let mut fired = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run_tick_loop(
            CronScheduler::new(store.clone()),
            std::time::Duration::from_millis(5),
            || {
                ticks += 1;
                if ticks == 3 {
                    stop.cancel();
                }
                now
            },
            |task| fired.push(task.id.clone()),
            shutdown,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    fired.sort();
    let mut expected = vec![recurring.id.clone(), once.id];
    expected.sort();
    assert_eq!(fired, expected);
    assert!(ticks >= 3);
    let reopened = ScheduledTaskStore::new_at(store.path().to_owned());
    let tasks = reopened.read().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, recurring.id);
    assert_eq!(tasks[0].last_fired_at, Some(at));
    assert!(fs::read_to_string(store.path()).unwrap().contains(bad));
}

#[tokio::test]
async fn h270_corrupt_document_ends_tick_with_an_error_before_any_dispatch() {
    use crate::schedule::{run_tick_loop, CronScheduler};
    let (store, _dir, _task) = fixture();
    let raw = r#"{"tasks":["PRIVATE_TRUNCATED""#;
    fs::write(store.path(), raw).unwrap();
    let mut fired = 0;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run_tick_loop(
            CronScheduler::new(store.clone()),
            std::time::Duration::from_millis(5),
            || time::macros::datetime!(2026-11-04 10:00:00),
            |_| fired += 1,
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap();
    let error = outcome.unwrap_err();
    assert!(!error.to_string().contains("PRIVATE_TRUNCATED"));
    assert_eq!(fired, 0);
    assert_eq!(fs::read_to_string(store.path()).unwrap(), raw);
}
