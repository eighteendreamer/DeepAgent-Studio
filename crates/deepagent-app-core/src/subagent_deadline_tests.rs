//! H-45: drive the production TaskTool -> runner -> child engine through the
//! parent deadline. Only model I/O and waiting tool/hook bodies are controlled.

use super::tests::audit_child_runner_with_db;
use super::*;
use deepagent_builtins::SubagentRunner;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::SubagentRunStore;
use deepagent_runtime::{Agent, AgentDecision, AgentKernel, Observation, RunRequest, TerminalKind};
use deepagent_tools::{RiskLevel, Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
struct WaitPoint {
    entered: Notify,
    active: AtomicUsize,
}

struct ActiveWait<'a>(&'a WaitPoint);

impl WaitPoint {
    fn enter(&self) -> ActiveWait<'_> {
        self.active.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        ActiveWait(self)
    }
}

impl Drop for ActiveWait<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ChildTransport {
    call_tool: bool,
}

#[async_trait::async_trait]
impl HttpTransport for ChildTransport {
    async fn stream(&self, _request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        if self.call_tool {
            sink.on_event(r#"{"type":"response.output_item.done","item":{"type":"function_call","call_id":"child-probe","name":"deadline_probe","arguments":"{}"}}"#)?;
        } else {
            sink.on_event(r#"{"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"child complete"}]}}"#)?;
        }
        sink.on_event(r#"{"type":"response.completed","response":{"status":"completed"}}"#)?;
        Ok(())
    }
}

struct WaitingTool {
    wait: Arc<WaitPoint>,
    cooperative: bool,
    approval: bool,
}

#[async_trait::async_trait]
impl Tool for WaitingTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "deadline_probe".into(),
            description: "Local H-45 wait fixture".into(),
            parameters: serde_json::json!({"type":"object","properties":{}}),
            risk: if self.approval {
                RiskLevel::High
            } else {
                RiskLevel::Medium
            },
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, _arguments: serde_json::Value) -> Result<ToolOutput> {
        panic!("the production child must supply ToolExecutionContext")
    }

    async fn invoke_with_context(
        &self,
        _arguments: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let _active = self.wait.enter();
        if self.cooperative {
            context.cancelled().await;
            Err(CoreError::other("H-45 fixture observed cancellation"))
        } else {
            std::future::pending().await
        }
    }
}

struct WaitingHook(Arc<WaitPoint>);

#[async_trait::async_trait]
impl deepagent_hooks::Hook for WaitingHook {
    fn name(&self) -> &str {
        "h45-wait"
    }

    async fn run(&self, _ctx: &HookContext) -> Result<HookOutcome> {
        let _active = self.0.enter();
        std::future::pending().await
    }
}

struct Delegate {
    isolation: &'static str,
    background: bool,
}

#[async_trait::async_trait]
impl Agent for Delegate {
    async fn think(&mut self, step: usize, _last: &[Observation]) -> Result<AgentDecision> {
        if step != 0 {
            // A background child outlives this foreground wait by design.
            std::future::pending::<()>().await;
        }
        Ok(AgentDecision::CallTool(deepagent_tools::ToolInvocation {
            id: Some("parent-delegate".into()),
            name: deepagent_builtins::TASK_TOOL_NAME.into(),
            arguments: serde_json::json!({
                "description":"H-45 child", "prompt":"local fixture",
                "isolation":self.isolation, "background":self.background,
            }),
        }))
    }
}

async fn run_deadline(
    runner: &ChatSubagentRunner,
    wait: &WaitPoint,
    isolation: &'static str,
    background: bool,
) -> deepagent_runtime::KernelTerminal {
    let db = &runner.db;
    let parent = RunStore::new(db).get("audit-parent").unwrap().unwrap();
    let mut session =
        Session::recover(db, &SystemClock, parent.session_id.parse().unwrap()).unwrap();
    let task = session.create_task("H-45 parent").unwrap();
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(deepagent_builtins::TaskTool::new(
            runner.clone(),
            vec![],
        )))
        .unwrap();
    let kernel = AgentKernel::new(
        db.clone(),
        &registry,
        Default::default(),
        deepagent_runtime::RuntimeConfig {
            task_timeout: Some(Duration::from_secs(1)),
            ..Default::default()
        },
        "audit-parent",
    );
    let control = kernel.handle();
    let mut agent = Delegate {
        isolation,
        background,
    };
    let (terminal, ()) = tokio::time::timeout(Duration::from_secs(6), async {
        tokio::join!(
            kernel.start(RunRequest::new(&mut session, task, &mut agent)),
            async {
                tokio::time::timeout(Duration::from_secs(3), wait.entered.notified())
                    .await
                    .expect("the actual child must reach the selected boundary before termination");
            }
        )
    })
    .await
    .expect("kernel must stop within its execution budget plus cleanup grace");
    let terminal = terminal.unwrap();
    assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
    assert!(control.is_cancelled());
    assert_eq!(
        session.state().task(task).unwrap().state,
        deepagent_core::task::TaskState::Failed
    );
    let events = RunStore::new(db)
        .events_after("audit-parent", None)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "run_terminal")
            .count(),
        1
    );
    terminal
}

fn assert_child_finalized(db: &Database) -> String {
    let children = SubagentRunStore::new(db)
        .list_for_parent("audit-parent")
        .unwrap();
    assert_eq!(children.len(), 1);
    let child = &children[0];
    assert_eq!(child.state, "cancelled");
    assert!(child.finished_at.is_some());
    let transcript: serde_json::Value =
        serde_json::from_slice(&std::fs::read(child.transcript_path.as_ref().unwrap()).unwrap())
            .unwrap();
    assert_eq!(transcript["state"], "cancelled");
    assert_eq!(transcript["attempts"].as_array().unwrap().len(), 1);
    let events = RunStore::new(db)
        .events_after("audit-parent", None)
        .unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|e| {
            matches!(
                e.event_type.as_str(),
                "subagent_completed" | "subagent_cancelled"
            )
        })
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].event_type, "subagent_cancelled");
    assert!(
        terminals[0].sequence
            < events
                .iter()
                .find(|e| e.event_type == "run_terminal")
                .unwrap()
                .sequence
    );
    child.id.clone()
}

#[tokio::test]
async fn h45_deadline_finalizes_child_during_tool_and_approval() {
    for boundary in ["tool", "uncooperative_tool", "approval"] {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.sqlite");
        let db = Arc::new(Database::open(&db_path).unwrap());
        let mut runner = audit_child_runner_with_db(
            dir.path(),
            Arc::new(ChildTransport { call_tool: true }),
            db.clone(),
        );
        let wait = Arc::new(WaitPoint::default());
        let tool_wait = if boundary == "approval" {
            Arc::new(WaitPoint::default())
        } else {
            wait.clone()
        };
        let mut child_registry = (*runner.registry).clone();
        child_registry
            .register(Arc::new(WaitingTool {
                wait: tool_wait.clone(),
                cooperative: boundary != "uncooperative_tool",
                approval: boundary == "approval",
            }))
            .unwrap();
        runner.registry = Arc::new(child_registry);
        let pending = crate::approval_bridge::PendingApprovals::new();
        if boundary == "approval" {
            let observed = wait.clone();
            runner
                .parent_approvals
                .set(Arc::new(crate::approval_bridge::ChannelApprovalGate::new(
                    pending.clone(),
                    Arc::new(move |_| {
                        observed.entered.notify_one();
                    }),
                )))
                .ok()
                .unwrap();
        }
        let terminal = run_deadline(&runner, &wait, "shared", false).await;
        let id = assert_child_finalized(&db);
        assert_eq!(pending.pending_count(), 0);
        assert_eq!(tool_wait.active.load(Ordering::SeqCst), 0);
        if boundary == "uncooperative_tool" {
            assert!(terminal.reason.unwrap().contains("cleanup exceeded"));
            assert!(SubagentRunStore::new(&db)
                .get(&id)
                .unwrap()
                .unwrap()
                .summary
                .unwrap()
                .contains("future dropped"));
        }
        runner.cleanup(&id).await.unwrap();
        drop(runner);
        drop(db);
        // File-backed durability and event cursors, not just in-memory state.
        let reopened = Database::open(&db_path).unwrap();
        assert_eq!(assert_child_finalized(&reopened), id);
        assert_eq!(
            RunStore::new(&reopened)
                .get("audit-parent")
                .unwrap()
                .unwrap()
                .terminal_kind
                .as_deref(),
            Some("budget_exceeded")
        );
        eprintln!("H-45 boundary={boundary}: child finalized once, wait released, reopened state consistent");
    }
}

#[tokio::test]
async fn h45_forced_drop_finalizes_child_during_preparation_and_stop_hook() {
    for point in [
        HookPoint::SubagentStart,
        HookPoint::WorktreeCreate,
        HookPoint::SubagentStop,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let runner = audit_child_runner_with_db(
            dir.path(),
            Arc::new(ChildTransport { call_tool: false }),
            db.clone(),
        );
        let wait = Arc::new(WaitPoint::default());
        let mut hooks = HookRegistry::new();
        hooks.register(point, Arc::new(WaitingHook(wait.clone())));
        runner.hooks.set(Arc::new(hooks)).ok().unwrap();
        let isolation = if point == HookPoint::WorktreeCreate {
            "worktree"
        } else {
            "shared"
        };
        let terminal = run_deadline(&runner, &wait, isolation, false).await;
        assert!(terminal.reason.unwrap().contains("cleanup exceeded"));
        let id = assert_child_finalized(&db);
        assert_eq!(wait.active.load(Ordering::SeqCst), 0);
        runner.cleanup(&id).await.unwrap();
        eprintln!("H-45 hook={point:?}: forced-drop owner finalized once");
    }
}

#[tokio::test]
async fn h45_deadline_does_not_cancel_independent_background_child() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut runner = audit_child_runner_with_db(
        dir.path(),
        Arc::new(ChildTransport { call_tool: true }),
        db.clone(),
    );
    let wait = Arc::new(WaitPoint::default());
    let mut registry = (*runner.registry).clone();
    registry
        .register(Arc::new(WaitingTool {
            wait: wait.clone(),
            cooperative: true,
            approval: false,
        }))
        .unwrap();
    runner.registry = Arc::new(registry);
    run_deadline(&runner, &wait, "shared", true).await;
    let children = SubagentRunStore::new(&db)
        .list_for_parent("audit-parent")
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].state, "running");
    assert_eq!(wait.active.load(Ordering::SeqCst), 1);
    assert!(runner.cancel(&children[0].id).await.unwrap());
    tokio::time::timeout(Duration::from_secs(3), async {
        while SubagentRunStore::new(&db)
            .get(&children[0].id)
            .unwrap()
            .unwrap()
            .finished_at
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(wait.active.load(Ordering::SeqCst), 0);
    let events = RunStore::new(&db)
        .events_after("audit-parent", None)
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "subagent_cancelled")
            .count(),
        1
    );
}

#[tokio::test]
async fn h45_forced_drop_retains_real_worktree_for_existing_cleanup() {
    fn git(root: &Path, args: &[&str]) -> String {
        let mut command = std::process::Command::new("git");
        command
            .arg("-C")
            .arg(root)
            .args([
                "-c",
                "user.name=H45 Fixture",
                "-c",
                "user.email=h45@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command
            .output()
            .expect("local Git required for real worktree regression");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init"]);
    std::fs::write(root.join("base.txt"), "unchanged").unwrap();
    git(&root, &["add", "base.txt"]);
    git(&root, &["commit", "-m", "fixture"]);
    let db = Arc::new(Database::open_in_memory().unwrap());
    let mut runner = audit_child_runner_with_db(
        &root,
        Arc::new(ChildTransport { call_tool: false }),
        db.clone(),
    );
    runner.transcript_root = dir.path().join("transcripts");
    let wait = Arc::new(WaitPoint::default());
    let mut hooks = HookRegistry::new();
    // CwdChanged occurs after real Git creation and its path is recorded.
    // Prevent the async cleanup path from running, exercising the drop owner.
    hooks.register(HookPoint::CwdChanged, Arc::new(WaitingHook(wait.clone())));
    runner.hooks.set(Arc::new(hooks)).ok().unwrap();
    run_deadline(&runner, &wait, "worktree", false).await;
    let id = assert_child_finalized(&db);
    let child = SubagentRunStore::new(&db).get(&id).unwrap().unwrap();
    let retained = child.worktree_path.unwrap();
    assert!(Path::new(&retained).join("base.txt").is_file());
    assert!(git(&root, &["worktree", "list", "--porcelain"]).contains(&id));
    assert!(runner.cleanup(&id).await.unwrap());
    assert!(!Path::new(&retained).exists());
    assert!(!git(&root, &["worktree", "list", "--porcelain"]).contains(&id));
    assert!(SubagentRunStore::new(&db)
        .get(&id)
        .unwrap()
        .unwrap()
        .worktree_path
        .is_none());
    assert_eq!(wait.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read_to_string(root.join("base.txt")).unwrap(),
        "unchanged"
    );
}
