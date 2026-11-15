//! H-22: exercise real child tool dispatch with an offline model transport.
use super::tests::{audit_child_runner, audit_child_runner_with_db};
use super::*;
use deepagent_builtins::{SubagentRequest, SubagentRunner};
use deepagent_persistence::checkpoint_store::CheckpointStore;
use deepagent_persistence::subagent_store::SubagentRunStore;
use deepagent_subagents::{SubAgentContext, SubAgentExecutor, Worktree};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

type Calls = Vec<(&'static str, Value)>;

struct WorkspaceTransport {
    namespace: &'static str,
    calls: Calls,
    consumer_calls: Option<Calls>,
    requests: Mutex<Vec<Value>>,
}

impl WorkspaceTransport {
    fn new(calls: Calls) -> Arc<Self> {
        Self::named(calls, "workspace")
    }

    fn named(calls: Calls, namespace: &'static str) -> Arc<Self> {
        Arc::new(Self {
            namespace,
            calls,
            consumer_calls: None,
            requests: Default::default(),
        })
    }

    fn output(&self, step: usize) -> Value {
        let id = format!("{}-call-{step}", self.namespace);
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .flat_map(|body| body["input"].as_array().unwrap())
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == id)
            .map(|item| serde_json::from_str(item["output"].as_str().unwrap()).unwrap())
            .unwrap_or_else(|| panic!("missing actual tool output: {id}"))
    }
}

#[async_trait::async_trait]
impl deepagent_models::transport::HttpTransport for WorkspaceTransport {
    async fn stream(
        &self,
        request: deepagent_models::transport::TransportRequest,
        sink: &mut dyn deepagent_models::transport::EventSink,
    ) -> Result<()> {
        let sink: &mut dyn deepagent_models::transport::EventSink =
            &mut crate::test_sequence::sink(sink);
        let body: Value = serde_json::from_str(&request.body)?;
        let input = body["input"].as_array().unwrap();
        // Count this attempt's settled outputs. A resumed context also contains
        // prior attempts; their outputs must not advance this scripted sequence
        // or cause the fixture to reuse a previously persisted call identity.
        let prefix = format!("{}-call-", self.namespace);
        let step = input
            .iter()
            .filter(|item| {
                item["type"] == "function_call_output"
                    && item["call_id"]
                        .as_str()
                        .is_some_and(|id| id.starts_with(&prefix))
            })
            .count();
        let calls = if input.iter().any(|item| {
            item["role"] == "user" && item["content"].to_string().contains("consumer-fixture")
        }) {
            self.consumer_calls.as_ref().unwrap_or(&self.calls)
        } else {
            &self.calls
        };
        self.requests.lock().unwrap().push(body.clone());
        let mut output = Vec::new();
        if let Some((name, arguments)) = calls.get(step) {
            let item = json!({"type":"function_call", "id":format!("{}-item-{step}", self.namespace),
                "call_id":format!("{}-call-{step}", self.namespace), "name":name,
                "arguments":arguments.to_string()});
            output.push(item.clone());
            for kind in ["response.output_item.added", "response.output_item.done"] {
                sink.on_event(&json!({"type":kind,"item":item}).to_string())?;
            }
        } else {
            sink.on_event(r#"{"type":"response.output_text.delta","delta":"workspace done"}"#)?;
        }
        sink.on_event(&serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":output,"status":"completed"}}).to_string())?;
        Ok(())
    }
}

fn context(id: &str, root: &Path) -> SubAgentContext {
    SubAgentContext {
        node_id: id.into(),
        goal: format!("{id}: inspect and update only the assigned workspace"),
        role: None,
        phase: None,
        worktree: Worktree {
            name: id.into(),
            path: root.to_string_lossy().into_owned(),
        },
        upstream_results: Vec::new(),
    }
}

fn request(isolation: &str) -> SubagentRequest {
    SubagentRequest {
        description: "workspace fixture".into(),
        prompt: "Inspect and update the workspace".into(),
        subagent_type: None,
        allowed_tools: vec![],
        model: None,
        effort: None,
        skills: vec![],
        isolation: isolation.into(),
        fork: false,
        phase: None,
        label: None,
        schema: None,
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Workspace Fixture",
            "-c",
            "user.email=workspace@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("Git required for workspace integration tests");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn repo(root: &Path) {
    std::fs::create_dir(root).unwrap();
    git(root, &["init"]);
    std::fs::write(root.join("base.txt"), "original").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "fixture"]);
}

#[cfg(windows)]
fn h112_install_commands(runner: &ChatSubagentRunner, log: &Path, deny_create: bool) {
    use deepagent_hooks::{HookDefinitions, SystemHookRunner};
    let quoted = log.to_string_lossy().replace('\'', "''");
    let command = format!(
        // Decode the documented UTF-8 wire rather than depending on the host
        // console code page; the production runner itself is not changed here.
        "$reader=[IO.StreamReader]::new([Console]::OpenStandardInput(),[Text.Encoding]::UTF8); \
         $raw=$reader.ReadToEnd(); $p=$raw | ConvertFrom-Json; \
         $path=$p.path; if($p.worktree_path){{$path=$p.worktree_path}}; \
         if($p.new_cwd){{$path=$p.new_cwd}}; \
         $exists=if($path){{[IO.Directory]::Exists($path)}}else{{$null}}; \
         $entry=@{{payload=$p;directory_exists=$exists}} | ConvertTo-Json -Compress -Depth 20; \
         [IO.File]::AppendAllText('{quoted}',$entry+[Environment]::NewLine)"
    );
    let mut definitions = json!({"hooks":{}});
    for event in ["WorktreeCreate", "WorktreeRemove", "CwdChanged"] {
        let command = if deny_create && event == "WorktreeCreate" {
            format!("{command}; [Console]::Error.Write('h112 deliberate veto'); exit 2")
        } else {
            command.clone()
        };
        definitions["hooks"][event] = json!([{"hooks":[{
            "type":"command","shell":"powershell","timeout":5,"command":command
        }]}]);
    }
    let mut hooks = HookRegistry::new();
    assert_eq!(
        HookDefinitions::parse(&definitions.to_string())
            .unwrap()
            .register_into(&mut hooks, Arc::new(SystemHookRunner)),
        3
    );
    assert!(runner.hooks.set(Arc::new(hooks)).is_ok());
}

#[cfg(windows)]
fn h112_read_commands(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[cfg(windows)]
#[tokio::test]
async fn h112_real_git_hooks_keep_identity_paths_timing_and_shared_control() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("项目 repo");
    repo(&root);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let db_path = temp.path().join("state.sqlite");
    let db = Arc::new(Database::open(&db_path).unwrap());
    let transport = WorkspaceTransport::new(vec![]);
    let mut runner = audit_child_runner_with_db(&root, transport.clone(), db.clone());
    runner.transcript_root = temp.path().join("transcripts");
    let log = temp.path().join("hook-events.jsonl");
    h112_install_commands(&runner, &log, false);
    runner.run_inner(request("shared"), None).await.unwrap();
    assert!(
        !log.exists(),
        "shared child must not claim worktree lifecycle"
    );
    runner.run_inner(request("worktree"), None).await.unwrap();
    let store = SubagentRunStore::new(&db);
    let record = store
        .list_for_parent("audit-parent")
        .unwrap()
        .into_iter()
        .find(|record| record.worktree_path.is_some())
        .unwrap();
    let child = record.worktree_path.as_ref().unwrap();
    assert!(Path::new(child).join("base.txt").is_file());
    assert!(git(&root, &["worktree", "list", "--porcelain"]).contains(&record.id));
    assert!(runner.cleanup(&record.id).await.unwrap());
    assert!(!Path::new(child).exists());
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(root.join("base.txt")).unwrap(),
        "original"
    );
    let observed = h112_read_commands(&log);
    assert_eq!(observed.len(), 3);
    let create = &observed[0]["payload"];
    let cwd = &observed[1]["payload"];
    let remove = &observed[2]["payload"];
    assert_eq!(create["hook_event_name"], "WorktreeCreate");
    assert_eq!(create["name"], record.id);
    assert_eq!(
        create["path"], *child,
        "legacy guard consumers still receive planned path"
    );
    assert_eq!(
        observed[0]["directory_exists"], false,
        "create hook precedes Git"
    );
    assert_eq!(cwd["hook_event_name"], "CwdChanged");
    assert_eq!(cwd["old_cwd"], root.to_string_lossy().as_ref());
    assert_eq!(cwd["new_cwd"], *child);
    assert_eq!(cwd["path"], *child);
    assert_eq!(observed[1]["directory_exists"], true);
    assert_eq!(remove["hook_event_name"], "WorktreeRemove");
    assert_eq!(remove["worktree_path"], *child);
    assert_eq!(remove["path"], *child);
    assert_eq!(
        observed[2]["directory_exists"], true,
        "remove hook precedes deletion"
    );
    for entry in &observed {
        assert_eq!(
            entry["payload"]["session_id"],
            runner.parent_session_id().unwrap().to_string()
        );
    }
    let before = store.get(&record.id).unwrap().unwrap();
    assert!(before.worktree_path.is_none());
    assert_eq!(before.state, "succeeded");
    assert_eq!(transport.requests.lock().unwrap().len(), 2);
    drop(runner);
    drop(db);
    let reopened = Database::open(&db_path).unwrap();
    assert_eq!(
        SubagentRunStore::new(&reopened)
            .get(&record.id)
            .unwrap()
            .unwrap(),
        before
    );
}

#[cfg(windows)]
#[tokio::test]
async fn h112_create_veto_prevents_git_and_model_without_changing_parent() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    repo(&root);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let transport = WorkspaceTransport::new(vec![]);
    let mut runner = audit_child_runner(&root, transport.clone());
    runner.transcript_root = temp.path().join("transcripts");
    let log = temp.path().join("veto.jsonl");
    h112_install_commands(&runner, &log, true);
    let error = runner
        .run_inner(request("worktree"), None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("h112 deliberate veto"));
    assert!(transport.requests.lock().unwrap().is_empty());
    let records = SubagentRunStore::new(&runner.db)
        .list_for_parent("audit-parent")
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].state, "failed");
    assert!(records[0].worktree_path.is_none());
    assert!(!runner.worktree_base().join(&records[0].id).exists());
    let observed = h112_read_commands(&log);
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0]["payload"]["name"], records[0].id);
    assert_eq!(observed[0]["directory_exists"], false);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
}

#[cfg(windows)]
#[tokio::test]
async fn h112_failed_child_notifies_before_real_git_cleanup() {
    struct FailedChild(AtomicUsize);
    #[async_trait::async_trait]
    impl deepagent_models::transport::HttpTransport for FailedChild {
        async fn stream(
            &self,
            _: deepagent_models::transport::TransportRequest,
            _: &mut dyn deepagent_models::transport::EventSink,
        ) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(CoreError::invalid("h112 synthetic child failure"))
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    repo(&root);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let transport = Arc::new(FailedChild(AtomicUsize::new(0)));
    let mut runner = audit_child_runner(&root, transport.clone());
    runner.transcript_root = temp.path().join("transcripts");
    let log = temp.path().join("failed-child.jsonl");
    h112_install_commands(&runner, &log, false);
    assert!(runner.run_inner(request("worktree"), None).await.is_err());
    assert_eq!(transport.0.load(Ordering::SeqCst), 1);
    let records = SubagentRunStore::new(&runner.db)
        .list_for_parent("audit-parent")
        .unwrap();
    assert_eq!(records.len(), 1);
    let child = runner.worktree_base().join(&records[0].id);
    assert_eq!(records[0].state, "failed");
    assert!(records[0].worktree_path.is_none());
    assert!(!child.exists());
    assert!(!git(&root, &["worktree", "list", "--porcelain"]).contains(&records[0].id));
    let observed = h112_read_commands(&log);
    assert_eq!(observed.len(), 3);
    assert_eq!(observed[0]["payload"]["name"], records[0].id);
    assert_eq!(observed[0]["directory_exists"], false);
    assert_eq!(
        observed[1]["payload"]["new_cwd"],
        child.to_string_lossy().as_ref()
    );
    assert_eq!(
        observed[2]["payload"]["worktree_path"],
        child.to_string_lossy().as_ref()
    );
    assert_eq!(observed[2]["directory_exists"], true);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
}

#[tokio::test]
async fn h112_managed_creation_rejects_path_rewrite_before_git_and_model() {
    struct Rewrite;
    #[async_trait::async_trait]
    impl deepagent_hooks::Hook for Rewrite {
        fn name(&self) -> &str {
            "h112-rewrite"
        }
        async fn run(&self, _: &HookContext) -> Result<HookOutcome> {
            Ok(HookOutcome::modify(json!({"path":"outside-managed-root"})))
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    repo(&root);
    let transport = WorkspaceTransport::new(vec![]);
    let mut runner = audit_child_runner(&root, transport.clone());
    runner.transcript_root = temp.path().join("transcripts");
    let mut hooks = HookRegistry::new();
    hooks.register(HookPoint::WorktreeCreate, Arc::new(Rewrite));
    assert!(runner.hooks.set(Arc::new(hooks)).is_ok());
    let error = runner
        .run_inner(request("worktree"), None)
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("rewrite the managed worktree path"));
    assert!(transport.requests.lock().unwrap().is_empty());
    assert!(!runner.worktree_base().exists());
}

#[tokio::test]
async fn dag_relative_absolute_read_edit_search_and_checkpoint_use_child_root() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    let child = temp.path().join("child");
    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&child).unwrap();
    std::fs::write(parent.join("base.txt"), "parent-only").unwrap();
    std::fs::write(child.join("base.txt"), "child-before").unwrap();
    let transport = WorkspaceTransport::new(vec![
        ("read_file", json!({"path":"base.txt"})),
        (
            "edit_file",
            json!({"path":"base.txt","old":"child-before","new":"child-after"}),
        ),
        (
            "write_file",
            json!({"path":child.join("absolute.txt"),"content":"absolute-child"}),
        ),
        ("grep", json!({"query":"child-after","glob":"*.txt"})),
    ]);
    let runner = audit_child_runner(&parent, transport.clone());
    let db = runner.db.clone();
    let parent_checkpoint = Arc::new(
        deepagent_runtime::CheckpointManager::new(
            db.clone(),
            "audit-parent",
            0,
            &parent,
            temp.path().join("parent-checkpoints"),
        )
        .unwrap(),
    );
    runner
        .parent_checkpoint
        .set(parent_checkpoint.clone())
        .unwrap();
    assert!(
        DagSubagentExecutor::new(runner)
            .execute(context("root-tools", &child))
            .await
            .unwrap()
            .ok
    );
    assert_eq!(
        std::fs::read_to_string(parent.join("base.txt")).unwrap(),
        "parent-only"
    );
    assert_eq!(
        std::fs::read_to_string(child.join("base.txt")).unwrap(),
        "child-after"
    );
    assert_eq!(
        std::fs::read_to_string(child.join("absolute.txt")).unwrap(),
        "absolute-child"
    );
    for step in 0..4 {
        assert_eq!(
            transport.output(step)["status"],
            "ok",
            "step {step}: {}",
            transport.output(step)
        );
    }
    assert!(transport.output(0).to_string().contains("child-before"));
    assert!(transport.output(3).to_string().contains("child-after"));
    let evidence = parent_checkpoint.mutation_evidence().unwrap();
    assert_eq!(evidence.len(), 2);
    assert!(evidence.iter().all(|entry| entry.path.starts_with(&child)));
    let checkpoints = CheckpointStore::new(&db)
        .list_for_run("audit-parent")
        .unwrap();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(Path::new(&checkpoints[0].workspace_root), child);
    deepagent_runtime::CheckpointManager::restore(&db, &checkpoints[0].id).unwrap();
    assert_eq!(
        std::fs::read_to_string(child.join("base.txt")).unwrap(),
        "child-before"
    );
    assert!(!child.join("absolute.txt").exists());
    assert_eq!(
        std::fs::read_to_string(parent.join("base.txt")).unwrap(),
        "parent-only"
    );
}

#[tokio::test]
async fn dag_workspace_policy_rejects_parent_paths_before_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("parent");
    let child = temp.path().join("child");
    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&child).unwrap();
    std::fs::write(parent.join("parent.txt"), "parent canary").unwrap();
    let transport = WorkspaceTransport::new(vec![
        ("read_file", json!({"path":parent.join("parent.txt")})),
        (
            "write_file",
            json!({"path":parent.join("outside.txt"),"content":"must reject"}),
        ),
    ]);
    let runner = audit_child_runner(&parent, transport.clone());
    let result = DagSubagentExecutor::new(runner)
        .execute(context("rejected-parent-path", &child))
        .await
        .unwrap();
    let denied_read = transport.output(0);
    assert_eq!(denied_read["status"], "error");
    assert!(!denied_read.to_string().contains("parent canary"));
    assert!(
        !result.ok,
        "outside write must stop before checkpoint capture: {result:?}"
    );
    assert!(result.summary.contains("escapes workspace"), "{result:?}");
    assert!(!parent.join("outside.txt").exists());
    assert!(!child.join("outside.txt").exists());
    assert_eq!(
        std::fs::read_to_string(parent.join("parent.txt")).unwrap(),
        "parent canary"
    );
}

#[tokio::test]
async fn dag_concurrent_assignments_isolate_persisted_and_ephemeral_children() {
    for persisted in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        for root in [&parent, &left, &right] {
            std::fs::create_dir(root).unwrap();
        }
        let transport = WorkspaceTransport::new(vec![(
            "write_file",
            json!({"path":"same.txt","content":"isolated child"}),
        )]);
        let mut runner = audit_child_runner(&parent, transport.clone());
        if !persisted {
            runner.parent_run_id = "ephemeral-parent".into();
        }
        let executor = DagSubagentExecutor::new(runner.clone());
        let (a, b) = tokio::join!(
            executor.execute(context("left", &left)),
            executor.execute(context("right", &right))
        );
        assert!(a.unwrap().ok && b.unwrap().ok, "persisted={persisted}");
        assert_eq!(runner.root, parent);
        assert!(runner.assigned_worktree.is_none());
        assert!(!parent.join("same.txt").exists());
        for child in [&left, &right] {
            assert_eq!(
                std::fs::read_to_string(child.join("same.txt")).unwrap(),
                "isolated child"
            );
            assert!(transport.requests.lock().unwrap().iter().any(|body| {
                body["input"].as_array().unwrap().iter().any(|item| {
                    serde_json::from_value(item.clone())
                        .ok()
                        .and_then(|item| {
                            deepagent_core::runtime_snapshot::RuntimeSnapshot::from_item(&item)
                        })
                        .is_some_and(|snapshot| {
                            snapshot
                                .context
                                .contains(&format!("Working directory: {}", child.display()))
                        })
                })
            }));
        }
        assert_eq!(transport.requests.lock().unwrap().len(), 4);
        // The same original runner and registry still dispatch into the parent.
        runner.run_inner(request("shared"), None).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(parent.join("same.txt")).unwrap(),
            "isolated child"
        );
    }
}

#[tokio::test]
async fn missing_dag_assignment_fails_before_model_without_parent_fallback() {
    for persisted in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let transport = WorkspaceTransport::new(vec![]);
        let mut runner = audit_child_runner(temp.path(), transport.clone());
        if !persisted {
            runner.parent_run_id = "ephemeral-parent".into();
        }
        let result = DagSubagentExecutor::new(runner)
            .execute(context("missing", &temp.path().join("missing")))
            .await
            .unwrap();
        assert!(!result.ok);
        assert!(result.summary.contains("worktree is missing"));
        assert!(transport.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn ordinary_worktree_task_rebinds_without_changing_shared_runner() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    repo(&root);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let transport = WorkspaceTransport::new(vec![(
        "write_file",
        json!({"path":"output.txt","content":"child"}),
    )]);
    let mut runner = audit_child_runner(&root, transport);
    runner.transcript_root = temp.path().join("transcripts");
    runner.run_inner(request("worktree"), None).await.unwrap();
    let records = SubagentRunStore::new(&runner.db)
        .list_for_parent("audit-parent")
        .unwrap();
    assert_eq!(records.len(), 1);
    let child = PathBuf::from(records[0].worktree_path.as_ref().unwrap());
    assert!(!root.join("output.txt").exists());
    assert_eq!(
        std::fs::read_to_string(child.join("output.txt")).unwrap(),
        "child"
    );
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    runner.cleanup(&records[0].id).await.unwrap();
    runner.run_inner(request("shared"), None).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("output.txt")).unwrap(),
        "child"
    );
}

struct DeferredCanary(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl deepagent_tools::Tool for DeferredCanary {
    fn descriptor(&self) -> deepagent_tools::ToolDescriptor {
        deepagent_tools::ToolDescriptor {
            name: "workspace_canary".into(),
            description: "Local test canary".into(),
            parameters: json!({"type":"object","properties":{}}),
            risk: deepagent_tools::RiskLevel::High,
            required_permissions: PermissionSet::read_only(),
        }
    }
    fn should_defer(&self) -> bool {
        true
    }
    async fn invoke(&self, _: Value) -> Result<deepagent_tools::ToolOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(deepagent_tools::ToolOutput::success(json!({"canary":true})))
    }
}

#[derive(Default)]
struct DenyRecorder(Mutex<Vec<deepagent_runtime::approval::ApprovalRequest>>);

#[async_trait::async_trait]
impl deepagent_runtime::ApprovalGate for DenyRecorder {
    async fn request(
        &self,
        request: deepagent_runtime::approval::ApprovalRequest,
    ) -> deepagent_runtime::approval::ApprovalDecision {
        self.0.lock().unwrap().push(request);
        deepagent_runtime::approval::ApprovalDecision::Deny
    }
}

#[tokio::test]
async fn rebinding_preserves_deferred_tools_and_parent_approval_denial() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let transport = WorkspaceTransport::new(vec![
        ("tool_search", json!({"query":"select:workspace_canary"})),
        ("workspace_canary", json!({})),
        (
            "write_file",
            json!({"path":"after-denial.txt","content":"child"}),
        ),
    ]);
    let mut runner = audit_child_runner(temp.path(), transport.clone());
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = (*runner.registry).clone();
    registry
        .register(Arc::new(DeferredCanary(invoked.clone())))
        .unwrap();
    runner.registry = Arc::new(registry);
    runner.tool_search_mode = deepagent_builtins::ToolSearchMode::Enabled;
    let denial = Arc::new(DenyRecorder::default());
    assert!(runner.parent_approvals.set(denial.clone()).is_ok());
    assert!(
        DagSubagentExecutor::new(runner)
            .execute(context("deferred", &child))
            .await
            .unwrap()
            .ok
    );
    let requests = transport.requests.lock().unwrap();
    let has_canary = |body: &Value| {
        body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "workspace_canary")
    };
    assert!(!has_canary(&requests[0]));
    assert!(has_canary(&requests[1]));
    drop(requests);
    assert_eq!(denial.0.lock().unwrap().len(), 1);
    assert_eq!(denial.0.lock().unwrap()[0].tool, "workspace_canary");
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
    assert_eq!(transport.output(1)["status"], "error");
    assert_eq!(
        std::fs::read_to_string(child.join("after-denial.txt")).unwrap(),
        "child"
    );
    assert!(!temp.path().join("after-denial.txt").exists());
}

#[tokio::test]
async fn session_search_keeps_parent_project_when_rebinding_after_ui_switch() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let transport = WorkspaceTransport::new(vec![(
        "session_search",
        json!({"query":"workspace-needle"}),
    )]);
    let mut runner = audit_child_runner(temp.path(), transport.clone());
    let projects = Arc::new(crate::project_service::ProjectService::new(
        runner.db.clone(),
    ));
    let parent_project = temp.path().to_string_lossy().into_owned();
    let other_project = child.to_string_lossy().into_owned();
    projects.add_project(&parent_project).unwrap();
    runner.host = runner.host.clone().with_projects(projects.clone());
    runner.registry = Arc::new(
        runner
            .host
            .build_registry(
                temp.path(),
                runner.access,
                None,
                None,
                Some(runner.local_execution_mode),
                false,
            )
            .unwrap()
            .0,
    );
    for project in [&parent_project, &other_project] {
        let mut session = Session::create_in_project(
            &runner.db,
            &SystemClock,
            Some("fixture"),
            Default::default(),
            Some(project),
        )
        .unwrap();
        session
            .append(deepagent_core::event::EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user(format!(
                    "workspace-needle {project}"
                )),
            })
            .unwrap();
    }
    projects.add_project(&other_project).unwrap();
    assert!(
        DagSubagentExecutor::new(runner)
            .execute(context("search", &child))
            .await
            .unwrap()
            .ok
    );
    let output = transport.output(0);
    assert_eq!(output["status"], "ok");
    assert_eq!(output["result"]["project"], parent_project);
    let hits = output["result"]["results"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["project"], parent_project);
}

#[tokio::test]
async fn git_dag_model_tools_deliver_bytes_and_resume_in_reopened_workspace() {
    use deepagent_planner::{PlanDag, PlanNode};
    use deepagent_subagents::{DagScheduler, GitWorktrees};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    repo(&root);
    let head = git(&root, &["rev-parse", "HEAD"]);
    let index = std::fs::read(root.join(".git/index")).unwrap();
    let db_path = temp.path().join("state.sqlite");
    let transport = Arc::new(WorkspaceTransport {
        namespace: "workspace",
        calls: vec![(
            "write_file",
            json!({"path":"delivered.txt","content":"actual producer bytes"}),
        )],
        consumer_calls: Some(vec![
            ("read_file", json!({"path":"delivered.txt"})),
            (
                "write_file",
                json!({"path":"consumed.txt","content":"consumer bytes"}),
            ),
        ]),
        requests: Default::default(),
    });
    let report = {
        let db = Arc::new(Database::open(&db_path).unwrap());
        let mut runner = audit_child_runner_with_db(&root, transport.clone(), db);
        runner.transcript_root = temp.path().join("transcripts");
        let provider = GitWorktrees::new(&root, runner.worktree_base());
        let executor = DagSubagentExecutor::new(runner);
        let dag = PlanDag::new([
            PlanNode::new("producer", "producer-fixture"),
            PlanNode::new("consumer", "consumer-fixture").depends_on(["producer".into()]),
        ])
        .unwrap();
        DagScheduler::new(&executor, &provider)
            .run(&dag)
            .await
            .unwrap()
    }; // Release every original database owner, then reopen from disk.
    assert!(report.all_succeeded, "{report:?}");
    let first = report.results["producer"].output.as_ref().unwrap();
    let next = report.results["consumer"].output.as_ref().unwrap();
    assert_eq!(
        std::fs::read_to_string(Path::new(&next.worktree.path).join("delivered.txt")).unwrap(),
        "actual producer bytes"
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&next.worktree.path).join("consumed.txt")).unwrap(),
        "consumer bytes"
    );
    let read_output = transport.output(0);
    assert_eq!(read_output["status"], "ok");
    assert!(read_output.to_string().contains("actual producer bytes"));
    assert!(!root.join("delivered.txt").exists() && !root.join("consumed.txt").exists());
    let db = Arc::new(Database::open(&db_path).unwrap());
    let resume_transport = WorkspaceTransport::named(
        vec![
            ("read_file", json!({"path":"consumed.txt"})),
            (
                "write_file",
                json!({"path":"resumed.txt","content":"resumed in child"}),
            ),
        ],
        "resumed-workspace",
    );
    let mut runner = audit_child_runner_with_db(&root, resume_transport.clone(), db.clone());
    runner.transcript_root = temp.path().join("transcripts");
    runner
        .resume(&next.worktree.name, "Continue", Default::default())
        .await
        .unwrap();
    assert_eq!(resume_transport.output(0)["status"], "ok");
    assert!(resume_transport
        .output(0)
        .to_string()
        .contains("consumer bytes"));
    let first_resume_input = resume_transport.requests.lock().unwrap()[0]["input"].to_string();
    assert!(
        first_resume_input.contains("actual producer bytes"),
        "prior tool observation must survive the database reopen"
    );
    assert!(
        !first_resume_input.contains("resumed-workspace-call-0"),
        "new read must still execute, not be fabricated from old history"
    );
    assert_eq!(
        std::fs::read_to_string(Path::new(&next.worktree.path).join("resumed.txt")).unwrap(),
        "resumed in child"
    );
    assert!(!root.join("resumed.txt").exists());
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
    let records = SubagentRunStore::new(&db)
        .list_for_parent("audit-parent")
        .unwrap();
    assert_eq!(
        records
            .iter()
            .find(|record| record.id == next.worktree.name)
            .unwrap()
            .resume_count,
        1
    );
    for output in [first, next] {
        assert!(runner.cleanup(&output.worktree.name).await.unwrap());
    }
}

#[tokio::test]
async fn dag_bash_cwd_uses_assigned_root_without_bypassing_allowlist() {
    let temp = tempfile::tempdir().unwrap();
    let child = temp.path().join("child");
    std::fs::create_dir(&child).unwrap();
    // PowerShell's formatted PathInfo output from `pwd` is lost by the
    // existing exit wrapper. Emit a plain string to observe the actual cwd.
    let command = if cfg!(windows) {
        "echo $PWD.Path"
    } else {
        "pwd"
    };
    let transport = WorkspaceTransport::new(vec![
        ("bash", json!({"command":command})),
        ("bash", json!({"command":"deepagent-unlisted-fixture"})),
    ]);
    let runner = audit_child_runner(temp.path(), transport.clone());
    assert!(
        DagSubagentExecutor::new(runner)
            .execute(context("command", &child))
            .await
            .unwrap()
            .ok
    );
    let output = transport.output(0);
    assert_eq!(output["status"], "ok", "{output}");
    let rendered = output["result"]["stdout"].as_str().unwrap();
    assert!(rendered.contains(child.to_str().unwrap()), "{rendered}");
    let rejected = transport.output(1);
    assert_eq!(rejected["status"], "error");
    assert!(rejected.to_string().contains("command not allow-listed"));
}
