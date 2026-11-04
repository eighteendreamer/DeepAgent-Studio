//! Real host roles and distinct TEMP directories, never OS credentials.
use super::*;
use deepagent_app_core::host_lock::HostRole;
use deepagent_app_core::AppService;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const ENTRY: &str = "process::h290_host_entry";
const ROOT: &str = "DEEPAGENT_H290_ROOT";
const ACTOR: &str = "DEEPAGENT_H290_ACTOR";
const CASE: &str = "DEEPAGENT_H290_CASE";
const BUDGET: Duration = Duration::from_secs(30);

struct Pause {
    inner: SqliteSecretStore,
    root: PathBuf,
    armed: AtomicBool,
    phase: String,
}

impl Pause {
    fn barrier(&self, phase: &str) -> Result<()> {
        if self.armed.load(Ordering::SeqCst) && self.phase == phase {
            std::fs::write(self.root.join("waiting"), b"ready").unwrap();
            let deadline = Instant::now() + BUDGET;
            while !self.root.join("resume").is_file() {
                if Instant::now() >= deadline {
                    return Err(CoreError::other("H290 barrier timed out"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }
}

impl SecretStore for Pause {
    fn set(&self, name: &str, key: &str) -> Result<()> {
        self.inner.set(name, key)?;
        self.barrier("stage")
    }
    fn get(&self, name: &str) -> Result<Option<String>> {
        self.inner.get(name)
    }
    fn delete(&self, name: &str) -> Result<()> {
        self.barrier("published")?;
        self.inner.delete(name)
    }
}

#[test]
#[ignore = "child fixture, explicitly executed by default parent tests"]
fn h290_host_entry() {
    let root = PathBuf::from(std::env::var_os(ROOT).expect("H290 child root"));
    let actor = std::env::var(ACTOR).unwrap();
    let case = std::env::var(CASE).unwrap();
    let app = AppService::open(root.join("state.sqlite")).unwrap();
    let wrapping = Arc::new(MemorySecretStore::new());
    wrapping
        .set(
            "deepagent_secret_wrap_key",
            "H290 process synthetic wrapping",
        )
        .unwrap();
    let store = Arc::new(Pause {
        inner: SqliteSecretStore::new(app.shared_database(), wrapping),
        root: root.clone(),
        armed: AtomicBool::new(false),
        phase: if case == "published" {
            "published"
        } else {
            "stage"
        }
        .into(),
    });
    let providers = CanvasProviderService::new(app.shared_database(), store.clone());
    match actor.as_str() {
        "primary" => {
            assert_eq!(app.host_role(), &HostRole::Primary);
            providers
                .save_provider(input("first", OLD_URL, Some(OLD)))
                .unwrap();
            let version = providers.provider("first").unwrap().credential_version;
            std::fs::write(root.join("old-version"), version).unwrap();
            store.armed.store(true, Ordering::SeqCst);
            providers
                .save_provider(input("first", NEW_URL, Some(NEW)))
                .unwrap();
        }
        "standby" => {
            assert!(matches!(app.host_role(), HostRole::Standby { .. }));
            std::fs::write(root.join("standby-ready"), b"ready").unwrap();
            if case == "interleave" {
                providers
                    .save_model(
                        "first",
                        CanvasModelConfig {
                            id: "other-model".into(),
                            name: "other-model".into(),
                            description: String::new(),
                            enabled: true,
                            scenarios: vec![CanvasScenario::Text],
                            priority: 1,
                            max_reference_images: None,
                        },
                    )
                    .unwrap();
                providers
                    .save_provider(input("second", OLD_URL, Some(OLD)))
                    .unwrap();
            }
            let pair = providers.resolve_model("first", "model").unwrap();
            let old = case == "stage";
            assert_eq!(pair.provider.base_url, if old { OLD_URL } else { NEW_URL });
            assert!(pair.api_key.as_deref() == Some(if old { OLD } else { NEW }));
        }
        "reopen" => {
            assert_eq!(app.host_role(), &HostRole::Primary);
            let (provider, key) = providers.provider_credentials("first").unwrap();
            assert_eq!(
                provider.base_url,
                if case == "stage" { OLD_URL } else { NEW_URL }
            );
            assert!(key.as_deref() == Some(if case == "stage" { OLD } else { NEW }));
            let old_version = std::fs::read_to_string(root.join("old-version")).unwrap();
            assert_eq!(provider.credential_version == old_version, case == "stage");
            if case == "interleave" {
                assert!(provider
                    .models
                    .iter()
                    .any(|model| model.id == "other-model"));
                assert_eq!(providers.list_providers().unwrap().len(), 2);
            } else {
                let count: usize = app
                    .shared_database()
                    .with_conn(|connection| {
                        connection
                            .query_row(
                                "SELECT count(*) FROM secret_records WHERE name LIKE 'canvas:%'",
                                [],
                                |row| row.get(0),
                            )
                            .map_err(|error| CoreError::Persistence(error.to_string()))
                    })
                    .unwrap();
                assert_eq!(
                    count, 2,
                    "killed process can leave an unreachable encrypted record"
                );
            }
            // Explicit new edit is still possible; an orphan is never adopted.
            providers
                .save_provider(input("first", NEW_URL, Some("h290-process-recovered")))
                .unwrap();
            assert!(
                providers.provider_api_key("first").unwrap().as_deref()
                    == Some("h290-process-recovered")
            );
        }
        _ => panic!("unknown H290 actor"),
    }
}

struct Owner {
    child: Child,
    log: PathBuf,
}

impl Owner {
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
    fn wait_file(&mut self, root: &Path, name: &str) {
        let deadline = Instant::now() + BUDGET;
        while !root.join(name).is_file() {
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(&self.log).unwrap()
            );
            assert!(Instant::now() < deadline, "H290 process barrier timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
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
                break;
            }
            assert!(Instant::now() < deadline, "H290 process exit timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn interleave(case: &str) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut primary = Owner::spawn(root, "primary", case);
    primary.wait_file(root, "waiting");
    let mut standby = Owner::spawn(root, "standby", case);
    standby.wait_file(root, "standby-ready");
    if case == "interleave" {
        std::fs::write(root.join("resume"), b"resume").unwrap();
        primary.finish();
    } else {
        primary.child.kill().unwrap();
        primary.child.wait().unwrap();
    }
    standby.finish();
    Owner::spawn(root, "reopen", case).finish();
}

#[test]
fn h290_killed_stage_preserves_old_identity_and_releases_gate_to_standby() {
    interleave("stage");
}

#[test]
fn h290_killed_retirement_keeps_fully_published_new_identity() {
    interleave("published");
}

#[test]
fn h290_standby_model_and_provider_edits_preserve_new_primary_secret_reference() {
    interleave("interleave");
}
