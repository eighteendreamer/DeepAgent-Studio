//! Explicit offline audit of two real hosts continuing one persisted session.
//! Model responses are held at a filesystem barrier; no external API is used.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
            serde_json::json!({"type":"response.completed","response":{"status":"completed"}}),
        ] {
            if sink.on_event(&event.to_string())? {
                break;
            }
        }
        Ok(())
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
#[ignore = "audit: session dispatch ownership must cover independent host processes"]
async fn audit_two_hosts_cannot_dispatch_the_same_session_concurrently() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let app = AppService::open(root.join("deepagent.db")).unwrap();
        assert!(matches!(app.host_role(), HostRole::Standby { .. }));
        assert!(app.recover_unfinished_runs().unwrap().is_empty());
        std::fs::write(root.join("child-role"), "standby").unwrap();
        let chat = chat_for(&app, &root, "child").await;
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

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--ignored", "--test-threads=1"])
        .env(CHILD_ROOT, root)
        .env(CHILD_SESSION, &session_id)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
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
}
