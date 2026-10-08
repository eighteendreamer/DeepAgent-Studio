//! Relocating a legacy data root must not happen while another live host owns
//! it. In-process claims short-circuit (one process may claim the same root
//! twice), so the guard is only real if a second OS process is refused — and the
//! refused host must leave the tree exactly as it found it.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use deepagent_app_core::data_root::UserPaths;
use deepagent_app_core::{host_lock, migrate};

const MODE: &str = "DEEPAGENT_MIGRATE_LOCK_TEST";
const SOURCE: &str = "DEEPAGENT_MIGRATE_LOCK_TEST_SOURCE";
const TARGET: &str = "DEEPAGENT_MIGRATE_LOCK_TEST_TARGET";
const REPORT: &str = "DEEPAGENT_MIGRATE_LOCK_TEST_REPORT";
const TEST_NAME: &str = "a_legacy_root_held_by_a_live_host_is_not_relocated";

fn report_path(root: &Path) -> PathBuf {
    root.join("child-report.txt")
}

fn start_child(source: &Path, target: &Path, report: &Path) -> Child {
    Command::new(std::env::current_exe().expect("test binary"))
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--test-threads=1")
        .env(MODE, "child")
        .env(SOURCE, source)
        .env(TARGET, target)
        .env(REPORT, report)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child migrator")
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn a_legacy_root_held_by_a_live_host_is_not_relocated() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("install").join("data");
    write(
        &source.join("files").join("plugins").join("demo.txt"),
        "demo",
    );
    write(&source.join("files").join("cache").join("one.json"), "{}");
    let target = UserPaths::under(tmp.path().join("home").join(".deepagent"));

    if std::env::var(MODE).is_ok_and(|mode| mode == "child") {
        let report = PathBuf::from(std::env::var(REPORT).unwrap());
        let result = migrate::migrate_into(
            &UserPaths::under(std::env::var(TARGET).unwrap()),
            &[PathBuf::from(std::env::var(SOURCE).unwrap())],
            false,
        );
        let payload = match result {
            Ok(_) => "migrated".to_string(),
            Err(error) => format!("refused: {error}"),
        };
        std::fs::write(&report, payload).expect("child report");
        return;
    }

    // Stand in for the desktop that is still running from the legacy root.
    let claim = host_lock::claim(&source).unwrap();
    assert!(claim.is_primary(), "the parent must own the legacy root");

    let mut child = start_child(&source, &target.root, &report_path(tmp.path()));
    let status = child.wait().expect("wait for child");
    assert!(status.success(), "the child host must exit cleanly");
    let report = std::fs::read_to_string(report_path(tmp.path())).expect("child report");
    assert!(
        report.starts_with("refused: another host owns"),
        "a racing host has to be turned away, got: {report}"
    );
    assert!(
        source
            .join("files")
            .join("plugins")
            .join("demo.txt")
            .is_file(),
        "nothing may move out of a root another host owns"
    );
    assert!(
        source
            .join("files")
            .join("cache")
            .join("one.json")
            .is_file(),
        "the refusal must happen before any entry is touched"
    );
    assert!(
        !target
            .root
            .join("files")
            .join("plugins")
            .join("demo.txt")
            .exists(),
        "and nothing lands in the target either"
    );

    // After the holder lets go, the same relocation succeeds.
    host_lock::release(&source);
    let reports = migrate::migrate_into(&target, std::slice::from_ref(&source), false).unwrap();
    assert_eq!(reports.len(), 1);
    assert!(!reports[0].skipped);
    assert!(target
        .root
        .join("files")
        .join("plugins")
        .join("demo.txt")
        .is_file());
    assert!(
        !source.exists(),
        "a finished move leaves no legacy root behind"
    );
}
