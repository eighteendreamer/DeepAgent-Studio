//! H66: child model overrides share the adapter, not the parent's hosted form.
use super::*;
use crate::{MemorySecretStore, SettingsService};
use deepagent_models::{transport::*, MockTransport};
use deepagent_persistence::{run_store::RunStore, subagent_store::SubagentRunStore};
use serde_json::{json, Value};
use std::sync::Mutex;

#[derive(Default)]
struct Capture(Mutex<Vec<Value>>);
#[async_trait::async_trait]
impl HttpTransport for Capture {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let model = body["model"].as_str().unwrap().to_owned();
        self.0.lock().unwrap().push(body);
        for event in crate::responses_fixture::completed(
            "h66-child",
            &model,
            crate::responses_fixture::text("answer", "synthetic child answer"),
            Value::Null,
        ) {
            if sink.on_event(&event)? {
                break;
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn h66_tracked_child_override_uses_its_own_search_capability_and_respects_tool_filter() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.sqlite");
    let db = Arc::new(Database::open(&path).unwrap());
    let settings = Arc::new(SettingsService::new(
        db.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"},{"id":"future-pro"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "synthetic")
        .await
        .unwrap();
    settings
        .harness_models()
        .select_model("deepseek", "deepseek-v4-pro")
        .unwrap();
    let capability =
        deepagent_models::ModelCapabilityResolver::new().resolve_model_id("deepseek-v4-pro");
    db.save_harness_models(
        "deepseek",
        &[deepagent_persistence::harness_registry::HarnessModel {
            provider_id: "deepseek".into(),
            model_name: "deepseek-v4-pro".into(),
            display_name: "deepseek-v4-pro".into(),
            protocol: None,
            context_window: capability.context_window as i64,
            output_limit: capability.max_output_tokens as i64,
            capabilities: serde_json::to_string(&capability).unwrap(),
            active: true,
        }],
    )
    .unwrap();
    let transport = Arc::new(Capture::default());
    let host = ChatService::new(db.clone(), settings.clone(), transport.clone(), root.path());
    let (registry, _) = host
        .build_registry(
            root.path(),
            deepagent_builtins::FsAccess::Workspace,
            None,
            None,
            None,
            false,
        )
        .unwrap();
    let mut runner =
        super::tests::audit_child_runner_with_db(root.path(), transport.clone(), db.clone());
    runner.client =
        crate::model_runtime::select_run_model(&settings, transport.clone(), None, None, None)
            .unwrap()
            .client;
    runner.model = "deepseek-v4-pro".into();
    runner.registry = Arc::new(registry);
    runner.host = host;
    for (index, (model, include_search)) in [
        ("deepseek-flash", true),
        ("deepseek-v4-pro", true),
        ("future-pro", true),
        ("deepseek-v4-pro", false),
    ]
    .into_iter()
    .enumerate()
    {
        let request = serde_json::from_value(json!({
            "description":"synthetic search capability", "prompt":"Reply briefly.",
            "model":model, "isolation":"shared",
            "allowed_tools": if include_search { vec!["web_search"] } else { vec!["read_file"] },
        }))
        .unwrap();
        let id = format!("h66-child-{index}");
        let persist = runner.begin_tracking(&id, &request, false, None).unwrap();
        assert!(persist);
        runner
            .run_tracked(
                id.clone(),
                SubagentInput {
                    request,
                    history: vec![],
                },
                None,
                None,
                persist,
                false,
            )
            .await
            .unwrap();
        let requests = transport.0.lock().unwrap();
        let wire = &requests[index];
        assert_eq!(wire["model"], model);
        let tools = wire["tools"].as_array().unwrap();
        assert_eq!(
            tools
                .iter()
                .any(|t| t["type"] == "function" && t["name"] == "web_search"),
            include_search
        );
        assert!(!tools.iter().any(|t| t["type"] == "web_search"));
        let record = SubagentRunStore::new(&db).get(&id).unwrap().unwrap();
        assert_eq!(record.state, "succeeded");
        let transcript: Value =
            serde_json::from_slice(&std::fs::read(record.transcript_path.unwrap()).unwrap())
                .unwrap();
        assert_eq!(transcript["request"]["model"], model);
    }
    let events = RunStore::new(&db)
        .events_after("audit-parent", None)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "subagent_completed")
            .count(),
        4
    );
    let children = SubagentRunStore::new(&db)
        .list_for_parent("audit-parent")
        .unwrap();
    drop(runner);
    drop(settings);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    assert_eq!(
        RunStore::new(&reopened)
            .events_after("audit-parent", None)
            .unwrap(),
        events
    );
    assert_eq!(
        SubagentRunStore::new(&reopened)
            .list_for_parent("audit-parent")
            .unwrap(),
        children
    );
    assert_eq!(transport.0.lock().unwrap().len(), 4);
}
