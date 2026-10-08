//! The install ledger is written by one process and read by another (the
//! uninstaller), while installs can be running in the desktop shell and a
//! terminal host at the same time. A lost update here means an asset that no
//! uninstall will ever find, so it is exercised with two real processes.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use deepagent_app_core::data_root::UserPaths;
use deepagent_app_core::install_ledger::{
    InstallKind, InstallSource, InstallSourceKind, InstallVia, LedgerEntry, LedgerStore,
};

const MODE: &str = "DEEPAGENT_LEDGER_TEST";
const ROOT: &str = "DEEPAGENT_LEDGER_TEST_ROOT";
const TEST_NAME: &str = "concurrent_processes_do_not_lose_ledger_entries";
const ENTRIES_PER_PROCESS: usize = 25;

fn report_path(root: &Path) -> PathBuf {
    root.join("child-report.txt")
}

fn start_child(root: &Path) -> Child {
    let _ = std::fs::remove_file(report_path(root));
    Command::new(std::env::current_exe().expect("test binary"))
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--test-threads=1")
        .arg("--nocapture")
        .env(MODE, "child")
        .env(ROOT, root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child ledger writer")
}

fn entry(prefix: &str, index: usize) -> LedgerEntry {
    LedgerEntry {
        id: format!("{prefix}-{index}"),
        kind: InstallKind::Plugin,
        version: Some("1.0.0".to_string()),
        source: InstallSource {
            kind: InstallSourceKind::Dir,
            spec: format!("/downloads/{prefix}-{index}"),
        },
        path: format!("files/plugins/{prefix}-{index}"),
        sidecars: Vec::new(),
        sha256: None,
        via: InstallVia::Cli,
        installed_at: String::new(),
        last_updated: None,
    }
}

fn write_all(store: &LedgerStore, prefix: &str) {
    for index in 0..ENTRIES_PER_PROCESS {
        store.record(entry(prefix, index)).expect("record entry");
    }
}

#[test]
fn concurrent_processes_do_not_lose_ledger_entries() {
    if std::env::var(MODE).is_ok_and(|mode| mode == "child") {
        let root = PathBuf::from(std::env::var(ROOT).expect("child receives the ledger root"));
        let paths = UserPaths::under(root.clone());
        paths.ensure_dirs().expect("create layout");
        let store = LedgerStore::open(&paths);
        write_all(&store, "child");
        std::fs::write(report_path(&root), "child-done").expect("report");
        return;
    }

    let tmp = tempfile::tempdir().expect("temp dir");
    let root = tmp.path().to_path_buf();
    let paths = UserPaths::under(root.clone());
    paths.ensure_dirs().expect("create layout");
    let store = LedgerStore::open(&paths);

    let mut child = start_child(&root);
    write_all(&store, "parent");
    let status = child.wait().expect("wait for child");
    assert!(status.success(), "child exited with {status:?}");
    let report = std::fs::read_to_string(report_path(&root)).expect("child report");
    assert_eq!(report.trim(), "child-done");

    let entries = store.entries().expect("read the merged ledger");
    let expected = ENTRIES_PER_PROCESS * 2;
    assert_eq!(
        entries.len(),
        expected,
        "both writers' entries must survive; the lock serializes read-modify-write"
    );
    for prefix in ["parent", "child"] {
        for index in 0..ENTRIES_PER_PROCESS {
            let id = format!("{prefix}-{index}");
            assert!(entries.iter().any(|entry| entry.id == id), "missing {id}");
        }
    }
    assert!(
        root.join("install-ledger.json").is_file(),
        "the ledger lives at the data root"
    );
}
