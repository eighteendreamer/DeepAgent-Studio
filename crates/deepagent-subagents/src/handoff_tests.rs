//! Real-Git output-delivery regressions. All repositories and files are disposable.
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_planner::{PlanDag, PlanNode};

use crate::{
    DagScheduler, GitWorktrees, SubAgentContext, SubAgentExecutor, SubAgentResult, Worktree,
    WorktreeOutput, WorktreeProvider,
};

fn git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Handoff Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("Git is required for handoff regression tests");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn fixture() -> (tempfile::TempDir, PathBuf, GitWorktrees) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo");
    std::fs::create_dir(&root).unwrap();
    git(&root, &["init"]);
    std::fs::write(root.join("base.txt"), "base\n").unwrap();
    std::fs::write(root.join("deleted.txt"), "remove me\n").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
    std::fs::write(root.join(".gitattributes"), "* -text\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fixture"]);
    let provider = GitWorktrees::new(&root, temp.path().join("worktrees"));
    (temp, root, provider)
}

#[tokio::test]
async fn snapshot_preserves_source_index_head_and_main_dirty_state() {
    let (_temp, root, provider) = fixture();
    std::fs::write(root.join("base.txt"), "main staged\n").unwrap();
    git(&root, &["add", "."]);
    std::fs::write(root.join("main-only.txt"), "keep\n").unwrap();
    let main_status = git(&root, &["status", "--porcelain=v1"]);
    let main_index = std::fs::read(root.join(".git/index")).unwrap();
    let main_head = git(&root, &["rev-parse", "HEAD"]);

    let source = provider.create("source").await.unwrap();
    let path = Path::new(&source.path);
    std::fs::write(path.join("committed.txt"), "committed\n").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "child output"]);
    std::fs::write(path.join("base.txt"), "staged child\n").unwrap();
    git(path, &["add", "."]);
    std::fs::write(path.join("base.txt"), "latest child bytes\n").unwrap();
    std::fs::write(path.join("new.txt"), "new bytes\n").unwrap();
    std::fs::write(path.join("ignored.txt"), "kept locally\n").unwrap();
    std::fs::remove_file(path.join("deleted.txt")).unwrap();
    let head = git(path, &["rev-parse", "HEAD"]);
    let index_path = git(path, &["rev-parse", "--git-path", "index"]);
    let index_before = std::fs::read(&index_path).unwrap();
    let staged_before = git(path, &["diff", "--cached"]);
    let output = provider.snapshot(&source).await.unwrap();
    assert_eq!(git(path, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read(index_path).unwrap(), index_before);
    assert_eq!(git(path, &["diff", "--cached"]), staged_before);
    assert!(path.join("ignored.txt").is_file());
    let next = provider
        .create_from("next", std::slice::from_ref(&output))
        .await
        .unwrap();
    let next_path = Path::new(&next.path);
    assert_eq!(
        std::fs::read_to_string(next_path.join("base.txt")).unwrap(),
        "latest child bytes\n"
    );
    assert_eq!(
        std::fs::read_to_string(next_path.join("new.txt")).unwrap(),
        "new bytes\n"
    );
    assert!(next_path.join("committed.txt").is_file());
    assert!(!next_path.join("deleted.txt").exists());
    assert!(!next_path.join("ignored.txt").exists());
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), main_head);
    assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), main_index);
    assert_eq!(git(&root, &["status", "--porcelain=v1"]), main_status);
    assert_eq!(
        git(&root, &["rev-parse", "refs/deepagent/dag-output/source"]),
        output.revision.unwrap()
    );
    // Explicit cleanup only: descendant history still protects the output commit.
    provider.remove("source").await.unwrap();
    assert!(!path.exists());
    assert!(git(&root, &["for-each-ref", "refs/deepagent/dag-output/source"]).is_empty());
    assert!(next_path.join("new.txt").is_file());
}

#[tokio::test]
async fn frozen_baseline_does_not_import_later_parent_commits() {
    let (_temp, root, provider) = fixture();
    provider.create("first").await.unwrap();
    std::fs::write(root.join("later.txt"), "not in this DAG\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "later parent change"]);
    let second = provider.create("second").await.unwrap();
    assert!(!Path::new(&second.path).join("later.txt").exists());
}

#[tokio::test]
async fn explicit_cleanup_failure_keeps_worktree_and_snapshot_ref() {
    let (_temp, root, provider) = fixture();
    let source = provider.create("locked_source").await.unwrap();
    std::fs::write(
        Path::new(&source.path).join("output.txt"),
        "keep until explicit cleanup",
    )
    .unwrap();
    let output = provider.snapshot(&source).await.unwrap();
    git(&root, &["worktree", "lock", &source.path]);
    assert!(provider.remove(&source.name).await.is_err());
    assert!(Path::new(&source.path).join("output.txt").is_file());
    assert_eq!(
        git(
            &root,
            &["rev-parse", "refs/deepagent/dag-output/locked_source"]
        ),
        output.revision.unwrap()
    );
    git(&root, &["worktree", "unlock", &source.path]);
    provider.remove(&source.name).await.unwrap();
    assert!(!Path::new(&source.path).exists());
}

#[tokio::test]
async fn preexisting_output_ref_is_not_overwritten_or_dereferenced() {
    let (_temp, root, provider) = fixture();
    let source = provider.create("source").await.unwrap();
    std::fs::write(Path::new(&source.path).join("output.txt"), "retained").unwrap();
    let head = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["update-ref", "refs/heads/fixture_target", &head]);
    git(
        &root,
        &[
            "symbolic-ref",
            "refs/deepagent/dag-output/source",
            "refs/heads/fixture_target",
        ],
    );
    assert!(provider.snapshot(&source).await.is_err());
    assert_eq!(
        git(&root, &["rev-parse", "refs/heads/fixture_target"]),
        head
    );
    assert!(Path::new(&source.path).join("output.txt").is_file());
    provider.remove(&source.name).await.unwrap();
    assert_eq!(
        git(&root, &["rev-parse", "refs/heads/fixture_target"]),
        head
    );
    assert!(git(&root, &["for-each-ref", "refs/deepagent/dag-output/source"]).is_empty());
}

#[tokio::test]
async fn diamond_fan_in_combines_outputs_without_reapplying_common_changes() {
    let (_temp, root, provider) = fixture();
    let base = provider.create("base").await.unwrap();
    std::fs::write(Path::new(&base.path).join("base.txt"), "common edit\n").unwrap();
    let common = provider.snapshot(&base).await.unwrap();
    let mut outputs = Vec::new();
    for name in ["left", "right"] {
        let branch = provider
            .create_from(name, std::slice::from_ref(&common))
            .await
            .unwrap();
        std::fs::write(Path::new(&branch.path).join(format!("{name}.txt")), name).unwrap();
        outputs.push(provider.snapshot(&branch).await.unwrap());
    }
    let joined = provider.create_from("joined", &outputs).await.unwrap();
    let path = Path::new(&joined.path);
    assert_eq!(
        std::fs::read_to_string(path.join("base.txt")).unwrap(),
        "common edit\n"
    );
    for name in ["left", "right"] {
        assert_eq!(
            std::fs::read_to_string(path.join(format!("{name}.txt"))).unwrap(),
            name
        );
    }
    let unchanged = provider.snapshot(&joined).await.unwrap();
    assert!(unchanged.revision.is_some());
    assert_eq!(
        std::fs::read_to_string(root.join("base.txt")).unwrap(),
        "base\n"
    );
}

struct WritingExecutor {
    seen: Mutex<Vec<String>>,
    mode: &'static str,
}

#[async_trait]
impl SubAgentExecutor for WritingExecutor {
    async fn execute(&self, ctx: SubAgentContext) -> Result<SubAgentResult> {
        self.seen.lock().unwrap().push(ctx.node_id.clone());
        let filename = if self.mode == "conflict" {
            "base.txt".into()
        } else {
            format!("{}.txt", ctx.node_id)
        };
        std::fs::write(Path::new(&ctx.worktree.path).join(filename), &ctx.node_id).unwrap();
        if self.mode == "error" && ctx.node_id == "a" {
            return Err(CoreError::other("injected executor error"));
        }
        Ok(SubAgentResult::success(ctx.node_id, "written"))
    }
}

fn fan_in() -> PlanDag {
    PlanDag::new([
        PlanNode::new("a", "a"),
        PlanNode::new("b", "b"),
        PlanNode::new("join", "join").depends_on(["a".to_string(), "b".to_string()]),
        PlanNode::new("last", "last").depends_on(["join".to_string()]),
    ])
    .unwrap()
}

#[tokio::test]
async fn integration_conflict_blocks_executor_and_preserves_all_source_outputs() {
    let (_temp, root, provider) = fixture();
    let executor = WritingExecutor {
        seen: Mutex::default(),
        mode: "conflict",
    };
    let report = DagScheduler::new(&executor, &provider)
        .run(&fan_in())
        .await
        .unwrap();
    assert!(!report.all_succeeded);
    assert_eq!(report.failed_nodes(), [&"join".to_string()]);
    assert!(report.results["join"]
        .summary
        .contains("dependency integration failed"));
    assert_eq!(report.skipped, ["last"]);
    let seen = executor.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    for id in ["a", "b"] {
        let output = report.results[id].output.as_ref().unwrap();
        assert!(output.revision.is_some());
        assert_eq!(
            std::fs::read_to_string(Path::new(&output.worktree.path).join("base.txt")).unwrap(),
            id
        );
    }
    assert_eq!(
        std::fs::read_to_string(root.join("base.txt")).unwrap(),
        "base\n"
    );
}

#[tokio::test]
async fn executor_error_preserves_partial_files_and_independent_sibling_report() {
    let (_temp, _root, provider) = fixture();
    let executor = WritingExecutor {
        seen: Mutex::default(),
        mode: "error",
    };
    let report = DagScheduler::new(&executor, &provider)
        .run(&fan_in())
        .await
        .unwrap();
    assert!(!report.results["a"].ok);
    assert!(report.results["b"].ok);
    let output = report.results["a"].output.as_ref().unwrap();
    assert!(output.revision.is_none());
    assert!(Path::new(&output.worktree.path).join("a.txt").is_file());
    assert_eq!(report.skipped, ["join", "last"]);
}

struct PublicationFault(GitWorktrees);

#[async_trait]
impl WorktreeProvider for PublicationFault {
    async fn create(&self, name: &str) -> Result<Worktree> {
        self.0.create(name).await
    }
    async fn create_from(&self, name: &str, inputs: &[WorktreeOutput]) -> Result<Worktree> {
        self.0.create_from(name, inputs).await
    }
    async fn snapshot(&self, _worktree: &Worktree) -> Result<WorktreeOutput> {
        Err(CoreError::other("injected publication failure"))
    }
    async fn remove(&self, _name: &str) -> Result<()> {
        panic!("scheduler must never call destructive cleanup")
    }
}

#[tokio::test]
async fn publication_failure_is_not_reported_as_success_and_keeps_files() {
    let (_temp, _root, provider) = fixture();
    let executor = WritingExecutor {
        seen: Mutex::default(),
        mode: "ok",
    };
    let report = DagScheduler::new(&executor, &PublicationFault(provider))
        .run(&fan_in())
        .await
        .unwrap();
    assert!(!report.all_succeeded);
    for result in report.results.values() {
        assert!(!result.ok);
        assert!(result.summary.contains("publication failed"));
        let output = result.output.as_ref().unwrap();
        assert!(Path::new(&output.worktree.path)
            .join(format!("{}.txt", result.node_id))
            .is_file());
    }
    assert_eq!(report.skipped, ["join", "last"]);
}

#[tokio::test]
async fn concurrent_and_repeated_dags_use_distinct_retained_worktrees() {
    let (_temp, _root, provider) = fixture();
    let executor = WritingExecutor {
        seen: Mutex::default(),
        mode: "ok",
    };
    let dag = PlanDag::new([PlanNode::new("a", "a")]).unwrap();
    let scheduler = DagScheduler::new(&executor, &provider);
    let (first, second) = tokio::join!(scheduler.run(&dag), scheduler.run(&dag));
    let third = scheduler.run(&dag).await.unwrap();
    let paths: std::collections::BTreeSet<_> = [first.unwrap(), second.unwrap(), third]
        .into_iter()
        .map(|report| {
            assert!(report.all_succeeded);
            let output = report.results["a"].output.as_ref().unwrap();
            assert!(Path::new(&output.worktree.path).join("a.txt").is_file());
            output.worktree.path.clone()
        })
        .collect();
    assert_eq!(paths.len(), 3);
}

#[tokio::test]
async fn dropping_execution_future_does_not_delete_partial_output() {
    struct Waiting {
        entered: tokio::sync::Notify,
        path: Mutex<Option<PathBuf>>,
    }
    #[async_trait]
    impl SubAgentExecutor for Waiting {
        async fn execute(&self, ctx: SubAgentContext) -> Result<SubAgentResult> {
            let path = PathBuf::from(ctx.worktree.path);
            std::fs::write(path.join("partial.txt"), "partial").unwrap();
            *self.path.lock().unwrap() = Some(path);
            self.entered.notify_one();
            std::future::pending().await
        }
    }
    let (_temp, _root, provider) = fixture();
    let executor = Waiting {
        entered: Default::default(),
        path: Mutex::default(),
    };
    let dag = PlanDag::new([PlanNode::new("wait", "wait")]).unwrap();
    let scheduler = DagScheduler::new(&executor, &provider);
    {
        let running = scheduler.run(&dag);
        tokio::pin!(running);
        tokio::select! {
            _ = &mut running => panic!("executor must still be pending"),
            _ = executor.entered.notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(10)) => panic!("executor did not enter"),
        }
    }
    assert_eq!(
        std::fs::read_to_string(
            executor
                .path
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .join("partial.txt")
        )
        .unwrap(),
        "partial"
    );
}

#[tokio::test]
async fn invalid_or_unpublished_input_and_git_timeout_fail_explicitly() {
    let (_temp, _root, provider) = fixture();
    let source = provider.create("source").await.unwrap();
    for revision in [None, Some("invalid-revision".to_string())] {
        let input = WorktreeOutput {
            worktree: source.clone(),
            revision,
        };
        assert!(provider.create_from("blocked", &[input]).await.is_err());
    }
    let timed = provider
        .clone()
        .with_timeout(std::time::Duration::from_nanos(1));
    assert!(timed.snapshot(&source).await.is_err());
    assert!(Path::new(&source.path).is_dir());
    let no_repo = tempfile::tempdir().unwrap();
    let invalid = GitWorktrees::new(no_repo.path(), no_repo.path().join("worktrees"));
    assert!(invalid.create("child").await.is_err());
}
