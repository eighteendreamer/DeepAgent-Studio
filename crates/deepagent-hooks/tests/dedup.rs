//! H-239: only equivalent declarations may share one dispatch.
#[path = "dedup/real_actions.rs"]
mod real_actions;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::{error::Result, id::SessionId};
use deepagent_hooks::{
    HookAction, HookActionExecutor, HookContext, HookData, HookDefinitions, HookOutcome, HookPoint,
    HookRegistry, SystemHookRunner,
};
use serde_json::{json, Value};

#[derive(Default)]
struct Host(Mutex<Vec<HookAction>>);

#[async_trait]
impl HookActionExecutor for Host {
    async fn execute(&self, action: &HookAction, _: Value) -> Result<HookOutcome> {
        self.0.lock().unwrap().push(action.clone());
        Ok(HookOutcome::Continue)
    }
}

fn context(name: &str) -> HookContext {
    HookContext::new(
        SessionId::nil(),
        HookPoint::BeforeToolUse,
        HookData::before_tool(name, json!({"path":"fixture.txt"})),
    )
}

fn action(kind: &str) -> Value {
    json!({"type":kind,"command":"synthetic_tool","prompt":"synthetic policy"})
}

fn group(action: Value, condition: Option<&str>, once: bool) -> Value {
    let mut value = json!({"matcher":"read_file","once":once,"hooks":[action]});
    if let Some(condition) = condition {
        value["if"] = json!(condition);
    }
    value
}

fn definitions(groups: Vec<Value>) -> HookDefinitions {
    HookDefinitions::parse(&json!({"hooks":{"PreToolUse":groups}}).to_string()).unwrap()
}

fn register(definitions: &HookDefinitions, host: Arc<Host>) -> HookRegistry {
    let mut registry = HookRegistry::new();
    definitions.register_into_with_host(&mut registry, Arc::new(SystemHookRunner), host);
    registry
}

#[tokio::test]
async fn h239_independent_custom_executors_are_not_assumed_equivalent_but_shared_ones_dedup() {
    let first = Arc::new(Host::default());
    let second = Arc::new(Host::default());
    let defs = definitions(vec![group(action("mcp_tool"), None, false)]);
    let mut registry = register(&defs, first.clone());
    for host in [second.clone(), second.clone()] {
        defs.register_into_with_host(&mut registry, Arc::new(SystemHookRunner), host);
    }
    registry.dispatch(&context("read_file")).await.unwrap();
    assert_eq!(first.0.lock().unwrap().len(), 1);
    assert_eq!(second.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn h239_all_host_kinds_preserve_conditions_and_entire_action_configuration() {
    for kind in ["prompt", "agent", "mcp_tool"] {
        for (field, value) in [
            ("model", json!("synthetic-second-model")),
            ("agent", json!("synthetic-profile")),
            ("arguments", json!({"policy":"second"})),
            ("env", json!({"DEEPAGENT_PLUGIN_ROOT":"synthetic-root-b"})),
            ("timeout", json!(1)),
            ("async", json!("rewake")),
            ("url", json!("http://127.0.0.1:1/not-requested-by-host")),
            ("method", json!("PUT")),
            ("headers", json!({"X-Synthetic":"second"})),
            ("allowedEnvVars", json!(["SYNTHETIC_ALLOWLIST"])),
            ("statusMessage", json!("synthetic second declaration")),
        ] {
            let first = action(kind);
            let mut second = first.clone();
            second[field] = value;
            let host = Arc::new(Host::default());
            let registry = register(
                &definitions(vec![
                    group(first, None, false),
                    group(second.clone(), None, false),
                    group(second, None, false),
                ]),
                host.clone(),
            );
            registry.dispatch(&context("read_file")).await.unwrap();
            let actions = host.0.lock().unwrap();
            assert_eq!(actions.len(), 2, "{kind}/{field}");
            assert_ne!(actions[0], actions[1], "{kind}/{field}");
        }
        let host = Arc::new(Host::default());
        let registry = register(
            &definitions(vec![
                group(action(kind), Some("write_file"), false),
                group(action(kind), Some("read_file"), false),
            ]),
            host.clone(),
        );
        registry.dispatch(&context("read_file")).await.unwrap();
        assert_eq!(host.0.lock().unwrap().len(), 1, "{kind}/condition");
    }
}

#[tokio::test]
async fn h239_structural_identity_cannot_confuse_prompt_and_agent_delimiters() {
    let mut first = action("agent");
    first["prompt"] = json!("policy|part");
    first["agent"] = json!("profile");
    let mut second = first.clone();
    second["prompt"] = json!("policy");
    second["agent"] = json!("part|profile");
    let host = Arc::new(Host::default());
    let registry = register(
        &definitions(vec![group(first, None, false), group(second, None, false)]),
        host.clone(),
    );
    registry.dispatch(&context("read_file")).await.unwrap();
    assert_eq!(host.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn h239_equivalent_defaults_maps_and_duplicate_registration_still_execute_once() {
    let first = group(action("mcp_tool"), None, false);
    let mut second = first.clone();
    second["if"] = json!(" * ");
    second["hooks"][0]["timeout"] = json!(60);
    let host = Arc::new(Host::default());
    let mut registry = register(&definitions(vec![first, second]), host.clone());
    definitions(vec![group(action("mcp_tool"), None, false)]).register_into_with_host(
        &mut registry,
        Arc::new(SystemHookRunner),
        host.clone(),
    );
    registry.dispatch(&context("read_file")).await.unwrap();
    assert_eq!(host.0.lock().unwrap().len(), 1);

    let host = Arc::new(Host::default());
    let mut first = action("mcp_tool");
    first["arguments"] =
        serde_json::from_str(r#"{"z":{"b":2,"a":1},"a":[{"d":4,"c":3}]}"#).unwrap();
    let mut second = first.clone();
    second["arguments"] =
        serde_json::from_str(r#"{"a":[{"c":3,"d":4}],"z":{"a":1,"b":2}}"#).unwrap();
    let registry = register(
        &definitions(vec![group(first, None, false), group(second, None, false)]),
        host.clone(),
    );
    registry.dispatch(&context("read_file")).await.unwrap();
    assert_eq!(host.0.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn h239_once_identity_preserves_independent_models_and_shared_clone_lifetime() {
    let mut second = action("prompt");
    second["model"] = json!("synthetic-second-model");
    let defs = definitions(vec![
        group(action("prompt"), Some("read_file"), true),
        group(second.clone(), Some("read_file"), true),
        group(second, Some("read_file"), true),
    ]);
    let host = Arc::new(Host::default());
    let registry = register(&defs, host.clone());
    registry.dispatch(&context("write_file")).await.unwrap();
    assert!(host.0.lock().unwrap().is_empty());
    registry.dispatch(&context("read_file")).await.unwrap();
    registry
        .clone()
        .dispatch(&context("read_file"))
        .await
        .unwrap();
    assert_eq!(host.0.lock().unwrap().len(), 2);
    register(&defs, host.clone())
        .dispatch(&context("read_file"))
        .await
        .unwrap();
    assert_eq!(host.0.lock().unwrap().len(), 4);
}
