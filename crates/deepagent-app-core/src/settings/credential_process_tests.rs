//! H88 real host processes: encrypted SQLite, distinct TEMP, offline discovery.
use super::*;
use crate::{host_lock::HostRole, AppService, MemorySecretStore, SqliteSecretStore};
use async_trait::async_trait;
use deepagent_models::transport::{EventSink, TransportRequest};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const ENTRY: &str = "settings::credential_process_tests::h88_host_process_entry";
const ROOT: &str = "DEEPAGENT_H88_ROOT";
const ACTOR: &str = "DEEPAGENT_H88_ACTOR";
const CASE: &str = "DEEPAGENT_H88_CASE";
const BUDGET: Duration = Duration::from_secs(30);
const OLD: &str = "h88-process-synthetic-old";
const NEW: &str = "h88-process-synthetic-new";
const MODELS: &str =
    r#"{"data":[{"id":"deepseek-v4-flash","object":"model","owned_by":"deepseek"}]}"#;

async fn await_resume(root: &Path) -> Result<()> {
    let deadline = Instant::now() + BUDGET;
    while !root.join("resume").is_file() {
        if Instant::now() >= deadline {
            return Err(CoreError::other("H88 process barrier timed out"));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}

struct Discovery {
    root: PathBuf,
    armed: AtomicBool,
}

#[async_trait]
impl HttpTransport for Discovery {
    async fn stream(&self, _: TransportRequest, _: &mut dyn EventSink) -> Result<()> {
        Err(CoreError::other("unexpected H88 stream"))
    }

    async fn get_json(&self, _: &str, _: &str) -> Result<String> {
        if self.armed.load(Ordering::Acquire) {
            std::fs::write(self.root.join("waiting"), b"discovery").unwrap();
            await_resume(&self.root).await?;
        }
        Ok(MODELS.to_owned())
    }
}

// Pause after the REAL encrypted write, while the production owner still holds
// its gate and durable committing marker. No alternate persistence path.
struct PausedSecrets {
    inner: SqliteSecretStore,
    root: PathBuf,
    armed: AtomicBool,
}

impl SecretStore for PausedSecrets {
    fn set(&self, name: &str, key: &str) -> Result<()> {
        self.inner.set(name, key)?;
        if self.armed.load(Ordering::Acquire) {
            std::fs::write(self.root.join("waiting"), b"committing").unwrap();
            let deadline = Instant::now() + BUDGET;
            while !self.root.join("resume").is_file() {
                if Instant::now() >= deadline {
                    return Err(CoreError::other("H88 commit barrier timed out"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<String>> {
        self.inner.get(name)
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.inner.delete(name)
    }
}

#[tokio::test]
#[ignore = "child fixture: default parent tests explicitly execute this entry"]
async fn h88_host_process_entry() {
    let root = PathBuf::from(std::env::var_os(ROOT).expect("H88 child root"));
    let actor = std::env::var(ACTOR).unwrap();
    let case = std::env::var(CASE).unwrap();
    let app = AppService::open(root.join("settings.sqlite")).unwrap();
    let discovery = Arc::new(Discovery {
        root: root.clone(),
        armed: AtomicBool::new(false),
    });
    let wrapping = Arc::new(MemorySecretStore::new());
    wrapping
        .set(
            "deepagent_secret_wrap_key",
            "H88 fixed synthetic wrapping value",
        )
        .unwrap();
    let secrets = Arc::new(PausedSecrets {
        inner: SqliteSecretStore::new(app.shared_database(), wrapping),
        root: root.clone(),
        armed: AtomicBool::new(false),
    });
    let settings = SettingsService::new(app.shared_database(), discovery.clone(), secrets.clone());
    match actor.as_str() {
        "primary" => {
            assert_eq!(app.host_role(), &HostRole::Primary);
            settings.initialize(OLD).await.unwrap();
            if case == "crash" {
                secrets.armed.store(true, Ordering::Release);
                settings.initialize(NEW).await.unwrap();
                panic!("parent must kill the incomplete commit");
            }
            discovery.armed.store(true, Ordering::Release);
            let result = if case == "initialize" {
                settings.initialize("h88-process-older-attempt").await
            } else {
                settings.refresh_models().await
            };
            assert!(
                result.is_err(),
                "stale discovery must lose publication authority"
            );
        }
        "standby" => {
            assert!(matches!(app.host_role(), HostRole::Standby { .. }));
            match case.as_str() {
                "logout" => settings.clear_key().unwrap(),
                "replace" | "initialize" => {
                    settings.initialize(NEW).await.unwrap();
                }
                "aba" => {
                    settings.clear_key().unwrap();
                    settings.initialize(OLD).await.unwrap();
                }
                _ => panic!("unknown H88 standby case"),
            }
        }
        "reopen" => {
            assert_eq!(app.host_role(), &HostRole::Primary);
            if case == "crash" {
                assert!(secrets.get(API_KEY_NAME).unwrap().as_deref() == Some(NEW));
                assert!(settings.api_key().is_err());
                assert!(settings.view().is_err());
                assert!(settings.refresh_models().await.is_err());
                settings.initialize(NEW).await.unwrap();
            }
            let expected = match case.as_str() {
                "logout" => None,
                "aba" => Some(OLD),
                _ => Some(NEW),
            };
            assert!(settings.api_key().unwrap().as_deref() == expected);
            assert_eq!(
                settings.view().unwrap().unwrap().configured,
                expected.is_some()
            );
            if expected.is_some() {
                settings.refresh_models().await.unwrap();
            }
        }
        _ => panic!("unknown H88 actor"),
    }
}

struct ChildOwner {
    child: Child,
    log: PathBuf,
}

impl ChildOwner {
    fn spawn(root: &Path, actor: &str, case: &str) -> Self {
        let temp = root.join(format!("temp-{actor}"));
        std::fs::create_dir_all(&temp).unwrap();
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
            .env(CASE, case)
            .env("TEMP", &temp)
            .env("TMP", &temp)
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
            assert!(Instant::now() < deadline, "H88 child exit timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_barrier(&mut self, root: &Path) {
        let deadline = Instant::now() + BUDGET;
        while !root.join("waiting").is_file() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(&self.log).unwrap()
            );
            assert!(Instant::now() < deadline, "H88 child barrier timed out");
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

#[test]
fn h88_primary_standby_revoke_late_refresh_and_initialization_across_distinct_temp() {
    for case in ["logout", "replace", "aba", "initialize"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut primary = ChildOwner::spawn(root, "primary", case);
        primary.wait_for_barrier(root);
        ChildOwner::spawn(root, "standby", case).finish();
        std::fs::write(root.join("resume"), b"resume").unwrap();
        primary.finish();
        ChildOwner::spawn(root, "reopen", case).finish();
    }
}

#[test]
fn h88_killed_committer_releases_gate_but_requires_explicit_recovery_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut primary = ChildOwner::spawn(root, "primary", "crash");
    primary.wait_for_barrier(root);
    primary.child.kill().unwrap();
    primary.child.wait().unwrap();
    ChildOwner::spawn(root, "reopen", "crash").finish();
}
