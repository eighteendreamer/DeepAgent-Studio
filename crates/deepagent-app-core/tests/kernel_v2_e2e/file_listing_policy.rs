//! File discovery must not expose names hidden from direct reads.

use super::*;
use deepagent_app_core::SandboxMode;
use deepagent_models::ModelRole;

struct ListingModel {
    operation: &'static str,
    calls: AtomicUsize,
    observed: Mutex<Option<(bool, bool)>>,
}

fn paired_output(body: &serde_json::Value, call_id: &str) -> serde_json::Value {
    let output = &body["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
        .expect("the next request must carry the paired tool output")["output"];
    serde_json::from_str(
        output
            .as_str()
            .expect("tool output must be serialized JSON"),
    )
    .unwrap()
}

fn listing_names(envelope: &serde_json::Value, operation: &str) -> Vec<String> {
    match operation {
        "list_dir" => envelope["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_owned())
            .collect(),
        "glob" => envelope["result"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|path| path.as_str().unwrap().to_owned())
            .collect(),
        _ => panic!("unknown listing operation"),
    }
}

#[test]
fn listing_observer_counts_only_result_names() {
    for (operation, result) in [
        ("list_dir", serde_json::json!({"entries":[]})),
        ("glob", serde_json::json!({"matches":[]})),
    ] {
        let echo = serde_json::json!({"result":result,"path":".env","error":"visible.txt"});
        assert!(listing_names(&echo, operation).is_empty());
    }
    assert_eq!(
        listing_names(
            &serde_json::json!({"result":{"entries":[{"name":".env"}]}}),
            "list_dir"
        ),
        [".env"]
    );
    assert_eq!(
        listing_names(
            &serde_json::json!({"result":{"matches":["visible.txt"]}}),
            "glob"
        ),
        ["visible.txt"]
    );
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
                assert_eq!(denied["status"], "error");
                assert!(denied["error"].to_string().contains("sensitive"));
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
                assert_eq!(listing["status"], "ok");
                let names = listing_names(&listing, self.operation);
                for denied in ["credentials", ".ENV.Local", "secret-store"] {
                    assert!(!names.iter().any(|name| name == denied), "{listing}");
                }
                assert!(names.iter().any(|name| name == ".hidden"));
                *self.observed.lock().unwrap() = Some((
                    names.iter().any(|name| name == ".env"),
                    names.iter().any(|name| name == "visible.txt"),
                ));
                response_text("Offline file-listing audit completed.")
            }
            _ => panic!("unexpected extra model request in file-listing audit"),
        };
        emit_responses_events(sink, &events)
    }
}

async fn observe_listing(operation: &'static str, mode: Option<SandboxMode>) -> (bool, bool) {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join(".env"), "AUDIT_LISTING_HIDDEN_3d8c\n").unwrap();
    std::fs::write(workspace.path().join("visible.txt"), "visible\n").unwrap();
    std::fs::write(
        workspace.path().join(".hidden"),
        "ordinary hidden fixture\n",
    )
    .unwrap();
    for name in ["credentials", ".ENV.Local"] {
        std::fs::write(workspace.path().join(name), "synthetic policy fixture\n").unwrap();
    }
    std::fs::create_dir(workspace.path().join("secret-store")).unwrap();
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
    if let Some(mode) = mode {
        settings.set_sandbox_mode(mode).unwrap();
    }
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
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let observed = model.observed.lock().unwrap().unwrap();
    eprintln!(
        "file listing audit: operation={operation}, mode={mode:?}, direct_read_denied=true, listing_ok=true, hidden_name_sent={}, visible_name_sent={}",
        observed.0, observed.1
    );
    observed
}

#[tokio::test]
async fn audit_file_discovery_does_not_send_denied_file_names_to_model() {
    let list_name = observe_listing("list_dir", None).await;
    let glob_name = observe_listing("glob", None).await;
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
    let (_hidden, visible) = observe_listing("glob", None).await;
    assert!(
        visible,
        "the audit fixture must observe a real listing result"
    );
}

#[tokio::test]
async fn file_discovery_sensitive_policy_is_preserved_across_runtime_profiles() {
    for mode in [
        SandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite,
        SandboxMode::FullAccess,
    ] {
        for operation in ["list_dir", "glob"] {
            let (hidden, visible) = observe_listing(operation, Some(mode)).await;
            assert!(!hidden && visible, "{operation}/{mode:?}");
        }
    }
}
