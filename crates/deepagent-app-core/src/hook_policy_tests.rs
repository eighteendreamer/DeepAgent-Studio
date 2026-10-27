//! Exercise the real assembly/agent runtime without relying on the cloned
//! file tool's own confinement to mask a missing policy projection.

use super::*;
use deepagent_context::{ConfigLayer, MANAGED_PRECEDENCE, USER_DEEPAGENT_PRECEDENCE};
use deepagent_core::id::SessionId;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{MockTransport, ModelConfig};
use deepagent_persistence::Database;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[path = "../tests/kernel_v2_e2e/responses_fixture.rs"]
mod responses_fixture;

const CANARY: &str = "SYNTHETIC_HOOK_POLICY_BODY";

struct ParentSessionHook(Arc<AtomicUsize>);

#[async_trait]
impl Hook for ParentSessionHook {
    fn name(&self) -> &str {
        "parent_session_bookkeeping"
    }

    async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(HookOutcome::Continue)
    }
}

struct ForcedRead {
    path: String,
    requests: Mutex<Vec<Value>>,
}

#[async_trait]
impl HttpTransport for ForcedRead {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: Value = serde_json::from_str(&request.body)?;
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(body);
            requests.len() - 1
        };
        let item = match index {
            // Deliberately request even if a capability hid the schema.
            0 => responses_fixture::call("forced-read", "read_file", json!({"path":self.path})),
            1 => responses_fixture::text("decision", r#"{"ok":true}"#),
            _ => panic!("extension callback recursed into the hook runtime"),
        };
        for event in responses_fixture::completed(
            &format!("hook-{index}"),
            "deepseek-flash",
            item,
            json!({"input_tokens":10,"output_tokens":5,"total_tokens":15}),
        ) {
            sink.on_event(&event)?;
        }
        Ok(())
    }
}

async fn assembled_read(
    ui_rules: PermissionRules,
    layers: Vec<ConfigLayer>,
    permissions: PermissionSet,
    outside: bool,
) -> (Value, bool, bool) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    let path = if outside {
        temporary.path().join("outside.txt")
    } else {
        root.join("fixture.txt")
    };
    std::fs::write(&path, CANARY).unwrap();
    let settings = SettingsService::new(
        Arc::new(Database::open_in_memory().unwrap()),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(crate::MemorySecretStore::new()),
    );
    settings.initialize("offline-policy-fixture").await.unwrap();
    settings.set_permission_rules(ui_rules).unwrap();
    // Match ALL tools: passing the extension registry into the child would
    // recursively invoke this action on its forced read.
    settings.set_hooks_json(&json!({"hooks":{"PreToolUse":[{
        "matcher":".*", "hooks":[{"type":"agent","prompt":"offline policy check","timeout":5}]
    }]}}).to_string()).unwrap();
    let transport = Arc::new(ForcedRead {
        path: path.to_string_lossy().into_owned(),
        requests: Mutex::new(Vec::new()),
    });
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(deepagent_builtins::ReadFileTool::new(
            WorkspaceRoot::new(&root).with_access(deepagent_builtins::FsAccess::Full),
        )))
        .unwrap();
    let overlay = RunConfigOverlay {
        value: json!({}),
        sources: Vec::new(),
        layers,
        errors: Vec::new(),
    };
    let sink: Arc<dyn RuntimeEventSink> = Arc::new(deepagent_runtime::NullEventSink);
    let parent_calls = Arc::new(AtomicUsize::new(0));
    let assembled = assemble_run_hooks(HookAssemblyRequest {
        settings: &settings,
        run_config: &overlay,
        project_hooks: None,
        plugin_projection: None,
        root: &root,
        sink,
        client: Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("offline"),
        )),
        model: "deepseek-flash".into(),
        reasoning_effort: ReasoningEffort::None,
        mcp: None,
        registry: &registry,
        permissions,
        plan: deepagent_builtins::PlanMode::new(),
        office_skill_guard: Some(Arc::new(ParentSessionHook(parent_calls.clone()))),
        access: deepagent_builtins::FsAccess::Workspace,
        bash_allow: Vec::new(),
        bash_full_access: false,
        is_trusted: false,
        project_hook_trust: Arc::new(ProjectHookTrust::new(None, root.clone())),
        runtime_environment: BTreeMap::new(),
    })
    .unwrap();
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        assembled.hooks.dispatch(&HookContext::new(
            SessionId::nil(),
            HookPoint::BeforeToolUse,
            HookData::before_tool("task_list", json!({})),
        )),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(outcome, HookOutcome::Continue);
    assert_eq!(parent_calls.load(Ordering::SeqCst), 1);
    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "policy guards must not install extension callbacks"
    );
    let result = requests[1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["type"] == "function_call_output" && item["call_id"] == "forced-read")
        .expect("actual paired tool output");
    (
        serde_json::from_str(result["output"].as_str().unwrap()).unwrap(),
        requests[1]["input"].to_string().contains(CANARY),
        requests[0]["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["name"] == "read_file")),
    )
}

#[tokio::test]
async fn hook_policy_forced_dispatch_cannot_expand_parent_capability_bits() {
    for allowed in [false, true] {
        let (output, body_sent, schema_visible) = assembled_read(
            PermissionRules::default(),
            vec![],
            if allowed {
                PermissionSet::read_only()
            } else {
                PermissionSet::empty()
            },
            false,
        )
        .await;
        assert_eq!(output["status"], if allowed { "ok" } else { "error" });
        assert_eq!(body_sent, allowed);
        assert_eq!(schema_visible, allowed);
        if !allowed {
            assert!(output.to_string().contains("permission_denied"));
        }
    }
}

#[tokio::test]
async fn hook_policy_path_guard_is_inherited_even_if_the_source_tool_is_full_access() {
    let (output, body_sent, schema_visible) = assembled_read(
        PermissionRules::default(),
        vec![],
        PermissionSet::read_only(),
        true,
    )
    .await;
    assert!(schema_visible);
    assert_eq!(output["status"], "error");
    assert!(output
        .to_string()
        .contains("blocked by the current sandbox mode"));
    assert!(!body_sent);
}

#[tokio::test]
async fn hook_policy_keeps_user_and_managed_precedence_without_reloading_sources() {
    for (precedence, managed_rule, ui_rule, allowed) in [
        (USER_DEEPAGENT_PRECEDENCE, "deny", "allow", false),
        (MANAGED_PRECEDENCE, "deny", "allow", false),
        (MANAGED_PRECEDENCE, "allow", "deny", true),
        (MANAGED_PRECEDENCE, "ask", "allow", false),
    ] {
        let ui_rules = serde_json::from_value(json!({ui_rule:["Read"]})).unwrap();
        let (output, body_sent, schema_visible) = assembled_read(
            ui_rules,
            vec![ConfigLayer {
                path: "synthetic-policy-source".into(),
                precedence,
                value: json!({"permissions":{managed_rule:["Read"]}}),
            }],
            PermissionSet::read_only(),
            false,
        )
        .await;
        assert!(schema_visible, "test actual dispatch, not hidden schemas");
        assert_eq!(output["status"], if allowed { "ok" } else { "error" });
        assert_eq!(body_sent, allowed);
    }
}
