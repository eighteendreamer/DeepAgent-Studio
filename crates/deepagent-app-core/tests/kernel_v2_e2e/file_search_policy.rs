//! Search must not disclose content that the same run cannot read directly.

use super::*;
use deepagent_models::ModelRole;

const PUBLIC: &str = "AUDIT_SEARCH_PUBLIC_493d";
const PROTECTED: &str = "AUDIT_SEARCH_PROTECTED_493d";

#[derive(Debug, Clone, Copy)]
struct SearchObservation {
    public_sent: bool,
    protected_sent: bool,
}

struct SearchPolicyModel {
    protected_path: String,
    pattern: Option<String>,
    calls: AtomicUsize,
    observed: Mutex<Option<SearchObservation>>,
}

#[async_trait]
impl HttpTransport for SearchPolicyModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let output = |call_id: &str| {
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id)
                .expect("the next request must contain the actual paired tool result")["output"]
                .to_string()
        };
        let events = match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => response_function_call(
                "audit-protected-read",
                "read_file",
                serde_json::json!({"path": self.protected_path}),
            ),
            1 => {
                let denied = output("audit-protected-read");
                assert!(denied.contains("sensitive"));
                assert!(!denied.contains(PROTECTED));
                let mut args = serde_json::json!({"query":"AUDIT_SEARCH"});
                if let Some(pattern) = &self.pattern {
                    args["glob"] = pattern.clone().into();
                }
                response_function_call("audit-protected-grep", "grep", args)
            }
            2 => {
                let searched = output("audit-protected-grep");
                *self.observed.lock().unwrap() = Some(SearchObservation {
                    public_sent: searched.contains(PUBLIC),
                    protected_sent: searched.contains(PROTECTED),
                });
                response_text("Offline search-policy audit completed.")
            }
            _ => panic!("unexpected extra model request in search-policy audit"),
        };
        emit_responses_events(sink, &events)
    }
}

async fn observe_search(protected_path: &str, pattern: Option<&str>) -> SearchObservation {
    let workspace = tempfile::tempdir().unwrap();
    let target = workspace.path().join(protected_path);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, format!("{PROTECTED}=fixture-only-no-credential\n")).unwrap();
    std::fs::write(workspace.path().join("safe.txt"), format!("{PUBLIC}\n")).unwrap();
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
    let model = Arc::new(SearchPolicyModel {
        protected_path: protected_path.into(),
        pattern: pattern.map(str::to_owned),
        calls: AtomicUsize::new(0),
        observed: Mutex::new(None),
    });
    let chat = ChatService::new(db, settings, model.clone(), workspace.path());
    chat.run_in_session(
        "Inspect the fixture file and search for AUDIT_SEARCH in this temporary workspace.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-file-search-policy"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 3);
    let events = chat.run_events("audit-file-search-policy", None).unwrap();
    let denied: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "tool_blocked" && event.data["name"] == "read_file")
        .collect();
    assert_eq!(denied.len(), 1);
    assert!(denied[0].data["reason"]
        .as_str()
        .unwrap()
        .contains("sensitive"));
    assert_eq!(denied[0].data["needs_approval"], false);
    let completed: Vec<_> = events
        .iter()
        .filter(|event| {
            event.event_type == "tool_completed" && event.data["call_id"] == "audit-protected-grep"
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
        "file search-policy audit: path={protected_path}, pattern={pattern:?}, read_denied=true, grep_ok=true, public_sent={}, protected_sent={}",
        observed.public_sent,
        observed.protected_sent,
    );
    observed
}

#[tokio::test]
async fn ordinary_search_with_restricted_glob_keeps_protected_fixture_out() {
    let observed = observe_search(".env", Some("*.txt")).await;
    assert!(observed.public_sent);
    assert!(!observed.protected_sent);
}

#[tokio::test]
#[ignore = "audit: grep must honor the same sensitive-file deny policy as read_file"]
async fn audit_grep_does_not_send_denied_file_contents_to_model() {
    let mut observations = Vec::new();
    for (path, pattern) in [
        (".env", None),
        ("credentials", None),
        ("certs/server.key", Some("**/*.key")),
    ] {
        observations.push(observe_search(path, pattern).await);
    }
    assert!(observations[0].public_sent && observations[1].public_sent);
    assert!(
        observations.iter().all(|observed| !observed.protected_sent),
        "grep sent content that read_file denied: {observations:?}"
    );
}
