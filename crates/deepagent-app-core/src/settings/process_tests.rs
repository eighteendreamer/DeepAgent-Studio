//! Real primary/standby processes, offline discovery and synthetic file SQLite.
use super::*;
use crate::{host_lock::HostRole, AppService, MemorySecretStore};
use async_trait::async_trait;
use deepagent_models::transport::{EventSink, TransportRequest};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const ENTRY: &str = "settings::process_tests::h87_host_process_entry";
const ROOT: &str = "DEEPAGENT_H87_ROOT";
const ACTOR: &str = "DEEPAGENT_H87_ACTOR";
const BUDGET: Duration = Duration::from_secs(30);

struct Discovery {
    root: PathBuf,
    armed: AtomicBool,
}

#[async_trait]
impl HttpTransport for Discovery {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other("unexpected streaming in settings fixture"))
    }

    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        if self.armed.load(Ordering::Acquire) {
            std::fs::write(self.root.join("refresh-waiting"), b"ready").unwrap();
            let deadline = Instant::now() + BUDGET;
            while !self.root.join("resume").is_file() {
                if Instant::now() >= deadline {
                    return Err(CoreError::other("H87 discovery barrier timed out"));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        Ok(repair_tests::MODELS.to_owned())
    }
}

fn assert_restrictions(settings: &SettingsService) {
    let saved = settings.load().unwrap().unwrap();
    assert_eq!(saved.sandbox_mode, SandboxMode::ReadOnly);
    assert_eq!(saved.hooks_json, r#"{"hooks":{}}"#);
    assert_eq!(saved.permission_rules.deny, ["Bash(rm:*)"]);
    assert_eq!(saved.welcome_name, "standby committed");
}

#[tokio::test]
#[ignore = "child fixture: parents below execute it explicitly in separate processes"]
async fn h87_host_process_entry() {
    let root = PathBuf::from(std::env::var_os(ROOT).expect("H87 child root"));
    let actor = std::env::var(ACTOR).unwrap();
    let app = AppService::open(root.join("settings.sqlite")).unwrap();
    let discovery = Arc::new(Discovery {
        root: root.clone(),
        armed: AtomicBool::new(false),
    });
    let settings = SettingsService::new(
        app.shared_database(),
        discovery.clone(),
        Arc::new(MemorySecretStore::new()),
    );
    match actor.as_str() {
        "primary" => {
            assert_eq!(app.host_role(), &HostRole::Primary);
            settings
                .initialize("h87-synthetic-credential")
                .await
                .unwrap();
            assert_eq!(
                settings.sandbox_mode().unwrap(),
                SandboxMode::WorkspaceWrite
            );
            discovery.armed.store(true, Ordering::Release);
            let view = settings.refresh_models().await.unwrap();
            assert_eq!(view.sandbox_mode, "read_only");
            assert_restrictions(&settings);
        }
        "standby" => {
            assert!(matches!(app.host_role(), HostRole::Standby { .. }));
            settings.set_sandbox_mode(SandboxMode::ReadOnly).unwrap();
            settings.set_hooks_json(r#"{"hooks":{}}"#).unwrap();
            settings
                .set_permission_rules(PermissionRules {
                    deny: vec!["Bash(rm:*)".into()],
                    ..Default::default()
                })
                .unwrap();
            settings.set_welcome_name("standby committed").unwrap();
            assert_restrictions(&settings);
        }
        "reopen" => {
            assert_eq!(app.host_role(), &HostRole::Primary);
            assert_restrictions(&settings);
        }
        _ => panic!("unknown H87 actor"),
    }
}

struct ChildOwner {
    child: Child,
    log: PathBuf,
}

impl ChildOwner {
    fn spawn(root: &Path, actor: &str) -> Self {
        let log = root.join(format!("{actor}.log"));
        let output = std::fs::File::create(&log).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                ENTRY,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ROOT, root)
            .env(ACTOR, actor)
            .stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(Stdio::from(output))
            .spawn()
            .unwrap();
        Self { child, log }
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + BUDGET;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{}",
                    std::fs::read_to_string(&self.log).unwrap()
                );
                return;
            }
            assert!(Instant::now() < deadline, "H87 child exit timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_refresh(&mut self, root: &Path) {
        let deadline = Instant::now() + BUDGET;
        while !root.join("refresh-waiting").is_file() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(&self.log).unwrap()
            );
            assert!(Instant::now() < deadline, "H87 refresh barrier timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn process_interleaving(kill_primary: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut primary = ChildOwner::spawn(root, "primary");
    primary.wait_for_refresh(root);
    let mut standby = ChildOwner::spawn(root, "standby");
    standby.finish();
    if kill_primary {
        primary.child.kill().unwrap();
        primary.child.wait().unwrap();
    } else {
        std::fs::write(root.join("resume"), b"resume").unwrap();
        primary.finish();
    }
    // A new process claims the released OS lock and reads durable state.
    ChildOwner::spawn(root, "reopen").finish();
}

#[test]
fn h87_primary_refresh_preserves_standby_commit_and_cold_reopen() {
    process_interleaving(false);
}

#[test]
fn h87_primary_exit_during_refresh_preserves_standby_commit_and_releases_lock() {
    process_interleaving(true);
}
