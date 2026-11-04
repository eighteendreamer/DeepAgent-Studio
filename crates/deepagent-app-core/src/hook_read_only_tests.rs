//! H234: a capability projection is not a Safe/name filter or a shared cache.
use super::*;
use deepagent_builtins::file_tools::ReadFileTool;
use deepagent_builtins::todo_tool::{TaskListTool, TodoItem, TodoStatus, TodoStore, TodoWriteTool};
use deepagent_builtins::WorkspaceRoot;
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};
use serde_json::json;
use std::sync::atomic::AtomicUsize;

struct SafeMutator {
    name: &'static str,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for SafeMutator {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: self.name.into(),
            description: "Synthetic state mutation, even with a familiar reader name".into(),
            parameters: json!({"type":"object"}),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }
    async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success(json!({})))
    }
}

#[tokio::test]
async fn h234_unknown_safe_state_tools_are_not_observer_capabilities_even_when_renamed() {
    for name in [
        "h234_state_writer",
        "read_file",
        "task_list",
        "todo_write",
        "task",
        "shell",
        "tool_search",
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut source = ToolRegistry::new();
        source
            .register(Arc::new(SafeMutator {
                name,
                calls: calls.clone(),
            }))
            .unwrap();
        assert!(source
            .get(name)
            .unwrap()
            .tool
            .is_concurrency_safe(&json!({})));
        let projected = build_hook_agent_registry(&source).unwrap();
        assert!(projected
            .invoke(name, json!({}), &PermissionSet::read_only(), false)
            .await
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // The parent capability was not globally removed or reclassified.
        source
            .invoke(name, json!({}), &PermissionSet::read_only(), false)
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn h234_file_observations_are_private_to_each_projected_actor() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("fixture.txt"), "H234 synthetic content").unwrap();
    let mut source = ToolRegistry::new();
    source
        .register(Arc::new(ReadFileTool::new(WorkspaceRoot::new(root.path()))))
        .unwrap();
    let arguments = json!({"path":"fixture.txt"});
    let parent = source
        .invoke(
            "read_file",
            arguments.clone(),
            &PermissionSet::read_only(),
            false,
        )
        .await
        .unwrap();
    assert_eq!(parent.value["unchanged_stub"], serde_json::Value::Null);
    for _ in 0..2 {
        let hook = build_hook_agent_registry(&source).unwrap();
        let read = hook
            .invoke(
                "read_file",
                arguments.clone(),
                &PermissionSet::read_only(),
                false,
            )
            .await
            .unwrap();
        assert!(read.value["content"]
            .as_str()
            .unwrap()
            .contains("H234 synthetic content"));
        assert_eq!(read.value["unchanged_stub"], serde_json::Value::Null);
    }
    let parent_again = source
        .invoke("read_file", arguments, &PermissionSet::read_only(), false)
        .await
        .unwrap();
    assert_eq!(parent_again.value["unchanged_stub"], true);
}

#[tokio::test]
async fn h234_observing_parent_tasks_does_not_consume_the_parent_reminder() {
    let store = TodoStore::new();
    let mut source = ToolRegistry::new();
    source
        .register(Arc::new(TaskListTool::new(store.clone())))
        .unwrap();
    source
        .register(Arc::new(TodoWriteTool::new(store.clone())))
        .unwrap();
    let observer = build_hook_agent_registry(&source).unwrap();
    // The parent's list is updated after assembly, as in a real tool hook.
    store.replace(vec![TodoItem {
        content: "H234 parent".into(),
        active_form: "Observing parent".into(),
        status: TodoStatus::Pending,
    }]);
    store.mark_snapshot_pending();
    let output = observer
        .invoke("task_list", json!({}), &PermissionSet::read_only(), false)
        .await
        .unwrap();
    assert_eq!(output.value["todos"][0]["content"], "H234 parent");
    assert!(store.is_snapshot_pending());
    assert_eq!(store.snapshot().len(), 1);
    assert!(observer.visible_to(&PermissionSet::empty()).is_empty());
    assert!(observer
        .invoke("task_list", json!({}), &PermissionSet::empty(), false)
        .await
        .is_err());
}

struct ChangedProjection;

#[async_trait]
impl Tool for ChangedProjection {
    fn descriptor(&self) -> ToolDescriptor {
        SafeMutator {
            name: "source_contract",
            calls: Default::default(),
        }
        .descriptor()
    }
    fn read_only_projection(self: Arc<Self>) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(SafeMutator {
            name: "different_contract",
            calls: Default::default(),
        }))
    }
    async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput> {
        unreachable!("contract rejection precedes invocation")
    }
}

#[test]
fn h234_projection_cannot_change_the_original_descriptor_or_grant() {
    let mut source = ToolRegistry::new();
    source.register(Arc::new(ChangedProjection)).unwrap();
    assert!(build_hook_agent_registry(&source)
        .err()
        .unwrap()
        .to_string()
        .contains("read-only projection changed its capability contract"));
}
