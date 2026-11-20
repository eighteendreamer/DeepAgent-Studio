//! Compare real Git status, the desktop Git service, and the model's Git context.

use super::*;
use deepagent_app_core::{GitService, ProjectService};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Debug)]
enum Change {
    Clean,
    Untracked,
    Staged,
    Unstaged,
    Deleted,
}

fn git(root: &Path, args: &[&str]) -> String {
    // Identity is scoped to this child process; no repository/global config is edited.
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Harness Audit")
        .env("GIT_AUTHOR_EMAIL", "harness-audit@example.invalid")
        .env("GIT_COMMITTER_NAME", "Harness Audit")
        .env("GIT_COMMITTER_EMAIL", "harness-audit@example.invalid")
        .output()
        .expect("Git must be installed for this real-repository audit");
    assert!(
        output.status.success(),
        "fixture git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("fixture uses UTF-8 paths")
}

#[derive(Default)]
struct GitContextModel(Mutex<Vec<serde_json::Value>>);

#[async_trait]
impl HttpTransport for GitContextModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body)?);
        emit_responses_events(
            sink,
            &responses_fixture::completed(
                "resp-git-context-audit",
                "deepseek-flash",
                responses_fixture::text("msg-git-context-audit", "AUDIT_GIT_CONTEXT_DONE"),
                serde_json::json!({"input_tokens":16,"output_tokens":4,"total_tokens":20}),
            ),
        )
    }
}

#[derive(Debug)]
struct Observation {
    raw_status: String,
    desktop_paths: Vec<String>,
    model_paths: Vec<String>,
    model_dirty: bool,
    wire_git_block: String,
}

async fn observe(change: Change, relative: &str) -> Observation {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();
    git(root, &["init", "-q"]);
    std::fs::write(root.join(".gitignore"), ".deepagent/\n").unwrap();
    std::fs::write(root.join("baseline.txt"), "baseline\n").unwrap();
    let target = root.join(relative);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    if matches!(change, Change::Clean | Change::Unstaged | Change::Deleted) {
        std::fs::write(&target, "baseline\n").unwrap();
    }
    git(root, &["add", "--all"]);
    git(
        root,
        &["commit", "-q", "--no-gpg-sign", "-m", "audit baseline"],
    );
    match change {
        Change::Clean => {}
        Change::Untracked | Change::Unstaged => {
            std::fs::write(&target, "uncommitted user edit\n").unwrap();
        }
        Change::Staged => {
            std::fs::write(&target, "staged user edit\n").unwrap();
            git(root, &["add", "--", relative]);
        }
        Change::Deleted => {
            // This is a single fixture file beneath the owned TempDir, not user data.
            std::fs::remove_file(&target).unwrap();
        }
    }
    let raw_status = git(root, &["status", "--porcelain=v1", "-z"]);
    let expected_status = match change {
        Change::Clean => String::new(),
        Change::Untracked => format!("?? {relative}\0"),
        Change::Staged => format!("A  {relative}\0"),
        Change::Unstaged => format!(" M {relative}\0"),
        Change::Deleted => format!(" D {relative}\0"),
    };
    assert_eq!(
        raw_status, expected_status,
        "real Git establishes fixture state"
    );

    let database = Arc::new(Database::open_in_memory().unwrap());
    let desktop_git = GitService::new(Arc::new(ProjectService::new(database.clone())));
    let desktop_paths = desktop_git
        .changes(root.to_str().unwrap())
        .unwrap()
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    let context = deepagent_workspace::detect_git_context(root).expect("real repository");
    let settings = Arc::new(SettingsService::new(
        database.clone(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    crate::registry_fixture::configure(&settings, "offline-audit-placeholder")
        .await
        .unwrap();
    let model = Arc::new(GitContextModel::default());
    let chat = ChatService::new(database, settings, model.clone(), root);
    chat.run_in_session(
        "Inspect the existing Git context without running any tools.",
        None,
        None,
        None,
        Vec::new(),
        None,
        false,
        Some("audit-git-context"),
        |_| {},
        |_| {},
    )
    .await
    .unwrap();
    let events = chat.run_events("audit-git-context", None).unwrap();
    let terminals: Vec<_> = events
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .collect();
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].data["kind"], "succeeded");
    let requests = model.0.lock().unwrap();
    assert_eq!(requests.len(), 1, "one real serialized model request");
    let instructions = super::cache_prefix::wire_system_text(&requests[0]);
    let git_tail = instructions
        .split_once("# Git\n")
        .expect("Git block reaches wire")
        .1;
    let wire_git_block = format!(
        "# Git\n{}",
        git_tail.split("- Recent commits:").next().unwrap()
    );
    assert!(instructions.contains(context.to_prompt_block().trim_end()));
    assert_eq!(
        git(root, &["status", "--porcelain=v1", "-z"]),
        raw_status,
        "the run must not change the fixture's Git state"
    );
    Observation {
        raw_status,
        desktop_paths,
        model_paths: context.changed_files,
        model_dirty: context.has_uncommitted,
        wire_git_block,
    }
}

#[tokio::test]
async fn git_context_clean_untracked_and_staged_controls_match_wire() {
    for change in [Change::Clean, Change::Untracked, Change::Staged] {
        let outcome = observe(change, "a").await;
        let dirty = !matches!(change, Change::Clean);
        let expected_paths = if dirty {
            vec!["a".to_string()]
        } else {
            Vec::new()
        };
        eprintln!("Git context control {change:?}: {outcome:?}");
        assert_eq!(outcome.model_dirty, dirty);
        assert_eq!(outcome.desktop_paths, expected_paths);
        assert_eq!(outcome.model_paths, expected_paths);
        assert_eq!(
            outcome.wire_git_block.contains("Working tree: clean"),
            !dirty
        );
    }
}

#[tokio::test]
async fn audit_unstaged_git_context_preserves_dirty_state_and_paths_on_wire() {
    let mut mismatches = Vec::new();
    for (change, relative) in [
        (Change::Unstaged, "a"),
        (Change::Deleted, "a"),
        (Change::Unstaged, "src/main.rs"),
    ] {
        let outcome = observe(change, relative).await;
        eprintln!("Git context audit {change:?}/{relative}: {outcome:?}");
        assert!(
            !outcome.raw_status.is_empty(),
            "Git must report a real change"
        );
        assert_eq!(outcome.desktop_paths, vec![relative]);
        if !outcome.model_dirty
            || outcome.model_paths != vec![relative]
            || outcome.wire_git_block.contains("Working tree: clean")
        {
            mismatches.push(format!("{change:?}/{relative}"));
        }
    }
    assert!(
        mismatches.is_empty(),
        "real unstaged changes must reach model context intact: {mismatches:?}"
    );
}
