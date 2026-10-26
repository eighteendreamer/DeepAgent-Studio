//! H-36: published result bytes, durable metadata and subsequent model reads.

use super::*;
use deepagent_persistence::artifact_store::{ArtifactRecord, ArtifactStore};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
struct Outputs(Arc<Mutex<BTreeMap<String, serde_json::Value>>>);

struct ResultReplay {
    inner: ReplayTransport,
    outputs: Outputs,
}

#[async_trait]
impl HttpTransport for ResultReplay {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body)?;
        for item in body["input"].as_array().unwrap() {
            if item["type"] != "function_call_output" {
                continue;
            }
            let output = match item["output"].as_str() {
                Some(text) => serde_json::from_str(text)?,
                None => item["output"].clone(),
            };
            if let Some(path) = output["result"]["saved_path"].as_str() {
                assert!(
                    std::path::Path::new(path).is_file(),
                    "published result must already exist at the actual next model request"
                );
            }
            self.outputs
                .0
                .lock()
                .unwrap()
                .insert(item["call_id"].as_str().unwrap().to_owned(), output);
        }
        self.inner.stream(request, sink).await
    }
}

async fn fixture_settings(db: Arc<Database>) -> Arc<SettingsService> {
    let settings = Arc::new(SettingsService::new(
        db,
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("sk-result-fixture-placeholder")
        .await
        .unwrap();
    settings
}

fn write_large(root: &std::path::Path, name: &str, marker: &str) {
    std::fs::write(
        root.join(name),
        format!(
            "{}\n{marker}",
            ("fixture-row-".repeat(20) + "\n").repeat(200)
        ),
    )
    .unwrap();
}

fn read_script(calls: &[(&str, serde_json::Value)]) -> Vec<Vec<String>> {
    let mut batch = Vec::new();
    for (call_id, arguments) in calls {
        let mut events = response_function_call(call_id, "read_file", arguments.clone());
        events.pop();
        batch.extend(events);
    }
    batch.extend(response_text(""));
    vec![batch, response_text("done")]
}

fn chat_for(
    db: Arc<Database>,
    settings: Arc<SettingsService>,
    root: &std::path::Path,
    calls: &[(&str, serde_json::Value)],
    outputs: &Outputs,
) -> ChatService {
    ChatService::new(
        db,
        settings,
        Arc::new(ResultReplay {
            inner: ReplayTransport::new(read_script(calls)),
            outputs: outputs.clone(),
        }),
        root,
    )
}

fn assert_artifacts(db: &Database, run_id: &str, expected_count: usize) -> Vec<ArtifactRecord> {
    let records = ArtifactStore::new(db).list_for_run(run_id).unwrap();
    assert_eq!(records.len(), expected_count);
    let unique = records
        .iter()
        .map(|record| &record.path)
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(unique.len(), expected_count);
    for record in &records {
        let bytes = std::fs::read(&record.path).unwrap();
        assert_eq!(record.byte_size, bytes.len() as i64);
        assert_eq!(
            record.digest.as_deref(),
            Some(format!("sha256:{:x}", Sha256::digest(&bytes)).as_str())
        );
        assert_eq!(
            record.id,
            std::path::Path::new(&record.path)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
        );
    }
    records
}

#[tokio::test]
async fn parallel_results_remain_indexed_after_reopen_and_actual_followup_read() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("parallel.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let settings = fixture_settings(db.clone()).await;
    for (file, marker) in [
        ("a.txt", "TAIL_A"),
        ("b.txt", "TAIL_B"),
        ("c.txt", "TAIL_C"),
    ] {
        write_large(root.path(), file, marker);
    }
    let outputs = Outputs::default();
    let chat = chat_for(
        db.clone(),
        settings,
        root.path(),
        &[
            ("result-a", serde_json::json!({"path":"a.txt"})),
            ("result-b", serde_json::json!({"path":"b.txt"})),
            ("result-c", serde_json::json!({"path":"c.txt"})),
        ],
        &outputs,
    );
    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = order.clone();
    let session_id = chat
        .run_in_session(
            "Read the three fixture files.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some("result-parallel"),
            move |event| match event {
                deepagent_runtime::RuntimeEvent::ToolStarted { call_id, .. }
                    if call_id.starts_with("result-") =>
                {
                    seen.lock().unwrap().push(true)
                }
                deepagent_runtime::RuntimeEvent::ToolCompleted { call_id, .. }
                    if call_id.starts_with("result-") =>
                {
                    seen.lock().unwrap().push(false)
                }
                _ => {}
            },
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(
        &order.lock().unwrap()[..3],
        &[true, true, true],
        "all three reads must enter the actual parallel branch before any completion"
    );
    let records = assert_artifacts(&db, "result-parallel", 3);
    let before = records
        .iter()
        .map(|record| std::fs::read(&record.path).unwrap())
        .collect::<Vec<_>>();
    for (call, marker) in [
        ("result-a", "TAIL_A"),
        ("result-b", "TAIL_B"),
        ("result-c", "TAIL_C"),
    ] {
        let recorded = outputs.0.lock().unwrap()[call].clone();
        assert_eq!(recorded["status"], "ok");
        assert!(
            std::fs::read_to_string(recorded["result"]["saved_path"].as_str().unwrap())
                .unwrap()
                .contains(marker)
        );
        assert!(!recorded["result"]["preview"]
            .as_str()
            .unwrap()
            .contains(marker));
    }
    drop(chat);
    drop(db);
    let reopened = Arc::new(Database::open(&db_path).unwrap());
    assert_eq!(assert_artifacts(&reopened, "result-parallel", 3), records);
    let full = String::from_utf8(before[0].clone()).unwrap();
    let (line, expected) = full
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("\"message\":"))
        .unwrap();
    let followup = chat_for(
        reopened.clone(),
        fixture_settings(reopened.clone()).await,
        root.path(),
        &[(
            "result-followup",
            serde_json::json!({"path":records[0].path,"offset":line+1,"limit":1}),
        )],
        &outputs,
    );
    let resumed = followup
        .run_in_session(
            "Read a retained result metadata line.",
            Some(&session_id),
            None,
            None,
            vec![],
            None,
            false,
            Some("result-followup-run"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    assert_eq!(resumed, session_id);
    let result = outputs.0.lock().unwrap()["result-followup"].clone();
    assert_eq!(result["status"], "ok", "followup output: {result}");
    assert!(result["result"]["content"]
        .as_str()
        .unwrap()
        .contains(expected.trim()));
    for (record, bytes) in records.iter().zip(before) {
        assert_eq!(std::fs::read(&record.path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn separate_runs_with_same_provider_call_id_keep_both_model_visible_results() {
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("reuse.sqlite")).unwrap());
    let settings = fixture_settings(db.clone()).await;
    write_large(root.path(), "a.txt", "FIRST_RUN_TAIL");
    write_large(root.path(), "b.txt", "SECOND_RUN_TAIL");
    let outputs = Outputs::default();
    let mut records = Vec::new();
    let mut sessions = Vec::new();
    for (run_id, file) in [
        ("result-first-run", "a.txt"),
        ("result-second-run", "b.txt"),
    ] {
        let chat = chat_for(
            db.clone(),
            settings.clone(),
            root.path(),
            &[("same-provider-call", serde_json::json!({"path":file}))],
            &outputs,
        );
        sessions.push(
            chat.run_in_session(
                "Read fixture.",
                None,
                None,
                None,
                vec![],
                None,
                false,
                Some(run_id),
                |_| {},
                |_| {},
            )
            .await
            .unwrap(),
        );
        records.push(assert_artifacts(&db, run_id, 1).remove(0));
    }
    assert_ne!(sessions[0], sessions[1]);
    assert_ne!(records[0].id, records[1].id);
    assert_ne!(records[0].path, records[1].path);
    for (record, marker) in records.iter().zip(["FIRST_RUN_TAIL", "SECOND_RUN_TAIL"]) {
        assert!(std::fs::read_to_string(&record.path)
            .unwrap()
            .contains(marker));
    }
}

#[tokio::test]
async fn external_result_storage_does_not_implicitly_expand_file_read_permission() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let db = Arc::new(Database::open(root.path().join("external.sqlite")).unwrap());
    write_large(&workspace, "source.txt", "EXTERNAL_RESULT_TAIL");
    let outputs = Outputs::default();
    let chat = chat_for(
        db.clone(),
        fixture_settings(db.clone()).await,
        &workspace,
        &[("external-save", serde_json::json!({"path":"source.txt"}))],
        &outputs,
    )
    .with_tool_results_dir(root.path().join("managed-results"));
    let session = chat
        .run_in_session(
            "Read fixture.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some("external-save-run"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let record = assert_artifacts(&db, "external-save-run", 1).remove(0);
    let bytes = std::fs::read(&record.path).unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("EXTERNAL_RESULT_TAIL"));
    let followup = chat_for(
        db.clone(),
        fixture_settings(db.clone()).await,
        &workspace,
        &[(
            "external-read",
            serde_json::json!({"path":record.path,"offset":1,"limit":1}),
        )],
        &outputs,
    );
    followup
        .run_in_session(
            "Attempt confined read.",
            Some(&session),
            None,
            None,
            vec![],
            None,
            false,
            Some("external-read-run"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let result = outputs.0.lock().unwrap()["external-read"].clone();
    assert_eq!(
        result["status"], "error",
        "retention must not grant read-anywhere: {result}"
    );
    assert!(result
        .to_string()
        .contains("blocked by the current sandbox mode"));
    assert_eq!(std::fs::read(&record.path).unwrap(), bytes);
}

#[tokio::test]
async fn archive_gc_preserves_live_and_recycled_fork_then_reclaims_last_reference() {
    use deepagent_app_core::{AppService, ArchiveService};
    use deepagent_persistence::event_store::EventStore;
    let root = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(root.path().join("gc.sqlite")).unwrap());
    write_large(root.path(), "source.txt", "RETAINED_FORK_TAIL");
    let outputs = Outputs::default();
    let chat = chat_for(
        db.clone(),
        fixture_settings(db.clone()).await,
        root.path(),
        &[("retained-call", serde_json::json!({"path":"source.txt"}))],
        &outputs,
    );
    let source = chat
        .run_in_session(
            "Read fixture.",
            None,
            None,
            None,
            vec![],
            None,
            false,
            Some("retained-owner"),
            |_| {},
            |_| {},
        )
        .await
        .unwrap();
    let record = assert_artifacts(&db, "retained-owner", 1).remove(0);
    let original = std::fs::read(&record.path).unwrap();
    let app = AppService::from_shared(db.clone());
    let source_id = source.parse().unwrap();
    let last = EventStore::new(&db)
        .load_session(source_id)
        .unwrap()
        .last()
        .unwrap()
        .sequence;
    let fork = app.fork_session(&source, last).unwrap().new_session_id;
    let archive = ArchiveService::new(db.clone());
    archive.archive_session(&source).unwrap();
    archive.delete_archived_session(&source).unwrap();
    assert_eq!(
        archive.gc_deleted_sessions(30).unwrap(),
        0,
        "within recycle retention"
    );
    assert_eq!(std::fs::read(&record.path).unwrap(), original);
    assert_eq!(archive.gc_deleted_sessions(0).unwrap(), 1);
    assert!(ArtifactStore::new(&db)
        .get(&record.id)
        .unwrap()
        .unwrap()
        .run_id
        .is_none());
    assert_eq!(
        std::fs::read(&record.path).unwrap(),
        original,
        "live fork protects detached result"
    );
    archive.archive_session(&fork).unwrap();
    archive.delete_archived_session(&fork).unwrap();
    assert_eq!(archive.gc_deleted_sessions(30).unwrap(), 0);
    assert_eq!(
        std::fs::read(&record.path).unwrap(),
        original,
        "recycled fork still protects result"
    );
    archive.unarchive_session(&fork).unwrap();
    assert_eq!(archive.gc_deleted_sessions(0).unwrap(), 0);
    assert_eq!(std::fs::read(&record.path).unwrap(), original);
    archive.archive_session(&fork).unwrap();
    archive.delete_archived_session(&fork).unwrap();
    assert_eq!(archive.gc_deleted_sessions(0).unwrap(), 1);
    assert!(!std::path::Path::new(&record.path).exists());
    assert!(ArtifactStore::new(&db).get(&record.id).unwrap().is_none());
    assert_eq!(archive.gc_deleted_sessions(0).unwrap(), 0);
}
