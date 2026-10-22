//! Opt-in lifecycle probes using the discovered plan and real Cargo/build.rs.

use super::*;
use deepagent_core::clock::FixedClock;
use deepagent_hooks::HookRegistry;
use deepagent_models::transport::MockTransport;
use deepagent_models::{ModelClient, ModelConfig};
use deepagent_persistence::{run_store::RunStore, Database};
use deepagent_runtime::{
    AgentKernel, CheckpointManager, ModelAgent, RunRequest, RuntimeConfig, TerminalKind,
};
use deepagent_session::Session;
use deepagent_tools::ToolRegistry;
use deepagent_tracing::metrics::Metrics;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
enum Interruption {
    None,
    Cancel,
    Deadline,
}

// Even an assertion/timeout must release the real build process. The build
// script also has its own hard limit so a broken probe cannot hang forever.
struct ReleaseBuild(std::path::PathBuf);

impl Drop for ReleaseBuild {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

async fn wait_for_file(path: &Path, limit: Duration) {
    tokio::time::timeout(limit, async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fixture process did not create {}", path.display()));
}

async fn audit_real_build(interruption: Interruption) -> (bool, bool) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'harness-verification-probe'\nversion = '0.0.0'\nedition = '2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").unwrap();
    std::fs::write(
        root.join("build.rs"),
        r#"use std::{fs, path::Path, thread, time::{Duration, Instant}};
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    fs::write("build-started", std::process::id().to_string()).unwrap();
    let began = Instant::now();
    while !Path::new("release-build").exists() {
        assert!(began.elapsed() < Duration::from_secs(15), "fixture safety deadline");
        thread::sleep(Duration::from_millis(10));
    }
    fs::write("build-wrote-after-release", "real build side effect").unwrap();
}
"#,
    )
    .unwrap();
    let release_path = root.join("release-build");
    let _release_on_unwind = ReleaseBuild(release_path.clone());
    let started_path = root.join("build-started");
    let late_path = root.join("build-wrote-after-release");

    // Use the actual structural discovery and SystemCommandRunner. The only
    // command adjustment isolates Cargo's output/lock from this test binary.
    let mut plan = discover_verification_plan(&root).unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].command.program, "cargo");
    plan.steps[0].command.args.extend([
        "--target-dir".to_string(),
        temp.path()
            .join("cargo-target")
            .to_string_lossy()
            .into_owned(),
    ]);

    let db = Arc::new(Database::open_in_memory().unwrap());
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, Some("real build audit")).unwrap();
    let task = session.create_task("create answer.txt").unwrap();
    let run_id = "audit-real-verification";
    RunStore::new(&db)
        .create(
            run_id,
            &session.id().to_string(),
            Some(&task.to_string()),
            1,
        )
        .unwrap();
    let checkpoint = Arc::new(
        CheckpointManager::new(
            db.clone(),
            run_id,
            0,
            &root,
            temp.path().join("checkpoints"),
        )
        .unwrap(),
    );
    let answer = root.join("answer.txt");
    checkpoint.capture_before(&answer).unwrap();
    std::fs::write(&answer, "fixture answer").unwrap();
    assert_eq!(
        checkpoint.mutation_evidence().unwrap()[0].kind,
        deepagent_runtime::checkpoint::MutationKind::Created,
    );
    let registry = ToolRegistry::new();
    // AppCore always attaches a hook registry. Do not omit it and infer
    // production completion behavior from a different kernel configuration.
    let hooks = HookRegistry::new();
    let config = RuntimeConfig {
        checkpoint: Some(checkpoint),
        task_timeout: matches!(interruption, Interruption::Deadline)
            .then_some(Duration::from_secs(5)),
        ..RuntimeConfig::default()
    };
    let kernel =
        AgentKernel::<FixedClock>::new(db.clone(), &registry, Metrics::new(), config, run_id)
            .with_hooks(&hooks)
            .with_verification(&plan);
    let handle = kernel.handle();
    let mut agent = ModelAgent::new(
        Arc::new(ModelClient::new(
            Arc::new(MockTransport::new([
                r#"{"type":"response.output_text.delta","delta":"Created answer.txt."}"#
                    .to_string(),
                r#"{"type":"response.completed","response":{"status":"completed"}}"#.to_string(),
            ])),
            ModelConfig::deepseek("offline-placeholder"),
        )),
        "deepseek-flash",
        "system",
        "create answer.txt",
        vec![],
    );
    let exited = AtomicBool::new(false);
    let run = async {
        let terminal = kernel
            .start(RunRequest::new(&mut session, task, &mut agent))
            .await
            .unwrap();
        exited.store(true, Ordering::SeqCst);
        terminal
    };
    let observe = async {
        wait_for_file(&started_path, Duration::from_secs(4)).await;
        assert!(!late_path.exists());
        let began = Instant::now();
        match interruption {
            Interruption::Cancel => {
                assert!(handle.cancel());
                assert!(handle.is_cancelled());
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Interruption::Deadline => {
                tokio::time::timeout(Duration::from_secs(6), async {
                    while !exited.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .expect("the kernel deadline must settle before releasing the build");
            }
            Interruption::None => tokio::time::sleep(Duration::from_millis(200)).await,
        }
        let exited_before_release = exited.load(Ordering::SeqCst);
        let before = RunStore::new(&db).get(run_id).unwrap().unwrap();
        let terminal_before_release = before.terminal_kind.is_some();
        eprintln!(
            "real-build {interruption:?}: observed_after_ms={} exited_before_release={exited_before_release} \
             persisted_before_release={:?} late_file_before_release={}",
            began.elapsed().as_millis(),
            before.terminal_kind,
            late_path.exists(),
        );
        // Release BEFORE asserting the defect; wait for the real subprocess's
        // side effect, then let the normal Kernel/runner unwind.
        std::fs::write(&release_path, "release").unwrap();
        wait_for_file(&late_path, Duration::from_secs(3)).await;
        (exited_before_release, terminal_before_release)
    };
    let (terminal, (exited_before_release, terminal_before_release)) =
        tokio::time::timeout(Duration::from_secs(12), async {
            tokio::join!(run, observe)
        })
        .await
        .expect("fixture kernel and observer must settle");
    // On deadline the original spawn_blocking may still be returning from
    // cmd.output(). A second real Cargo check waits for its target-directory
    // lock, ensuring cleanup before TempDir removal. build.rs is unchanged.
    let cleanup = tokio::time::timeout(
        Duration::from_secs(5),
        plan.runner.run(&plan.steps[0].command),
    )
    .await
    .expect("cleanup Cargo check must finish")
    .unwrap();
    assert!(cleanup.success(), "{}", cleanup.combined());
    let record = RunStore::new(&db).get(run_id).unwrap().unwrap();
    let terminal_count = RunStore::new(&db)
        .events_after(run_id, None)
        .unwrap()
        .iter()
        .filter(|event| event.event_type == "run_terminal")
        .count();
    eprintln!(
        "real-build {interruption:?}: terminal={:?} persisted={:?} task={:?} \
         write_after_terminal={} terminal_count={terminal_count} cleanup_success={}",
        terminal.kind,
        record.terminal_kind,
        session.state().task(task).unwrap().state,
        terminal_before_release && late_path.exists(),
        cleanup.success(),
    );
    assert_eq!(terminal_count, 1);
    if matches!(interruption, Interruption::None) {
        assert!(!exited_before_release);
        assert_eq!(terminal.kind, TerminalKind::Succeeded);
    }
    if matches!(interruption, Interruption::Deadline) {
        assert_eq!(terminal.kind, TerminalKind::BudgetExceeded);
    }
    (
        exited_before_release,
        terminal_before_release && late_path.exists(),
    )
}

#[tokio::test]
async fn discovered_real_build_completes_after_release() {
    audit_real_build(Interruption::None).await;
}

#[tokio::test]
#[ignore = "audit probe: run cancellation must stop the discovered real build"]
async fn audit_cancel_stops_discovered_real_build() {
    audit_real_build(Interruption::None).await;
    let (exited, _) = audit_real_build(Interruption::Cancel).await;
    assert!(
        exited,
        "user cancellation waited for the real build to be released"
    );
}

#[tokio::test]
#[ignore = "audit probe: run deadline must own the discovered build process"]
async fn audit_deadline_prevents_real_build_side_effect_after_terminal() {
    audit_real_build(Interruption::None).await;
    let (_, wrote_after_terminal) = audit_real_build(Interruption::Deadline).await;
    assert!(
        !wrote_after_terminal,
        "real build.rs wrote a file after Kernel persisted budget_exceeded",
    );
}
