//! A read-before-write guard must validate the version actually seen by the model.

use super::*;
use deepagent_models::ModelRole;

const INITIAL: &str = "owner=audit-initial\nvalue=old\n";
const EXTERNAL: &str = "owner=audit-external-editor-saved\nvalue=old\n";

struct ExternalEditorModel {
    path: PathBuf,
    external_change: bool,
    refresh_read: bool,
    calls: AtomicUsize,
}

fn output_for(body: &serde_json::Value, call_id: &str) -> String {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .expect("the actual model request must contain the paired tool result")["output"]
        .to_string()
}

#[async_trait]
impl HttpTransport for ExternalEditorModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let write = || {
            let seen = if self.refresh_read { EXTERNAL } else { INITIAL };
            response_function_call(
                "audit-version-write",
                "write_file",
                serde_json::json!({
                    "path":"shared.txt", "content":seen.replace("value=old", "value=new")
                }),
            )
        };
        let events = match call {
            0 => response_function_call(
                "audit-version-read",
                "read_file",
                serde_json::json!({"path":"shared.txt"}),
            ),
            1 => {
                let output = output_for(&body, "audit-version-read");
                assert!(output.contains("owner=audit-initial"));
                assert!(!output.contains("owner=audit-external-editor-saved"));
                assert_eq!(std::fs::read_to_string(&self.path).unwrap(), INITIAL);
                if self.external_change {
                    // A separate actor saves after the read result has reached
                    // the model, before it issues the write. No timing race.
                    std::fs::write(&self.path, EXTERNAL).unwrap();
                    assert_eq!(std::fs::read_to_string(&self.path).unwrap(), EXTERNAL);
                }
                if self.refresh_read {
                    response_function_call(
                        "audit-version-refresh",
                        "read_file",
                        serde_json::json!({"path":"shared.txt"}),
                    )
                } else {
                    write()
                }
            }
            2 if self.refresh_read => {
                assert!(output_for(&body, "audit-version-refresh")
                    .contains("owner=audit-external-editor-saved"));
                write()
            }
            2 | 3 => {
                let _paired_write_result = output_for(&body, "audit-version-write");
                response_text("Audit completed; inspect the recorded tool outcome.")
            }
            _ => panic!("unexpected extra model request in file-version audit"),
        };
        emit_responses_events(sink, &events)
    }
}

struct Observation {
    write_ok: bool,
    disk: String,
}

async fn observe_write(external_change: bool, refresh_read: bool) -> Observation {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("shared.txt");
    std::fs::write(&path, INITIAL).unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings
        .initialize("offline-audit-placeholder")
        .await
        .unwrap();
    for role in [ModelRole::Chat, ModelRole::Reasoner] {
        settings.set_model(role, "deepseek-flash").unwrap();
    }
    let model = Arc::new(ExternalEditorModel {
        path: path.clone(),
        external_change,
        refresh_read,
        calls: AtomicUsize::new(0),
    });
    let chat = ChatService::new(db, settings, model.clone(), workspace.path());
    chat.run_in_session(
        "Read shared.txt and change value=old to value=new. Preserve other edits.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-file-version"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        if refresh_read { 4 } else { 3 }
    );
    let events = chat.run_events("audit-file-version", None).unwrap();
    let writes: Vec<_> = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_completed" && event.data["call_id"] == "audit-version-write"
        })
        .collect();
    assert_eq!(writes.len(), 1);
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let observed = Observation {
        write_ok: writes[0].data["ok"].as_bool().unwrap(),
        disk: std::fs::read_to_string(path).unwrap(),
    };
    eprintln!(
        "file-version audit: external={external_change}, refreshed={refresh_read}, write_ok={}, external_owner_preserved={}, model_value_written={}",
        observed.write_ok,
        observed.disk.contains("owner=audit-external-editor-saved"),
        observed.disk.contains("value=new"),
    );
    observed
}

#[tokio::test]
async fn unchanged_file_write_and_refreshed_external_edit_are_valid() {
    let unchanged = observe_write(false, false).await;
    assert!(unchanged.write_ok);
    assert_eq!(unchanged.disk, INITIAL.replace("value=old", "value=new"));
    let refreshed = observe_write(true, true).await;
    assert!(refreshed.write_ok);
    assert_eq!(refreshed.disk, EXTERNAL.replace("value=old", "value=new"));
}

#[tokio::test]
#[ignore = "audit: a stale read must not authorize clobbering an external file edit"]
async fn audit_stale_read_rejects_overwriting_external_file_edit() {
    let observed = observe_write(true, false).await;
    assert!(
        !observed.write_ok && observed.disk == EXTERNAL,
        "write_file accepted an obsolete read and replaced the external editor's saved content"
    );
}
