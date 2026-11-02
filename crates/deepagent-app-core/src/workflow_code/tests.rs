//! Adapter lifecycle tests only. No mocked receipt is execution/containment
//! acceptance: these exercise handles and the existing ToolRegistry gates.
use super::*;
use deepagent_tools::ToolRegistry;

fn request() -> CodeRequest {
    CodeRequest::from_config(
        json!({
            "codeLanguage":"javascript",
            "codeScript":"function main(x) { return { result: 'PRIVATE_SOURCE' }; }",
            "codeInputVariables":[{"name":"input","value":"PRIVATE_INPUT"}],
            "codeOutputVariables":[{"name":"result","type":"string"}],
        })
        .as_object()
        .unwrap(),
        &BTreeMap::new(),
    )
    .unwrap()
}

fn gateway() -> Arc<WorkflowCodeGateway> {
    Arc::new(WorkflowCodeGateway::new(None, None))
}

#[test]
fn confined_sdk_requirement_uses_the_registry_pin_with_exact_not_prefix_matching() {
    use crate::runtime_service::{
        default_registry, RuntimePreference, RuntimeService, UnavailableDownloader,
    };
    let active = tempfile::tempdir().unwrap();
    let service = RuntimeService::new(
        &[active.path().to_path_buf()],
        Arc::new(UnavailableDownloader),
    );
    let registry = default_registry();
    for (kind, id) in [
        (RuntimeKind::Node, "node-24"),
        (RuntimeKind::Python, "python-3.11"),
    ] {
        let requirement = service.managed_requirement(kind).unwrap();
        let entry = registry.iter().find(|entry| entry.id == id).unwrap();
        assert_eq!(requirement.kind, kind);
        assert_eq!(requirement.preference, RuntimePreference::ManagedOnly);
        assert_eq!(requirement.version, Some(format!("={}", entry.version)));
    }
}

#[test]
fn prepare_is_private_bounded_and_has_no_sdk_requirement() {
    let gateway = gateway();
    let first = gateway.prepare(request()).unwrap();
    let arguments = serde_json::to_value(&first).unwrap();
    assert!(gateway.tool().validate_input(arguments.clone()).is_ok());
    assert_eq!(arguments.as_object().unwrap().len(), 3);
    assert!(!arguments.to_string().contains("PRIVATE"));
    assert_eq!(
        first.script_sha256,
        format!("{:x}", Sha256::digest(request().script.as_bytes()))
    );
    let parallel = gateway.prepare(request()).unwrap();
    assert_ne!(parallel.request_id, first.request_id);
    gateway.release(&parallel.request_id);
    gateway.release(&first.request_id);
    let next = gateway.prepare(request()).unwrap();
    assert_ne!(first.request_id, next.request_id);
    assert_eq!(first.script_sha256, next.script_sha256);
    assert!(gateway.tool().validate_input(arguments).is_err());
}

#[test]
fn altered_identity_and_cross_run_handles_are_rejected_without_consuming_the_real_request() {
    let gateway = gateway();
    let invocation = gateway.prepare(request()).unwrap();
    for field in ["request_id", "language", "script_sha256"] {
        let mut value = serde_json::to_value(&invocation).unwrap();
        value[field] = if field == "language" {
            json!("python")
        } else {
            json!("forged")
        };
        assert!(gateway.tool().validate_input(value).is_err());
    }
    let other = super::WorkflowCodeGateway::new(None, None);
    assert!(other
        .parse(serde_json::to_value(&invocation).unwrap())
        .is_err());
    assert!(gateway
        .tool()
        .validate_input(serde_json::to_value(invocation).unwrap())
        .is_ok());
}

#[test]
fn h181_parallel_preparation_is_run_local_bounded_and_individually_revocable() {
    let gateway = gateway();
    let invocations: Vec<_> = (0..50)
        .map(|_| gateway.prepare(request()).unwrap())
        .collect();
    let ids: std::collections::BTreeSet<_> =
        invocations.iter().map(|item| &item.request_id).collect();
    assert_eq!(ids.len(), 50);
    assert!(
        gateway.prepare(request()).is_err(),
        "no unbounded private handle cache"
    );
    for item in &invocations {
        assert!(gateway.parse(serde_json::to_value(item).unwrap()).is_ok());
    }
    gateway.release(&invocations[3].request_id);
    assert!(gateway
        .parse(serde_json::to_value(&invocations[3]).unwrap())
        .is_err());
    let replacement = gateway.prepare(request()).unwrap();
    assert!(!ids.contains(&replacement.request_id));
    for item in &invocations {
        gateway.release(&item.request_id);
    }
    gateway.release(&replacement.request_id);
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failure_consumes_the_handle_and_cannot_be_replayed() {
    let gateway = gateway();
    let invocation = gateway.prepare(request()).unwrap();
    let arguments = serde_json::to_value(&invocation).unwrap();
    let error = gateway.tool().invoke(arguments.clone()).await.unwrap_err();
    assert!(error.to_string().contains("CodeRuntimeUnavailable"));
    assert!(!error.to_string().contains("PRIVATE"));
    assert!(gateway
        .tool()
        .invoke(arguments)
        .await
        .unwrap_err()
        .to_string()
        .contains("CodeHandleInvalid"));
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancellation_is_observed_before_the_sdk_or_backend() {
    let gateway = gateway();
    let invocation = gateway.prepare(request()).unwrap();
    let context = ToolExecutionContext::default();
    context.cancellation_token().cancel();
    let error = gateway
        .tool()
        .invoke_with_context(serde_json::to_value(invocation).unwrap(), context)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("cancelled before SDK resolution"));
    assert!(gateway.pending.lock().unwrap().is_empty());
}

#[tokio::test]
async fn registry_requires_both_the_sandbox_capability_and_high_risk_approval() {
    let gateway = gateway();
    let invocation = gateway.prepare(request()).unwrap();
    let arguments = serde_json::to_value(invocation).unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(gateway.tool())).unwrap();
    let developer = PermissionSet::developer();
    assert!(registry
        .invoke(CODE_EXECUTE_TOOL, arguments.clone(), &developer, true)
        .await
        .is_err());
    let sandbox = developer.with(Permission::Sandbox);
    assert!(registry
        .invoke(CODE_EXECUTE_TOOL, arguments.clone(), &sandbox, false)
        .await
        .is_err());
    assert_eq!(gateway.pending.lock().unwrap().len(), 1);
    let error = registry
        .invoke(CODE_EXECUTE_TOOL, arguments, &sandbox, true)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("CodeRuntimeUnavailable"));
    assert!(gateway.pending.lock().unwrap().is_empty());
}
