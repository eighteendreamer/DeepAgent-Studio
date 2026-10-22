//! File discovery must not expose names hidden from direct reads.

use super::*;
use deepagent_models::ModelRole;

struct ListingModel {
    operation: &'static str,
    calls: AtomicUsize,
    observed: Mutex<Option<(bool, bool)>>,
}

fn paired_output(body: &serde_json::Value, call_id: &str) -> String {
    body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .expect("the next request must carry the paired tool output")["output"]
        .to_string()
}

#[async_trait]
impl HttpTransport for ListingModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => response_function_call(
                "audit-listing-direct-read",
                "read_file",
                serde_json::json!({"path":".env"}),
            ),
            1 => {
                let denied = paired_output(&body, "audit-listing-direct-read");
                assert!(denied.contains("sensitive"));
                match self.operation {
                    "list_dir" => response_function_call(
                        "audit-listing-operation",
                        "list_dir",
                        serde_json::json!({}),
                    ),
                    "glob" => response_function_call(
                        "audit-listing-operation",
                        "glob",
                        serde_json::json!({"pattern":"**"}),
                    ),
                    _ => panic!("unknown listing operation"),
                }
            }
            2 => {
                let listing = paired_output(&body, "audit-listing-operation");
                *self.observed.lock().unwrap() =
                    Some((listing.contains(".env"), listing.contains("visible.txt")));
                response_text("Offline file-listing audit completed.")
            }
            _ => panic!("unexpected extra model request in file-listing audit"),
        };
        emit_responses_events(sink, &events)
    }
}

async fn observe_listing(operation: &'static str) -> (bool, bool) {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join(".env"), "AUDIT_LISTING_HIDDEN_3d8c\n").unwrap();
    std::fs::write(workspace.path().join("visible.txt"), "visible\n").unwrap();
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
    let model = Arc::new(ListingModel {
        operation,
        calls: AtomicUsize::new(0),
        observed: Mutex::new(None),
    });
    let chat = ChatService::new(db, settings, model.clone(), workspace.path());
    chat.run_in_session(
        "Inspect this temporary workspace and report the matching file names.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-file-listing-policy"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 3);
    let events = chat.run_events("audit-file-listing-policy", None).unwrap();
    let blocked = events
        .iter()
        .filter(|event| event.event_type == "tool_blocked" && event.data["name"] == "read_file")
        .count();
    assert_eq!(blocked, 1);
    let completed: Vec<_> = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_completed"
                && event.data["call_id"] == "audit-listing-operation"
        })
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].data["ok"], true);
    let observed = model.observed.lock().unwrap().unwrap();
    eprintln!(
        "file listing audit: operation={operation}, direct_read_denied=true, listing_ok=true, hidden_name_sent={}, visible_name_sent={}",
        observed.0, observed.1
    );
    observed
}

#[tokio::test]
#[ignore = "audit: glob and list_dir must hide names denied by direct read policy"]
async fn audit_file_discovery_does_not_send_denied_file_names_to_model() {
    let list_name = observe_listing("list_dir").await;
    let glob_name = observe_listing("glob").await;
    assert!(list_name.1 && glob_name.1, "ordinary file name disappeared");
    assert!(
        !list_name.0 && !glob_name.0,
        "denied .env name leaked through discovery"
    );
}

#[tokio::test]
async fn file_discovery_control_reports_visible_file() {
    // The control is the existing production test surface: discovery remains
    // usable for ordinary files; this fixture only ensures the probe is not
    // passing because all listing output was discarded.
    let (_hidden, visible) = observe_listing("glob").await;
    assert!(
        visible,
        "the audit fixture must observe a real listing result"
    );
}
