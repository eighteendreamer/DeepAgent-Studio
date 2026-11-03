//! Explicit offline audit of two real hosts continuing one persisted session.
//! Model responses are held at a filesystem barrier; no external API is used.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use deepagent_app_core::host_lock::{self, HostRole};
use deepagent_app_core::{AppService, ChatService, MemorySecretStore, SettingsService};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::transport::{EventSink, HttpTransport, MockTransport, TransportRequest};
use deepagent_persistence::run_store::RunStore;

const TEST: &str = "audit_two_hosts_cannot_dispatch_the_same_session_concurrently";
const CHILD_ROOT: &str = "DEEPAGENT_AUDIT_CHAT_CHILD_ROOT";
const CHILD_SESSION: &str = "DEEPAGENT_AUDIT_CHAT_CHILD_SESSION";

struct GatedModel {
    root: PathBuf,
    label: &'static str,
}

async fn wait_for_file(path: &Path, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    while !path.is_file() {
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

#[async_trait]
impl HttpTransport for GatedModel {
    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        std::fs::write(
            self.root.join(format!("{}-request.json", self.label)),
            request.body,
        )
        .map_err(|error| CoreError::other(error.to_string()))?;
        if !wait_for_file(&self.root.join("release"), Duration::from_secs(15)).await {
            return Err(CoreError::other("audit response barrier timed out"));
        }
        for event in [
            serde_json::json!({"type":"response.output_text.delta","delta":format!("audit-{}-answer", self.label)}),
            serde_json::json!({"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}),
        ] {
            if sink.on_event(&event.to_string())? {
                break;
            }
        }
        Ok(())
    }

    async fn stream_cancelled(
        &self,
        request: TransportRequest,
        sink: &mut dyn EventSink,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        tokio::select! {
            result = self.stream(request, sink) => result,
            _ = async {
                while !cancel.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } => Err(CoreError::other("fixture request cancelled")),
        }
    }
}

struct ChildOwner(Child);

impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn chat_for(app: &AppService, root: &Path, label: &'static str) -> ChatService {
    let settings = Arc::new(SettingsService::new(
        app.shared_database(),
        Arc::new(MockTransport::with_get_json(
            r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#,
        )),
        Arc::new(MemorySecretStore::new()),
    ));
    settings.initialize("sk-audit-placeholder").await.unwrap();
    ChatService::new(
        app.shared_database(),
        settings,
        Arc::new(GatedModel {
            root: root.to_path_buf(),
            label,
        }),
        root.join("workspace"),
    )
}

async fn run_turn(chat: ChatService, session_id: String, run_id: &'static str) -> Result<String> {
    chat.run_in_session(
        &format!("{run_id}: reply with audit answer without tools."),
        Some(&session_id),
        None,
        None,
        Vec::new(),
        None,
        false,
        Some(run_id),
        |_| {},
        |_| {},
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_two_hosts_cannot_dispatch_the_same_session_concurrently() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let app = AppService::open(root.join("deepagent.db")).unwrap();
        assert!(matches!(app.host_role(), HostRole::Standby { .. }));
        assert!(app.recover_unfinished_runs().unwrap().is_empty());
        std::fs::write(root.join("child-role"), "standby").unwrap();
        let chat = chat_for(&app, &root, "child").await;
        assert!(!chat.request_cancel("audit-parent-run").unwrap().accepted);
        assert!(
            !chat
                .request_cancel(&std::env::var(CHILD_SESSION).unwrap())
                .unwrap()
                .accepted
        );
        let result = run_turn(
            chat,
            std::env::var(CHILD_SESSION).unwrap(),
            "audit-child-run",
        )
        .await;
        std::fs::write(root.join("child-result"), format!("{result:?}")).unwrap();
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("workspace")).unwrap();
    let app = AppService::open(root.join("deepagent.db")).unwrap();
    assert_eq!(app.host_role(), &HostRole::Primary);
    let chat = chat_for(&app, root, "parent").await;
    let session_id = chat.create_session(None).unwrap();
    let mut parent = tokio::spawn(run_turn(chat, session_id.clone(), "audit-parent-run"));
    assert!(wait_for_file(&root.join("parent-request.json"), Duration::from_secs(10)).await);

    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--test-threads=1"])
            .env(CHILD_ROOT, root)
            .env(CHILD_SESSION, &session_id)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let child_reached_model =
        wait_for_file(&root.join("child-request.json"), Duration::from_secs(3)).await;
    let parent_still_pending = !parent.is_finished();
    let state = |id| {
        RunStore::new(app.database())
            .get(id)
            .unwrap()
            .map(|run| run.state)
    };
    let parent_state = state("audit-parent-run");
    let child_state = state("audit-child-run");
    let child_request =
        std::fs::read_to_string(root.join("child-request.json")).unwrap_or_default();
    let child_sees_parent_input = child_request.contains("audit-parent-run:");
    let child_sees_parent_answer = child_request.contains("audit-parent-answer");

    // Release both requests and join both processes before asserting the audit
    // invariant, so a known failure does not leave pending execution behind.
    std::fs::write(root.join("release"), "release both fixture responses").unwrap();
    let parent_result = match tokio::time::timeout(Duration::from_secs(10), &mut parent).await {
        Ok(result) => result.unwrap(),
        Err(_) => {
            parent.abort();
            let _ = parent.await;
            Err(CoreError::other("parent cleanup timed out"))
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let child_status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            break None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let child_role = std::fs::read_to_string(root.join("child-role")).unwrap_or_default();
    let child_result = std::fs::read_to_string(root.join("child-result")).unwrap_or_default();
    eprintln!(
        "child_role={child_role} parent_pending={parent_still_pending} child_reached_model={child_reached_model} parent_state={parent_state:?} child_state={child_state:?} child_sees_parent_input={child_sees_parent_input} child_sees_parent_answer={child_sees_parent_answer} parent_result_ok={} child_status={child_status:?} child_result={child_result}",
        parent_result.is_ok()
    );
    host_lock::release(root);
    assert!(
        child_status.is_some_and(|status| status.success()),
        "child fixture must exit"
    );
    assert_eq!(
        child_role, "standby",
        "control: independent standby host was admitted"
    );
    assert!(
        parent_still_pending,
        "control: the first model response was held"
    );
    assert!(
        !child_reached_model,
        "a second host must not dispatch a concurrent model turn into the same session"
    );
    assert!(
        child_state.is_none(),
        "conflict must precede durable admission"
    );
    assert!(child_result.contains("is busy"), "{child_result}");
    assert!(parent_result.is_ok(), "owner must finish normally");

    // An independent coordinator can resume only after the owner has finished.
    let resumed = chat_for(&app, root, "resumed").await;
    run_turn(resumed, session_id, "audit-resumed-run")
        .await
        .unwrap();
    let request = std::fs::read_to_string(root.join("resumed-request.json")).unwrap();
    assert!(request.contains("audit-parent-answer"));
    assert!(!request.contains("audit-child-run:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_services_can_dispatch_different_sessions_concurrently() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("workspace")).unwrap();
    let app = AppService::open(root.join("deepagent.db")).unwrap();
    let first = chat_for(&app, root, "first").await;
    let second = chat_for(&app, root, "second").await;
    let sid1 = first.create_session(None).unwrap();
    let sid2 = second.create_session(None).unwrap();
    let turn1 = tokio::spawn(run_turn(first, sid1, "different-first"));
    let turn2 = tokio::spawn(run_turn(second, sid2, "different-second"));
    let first_dispatched =
        wait_for_file(&root.join("first-request.json"), Duration::from_secs(10)).await;
    let second_dispatched =
        wait_for_file(&root.join("second-request.json"), Duration::from_secs(3)).await;
    let both_pending = !turn1.is_finished() && !turn2.is_finished();
    std::fs::write(root.join("release"), "release").unwrap();
    let result1 = turn1.await.unwrap();
    let result2 = turn2.await.unwrap();
    host_lock::release(root);
    assert!(first_dispatched && second_dispatched && both_pending);
    assert!(result1.is_ok() && result2.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_queue_interrupts_owner_and_remains_owned_until_follower_finishes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("workspace")).unwrap();
    let app = AppService::open(root.join("deepagent.db")).unwrap();
    let chat = chat_for(&app, root, "local").await;
    let competitor = chat_for(&app, root, "competitor").await;
    let sid = chat.create_session(None).unwrap();
    let owner = tokio::spawn(run_turn(chat.clone(), sid.clone(), "local-owner"));
    assert!(wait_for_file(&root.join("local-request.json"), Duration::from_secs(10)).await);
    let follower = tokio::spawn(run_turn(chat, sid.clone(), "local-follower"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let follower_dispatched = loop {
        let request = std::fs::read_to_string(root.join("local-request.json")).unwrap_or_default();
        if request.contains("local-follower:") {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let refused = run_turn(competitor, sid, "local-competitor").await;
    std::fs::write(root.join("release"), "release").unwrap();
    assert!(owner.await.unwrap().is_ok());
    assert!(follower.await.unwrap().is_ok());
    host_lock::release(root);
    assert!(follower_dispatched);
    assert!(refused.unwrap_err().to_string().contains("is busy"));
    let store = RunStore::new(app.database());
    assert_eq!(
        store
            .get("local-owner")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("cancelled")
    );
    assert_eq!(
        store
            .get("local-follower")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("succeeded")
    );
    assert!(store.get("local-competitor").unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_setup_and_aborted_future_release_dispatch_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("workspace")).unwrap();
    let app = AppService::open(root.join("deepagent.db")).unwrap();
    let chat = chat_for(&app, root, "aborted").await;
    let sid = chat.create_session(None).unwrap();
    // InputIngress rejects whitespace before history recovery/model dispatch.
    let failed = chat
        .run_in_session(
            "   ",
            Some(&sid),
            None,
            None,
            Vec::new(),
            None,
            false,
            Some("failed-setup"),
            |_| {},
            |_| {},
        )
        .await;
    assert!(failed.is_err());
    assert_eq!(
        RunStore::new(app.database())
            .get("failed-setup")
            .unwrap()
            .unwrap()
            .terminal_kind
            .as_deref(),
        Some("failed")
    );
    assert!(!root.join("aborted-request.json").exists());
    let owner = tokio::spawn(run_turn(chat.clone(), sid.clone(), "future-abort"));
    assert!(wait_for_file(&root.join("aborted-request.json"), Duration::from_secs(10)).await);
    owner.abort();
    assert!(owner.await.unwrap_err().is_cancelled());
    assert!(!chat.request_cancel("future-abort").unwrap().accepted);
    let resumed = chat_for(&app, root, "after-abort").await;
    assert!(!resumed.request_cancel("future-abort").unwrap().accepted);
    std::fs::write(root.join("release"), "release").unwrap();
    run_turn(resumed, sid, "after-future-abort").await.unwrap();
    host_lock::release(root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_owner_process_releases_dispatch_without_ttl() {
    const ROOT: &str = "DEEPAGENT_DISPATCH_CRASH_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        let root = PathBuf::from(root);
        let app = AppService::open(root.join("deepagent.db")).unwrap();
        let chat = chat_for(&app, &root, "crash-owner").await;
        let sid = chat.create_session(None).unwrap();
        std::fs::write(root.join("crash-session"), &sid).unwrap();
        run_turn(chat, sid, "crash-owner-run").await.unwrap();
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir(root.join("workspace")).unwrap();
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "killed_owner_process_releases_dispatch_without_ttl",
                "--test-threads=1",
            ])
            .env(ROOT, root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    assert!(
        wait_for_file(
            &root.join("crash-owner-request.json"),
            Duration::from_secs(10)
        )
        .await
    );
    let app = AppService::open(root.join("deepagent.db")).unwrap();
    assert!(matches!(app.host_role(), HostRole::Standby { .. }));
    let sid = std::fs::read_to_string(root.join("crash-session")).unwrap();
    let chat = chat_for(&app, root, "after-crash").await;
    let error = run_turn(chat.clone(), sid.clone(), "before-crash")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("is busy"));
    assert!(!root.join("after-crash-request.json").exists());
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    std::fs::write(root.join("release"), "release").unwrap();
    run_turn(chat, sid, "after-crash").await.unwrap();
    assert!(root.join("after-crash-request.json").is_file());
    // This tests OS ownership release, not repair of the crashed run's durable
    // terminal. Startup recovery/terminal consistency remain separate H items.
    assert!(RunStore::new(app.database())
        .get("crash-owner-run")
        .unwrap()
        .unwrap()
        .finished_at
        .is_none());
    host_lock::release(root);
}
