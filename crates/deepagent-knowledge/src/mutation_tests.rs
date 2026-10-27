//! Real temporary-vault regressions for the shared mutation boundary (H-52).

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use crate::{
    EntryKind, HashingEmbedder, KnowledgeBase, KnowledgeConfig, KnowledgeDraft, Scope, Vault,
};

fn base(root: &Path) -> KnowledgeBase<HashingEmbedder> {
    let mut base = KnowledgeBase::new(
        vec![
            Vault::new(root.join("project"), Scope::Project),
            Vault::new(root.join("global"), Scope::Global),
        ],
        HashingEmbedder::default(),
        KnowledgeConfig::default(),
    );
    base.load_all().unwrap();
    base
}

fn note(title: &str, body: &str, scope: Scope) -> KnowledgeDraft {
    KnowledgeDraft {
        title: title.into(),
        body: body.into(),
        kind: EntryKind::Note,
        tags: vec![],
        source_session: None,
        scope,
        tier: None,
        importance: None,
    }
}

fn key(scope: Scope, id: &str) -> String {
    format!("{}:{id}", scope.label())
}

#[test]
fn independent_stale_views_allocate_from_current_latin_cjk_and_truncated_names() {
    let long = "界".repeat(85);
    for scope in [Scope::Project, Scope::Global] {
        for (a, b) in [
            ("retry: policy".into(), "retry policy".into()),
            ("中文：策略".into(), "中文 策略".into()),
            (format!("{long}甲"), format!("{long}乙")),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let mut first = base(tmp.path());
            let mut second = base(tmp.path());
            assert_eq!(Vault::slug(&a), Vault::slug(&b));
            let a = first
                .write(note(&a, "distinct first evidence", scope), 1)
                .unwrap();
            let b = second
                .write(note(&b, "distinct second evidence", scope), 2)
                .unwrap();
            assert_ne!(a.id, b.id);
            let disk = base(tmp.path());
            assert_eq!(disk.len(), 2);
            assert_eq!(disk.get(&key(scope, &a.id)).unwrap().body, a.body);
            assert_eq!(disk.get(&key(scope, &b.id)).unwrap().body, b.body);
            assert_eq!(
                second.list(),
                disk.list(),
                "successful mutation refreshes its scope"
            );
        }
    }
}

#[test]
fn stale_same_note_update_delete_and_blind_retry_conflict_until_reload() {
    for scope in [Scope::Project, Scope::Global] {
        let tmp = tempfile::tempdir().unwrap();
        let mut first = base(tmp.path());
        let original = first.write(note("Policy", "original", scope), 1).unwrap();
        let mut stale = base(tmp.path());
        first.write(note("Policy", "new author", scope), 1).unwrap(); // same timestamp
        for _ in 0..2 {
            assert!(stale
                .write(note("Policy", "stale edit", scope), 1)
                .unwrap_err()
                .to_string()
                .contains("version conflict"));
            assert!(stale
                .delete(&key(scope, &original.id))
                .unwrap_err()
                .to_string()
                .contains("reload"));
        }
        assert_eq!(
            stale.get(&key(scope, &original.id)).unwrap().body,
            "original"
        );
        assert_eq!(
            base(tmp.path())
                .get(&key(scope, &original.id))
                .unwrap()
                .body,
            "new author"
        );
        stale.load_all().unwrap();
        let updated = stale
            .write(note("Policy", "reviewed edit", scope), 3)
            .unwrap();
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.created_at, original.created_at);
        assert!(first.delete(&key(scope, &original.id)).is_err());
        first.load_all().unwrap();
        assert!(first.delete(&key(scope, &original.id)).unwrap());
        assert!(base(tmp.path()).is_empty());
        assert!(stale
            .write(note("Policy", "resurrect stale", scope), 4)
            .is_err());
    }
}

#[test]
fn an_unobserved_same_title_is_a_conflict_not_an_implicit_update() {
    let tmp = tempfile::tempdir().unwrap();
    let mut first = base(tmp.path());
    let mut second = base(tmp.path());
    first
        .write(note("same title", "first", Scope::Global), 1)
        .unwrap();
    assert!(second
        .write(note(" SAME  TITLE ", "second", Scope::Global), 2)
        .is_err());
    assert_eq!(base(tmp.path()).list()[0].body, "first");
    second.load_all().unwrap();
    second
        .write(note("same title", "explicitly reviewed", Scope::Global), 2)
        .unwrap();
    assert_eq!(base(tmp.path()).len(), 1);
}

#[test]
fn exact_bytes_detect_external_metadata_edits_and_unreadable_names_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut host = base(tmp.path());
    let entry = host
        .write(note("External", "body", Scope::Project), 1)
        .unwrap();
    let path = tmp.path().join("project").join(format!("{}.md", entry.id));
    let raw = std::fs::read_to_string(&path).unwrap();
    let edited = raw.replacen("---\n", "---\nexternal_owner: human\n", 1);
    std::fs::write(&path, &edited).unwrap();
    assert!(host
        .write(note("External", "stale", Scope::Project), 1)
        .is_err());
    assert!(host.delete(&key(Scope::Project, &entry.id)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
    let broken = tmp.path().join("project/broken.md");
    std::fs::write(&broken, [0xff, 0xfe]).unwrap();
    assert!(host
        .write(note("Broken", "must not replace bytes", Scope::Project), 2)
        .is_err());
    assert_eq!(std::fs::read(&broken).unwrap(), vec![0xff, 0xfe]);
    std::fs::remove_file(&broken).unwrap();
    host.load_all().unwrap();
    host.write(note("External", "after reload", Scope::Project), 2)
        .unwrap();
}

#[test]
fn external_filename_case_and_markdown_extension_never_alias_a_new_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let mut host = base(tmp.path());
    let entry = host
        .write(
            note("External title", "external original", Scope::Project),
            1,
        )
        .unwrap();
    let dir = tmp.path().join("project");
    std::fs::rename(dir.join(format!("{}.md", entry.id)), dir.join("Note.MD")).unwrap();
    host.load_all().unwrap();
    let created = host
        .write(note("Note", "separate note", Scope::Project), 2)
        .unwrap();
    assert_eq!(host.len(), 2);
    assert_eq!(
        std::fs::read_to_string(dir.join("Note.MD")).unwrap(),
        entry.to_markdown()
    );
    host.write(
        note("External title", "reviewed external edit", Scope::Project),
        3,
    )
    .unwrap();
    let disk = base(tmp.path());
    assert_eq!(disk.len(), 2);
    assert_eq!(
        disk.get("project:Note").unwrap().body,
        "reviewed external edit"
    );
    assert_eq!(
        disk.get(&key(Scope::Project, &created.id)).unwrap().body,
        "separate note"
    );
}

#[test]
fn stale_drafts_allocate_uniquely_and_promotion_checks_both_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = base(tmp.path());
    let mut b = base(tmp.path());
    let first = a
        .add_draft(note("draft: collision", "draft a", Scope::Global), 1)
        .unwrap();
    let second = b
        .add_draft(note("draft collision", "draft b", Scope::Global), 2)
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(b.draft_count(), 2);
    let active = a
        .write(note("draft: collision", "active author", Scope::Global), 3)
        .unwrap();
    assert_ne!(
        active.id, first.id,
        "active allocation reserves pending draft identities"
    );
    assert!(b.accept_draft(&key(Scope::Global, &first.id), 4).is_err());
    let disk = base(tmp.path());
    assert_eq!(
        disk.draft_count(),
        2,
        "promotion conflict must not consume the draft"
    );
    assert_eq!(disk.list()[0].body, "active author");
    b.load_all().unwrap();
    let accepted = b.accept_draft(&key(Scope::Global, &first.id), 4).unwrap();
    assert_eq!(accepted.id, active.id);
    assert_eq!(b.draft_count(), 1);
    let path = tmp
        .path()
        .join("global/.drafts")
        .join(format!("{}.md", second.id));
    std::fs::write(
        &path,
        std::fs::read_to_string(&path)
            .unwrap()
            .replace("draft b", "human draft"),
    )
    .unwrap();
    assert!(b.accept_draft(&key(Scope::Global, &second.id), 5).is_err());
    assert!(b.discard_draft(&key(Scope::Global, &second.id)).is_err());
    b.load_all().unwrap();
    assert!(b.discard_draft(&key(Scope::Global, &second.id)).unwrap());
    assert_eq!(base(tmp.path()).draft_count(), 0);
}

#[test]
fn failed_write_preserves_existing_disk_and_can_retry_without_a_new_slug() {
    let tmp = tempfile::tempdir().unwrap();
    let mut host = base(tmp.path());
    let occupied = tmp.path().join("project/retry.md");
    std::fs::create_dir_all(&occupied).unwrap();
    assert!(host
        .write(note("Retry", "retry evidence", Scope::Project), 1)
        .is_err());
    assert!(host.is_empty());
    assert!(occupied.is_dir());
    std::fs::remove_dir(&occupied).unwrap();
    let saved = host
        .write(note("Retry", "retry evidence", Scope::Project), 2)
        .unwrap();
    assert_eq!(saved.id, "retry");
    assert_eq!(host.list(), base(tmp.path()).list());
}

#[test]
fn stale_consolidation_recomputes_candidates_before_any_archive() {
    let tmp = tempfile::tempdir().unwrap();
    let mut writer = base(tmp.path());
    let body = "This deployment decision is long enough to be a duplicate.";
    let a = writer
        .write(note("Decision A", body, Scope::Global), 1)
        .unwrap();
    let b = writer
        .write(note("Decision B", body, Scope::Global), 2)
        .unwrap();
    let mut stale = base(tmp.path());
    writer
        .write(
            note(
                "Decision A",
                "A different current decision with no shared content",
                Scope::Global,
            ),
            3,
        )
        .unwrap();
    assert!(stale.consolidate(None, 4).unwrap().is_empty());
    let disk = base(tmp.path());
    assert_eq!(stale.list(), disk.list());
    assert!(disk
        .get(&key(Scope::Global, &a.id))
        .unwrap()
        .body
        .starts_with("A different"));
    assert_eq!(disk.get(&key(Scope::Global, &b.id)).unwrap().body, body);
    assert!(!tmp.path().join("global/.superseded").exists());
}

#[test]
fn partial_consolidation_reconciles_cache_and_never_replaces_archive_history() {
    let tmp = tempfile::tempdir().unwrap();
    let mut host = base(tmp.path());
    let body = "A shared deployment decision with enough meaningful characters.";
    let a = host.write(note("A", body, Scope::Global), 1).unwrap();
    let b = host.write(note("B", body, Scope::Global), 2).unwrap();
    host.write(
        note(
            "C",
            &format!("{body}\nCurrent survivor detail."),
            Scope::Global,
        ),
        3,
    )
    .unwrap();
    let archive = tmp.path().join("global/.superseded");
    std::fs::create_dir_all(&archive).unwrap();
    let blocked = archive.join(format!("{}.md", b.id));
    std::fs::write(&blocked, "previous archived generation").unwrap();
    assert!(host.consolidate(None, 4).is_err());
    assert!(host.get(&key(Scope::Global, &a.id)).is_none());
    assert!(host.get(&key(Scope::Global, &b.id)).is_some());
    assert_eq!(
        host.list(),
        base(tmp.path()).list(),
        "partial commits must be visible"
    );
    assert_eq!(
        std::fs::read_to_string(&blocked).unwrap(),
        "previous archived generation"
    );
    std::fs::rename(&blocked, archive.join("previous-b.md")).unwrap();
    assert_eq!(host.consolidate(None, 5).unwrap().superseded.len(), 1);
    assert_eq!(host.len(), 1);
    assert_eq!(
        std::fs::read_to_string(archive.join("previous-b.md")).unwrap(),
        "previous archived generation"
    );
}

#[cfg(windows)]
#[test]
fn failed_promotion_cleanup_keeps_draft_and_published_active_retriable() {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ from the existing windows-sys FileSystem definitions.
    const SHARE_READ: u32 = 1;
    let tmp = tempfile::tempdir().unwrap();
    let mut host = base(tmp.path());
    let draft = host
        .add_draft(note("Pending", "source evidence", Scope::Global), 1)
        .unwrap();
    let path = tmp
        .path()
        .join("global/.drafts")
        .join(format!("{}.md", draft.id));
    // Permit snapshot reads, but omit FILE_SHARE_DELETE to force the cleanup
    // failure at the real OS, after the active entry has been published.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ)
        .open(&path)
        .unwrap();
    let result = host.accept_draft(&key(Scope::Global, &draft.id), 2);
    drop(held);
    assert!(result.is_err());
    assert_eq!(host.len(), 1);
    assert_eq!(host.draft_count(), 1);
    assert_eq!(host.list(), base(tmp.path()).list());
    host.accept_draft(&key(Scope::Global, &draft.id), 3)
        .unwrap();
    assert_eq!(host.len(), 1);
    assert_eq!(host.draft_count(), 0);
}

#[cfg(windows)]
#[test]
fn real_rename_failure_preserves_version_and_source_draft_for_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    const SHARE_READ: u32 = 1; // windows-sys FILE_SHARE_READ, no FILE_SHARE_DELETE.
    let tmp = tempfile::tempdir().unwrap();
    let mut host = base(tmp.path());
    let entry = host
        .write(note("Retry", "original", Scope::Global), 1)
        .unwrap();
    let path = tmp.path().join("global").join(format!("{}.md", entry.id));
    let source = host
        .add_draft(note("Retry", "draft evidence", Scope::Global), 2)
        .unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(SHARE_READ)
        .open(&path)
        .unwrap();
    assert!(host
        .write(note("Retry", "failed replacement", Scope::Global), 3)
        .is_err());
    assert!(host
        .accept_draft(&key(Scope::Global, &source.id), 4)
        .is_err());
    assert_eq!(
        host.get(&key(Scope::Global, &entry.id)).unwrap().body,
        "original"
    );
    assert_eq!(host.draft_count(), 1);
    assert_eq!(host.list(), base(tmp.path()).list());
    drop(held);
    let promoted = host
        .accept_draft(&key(Scope::Global, &source.id), 5)
        .unwrap();
    assert_eq!(promoted.id, entry.id);
    assert_eq!(promoted.body, "draft evidence");
    assert_eq!(host.draft_count(), 0);
    assert!(std::fs::read_dir(tmp.path().join("global"))
        .unwrap()
        .all(|item| !item
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")));
}

fn wait_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// The parent owns and kills children even if an assertion or timeout panics.
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn vault_process_worker() {
    let Some(root) = std::env::var_os("DEEPAGENT_H52_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let role = std::env::var("DEEPAGENT_H52_TEST_ROLE").unwrap();
    let mut host = base(&root);
    std::fs::write(root.join(format!("ready-{role}")), b"ready").unwrap();
    wait_file(&root.join("go"));
    if std::env::var("DEEPAGENT_H52_TEST_MODE").as_deref() == Ok("edit") {
        if role == "a" {
            host.write(note("Shared", "real process a update", Scope::Global), 1)
                .unwrap();
            std::fs::write(root.join("updated-a"), b"updated").unwrap();
        } else {
            wait_file(&root.join("updated-a"));
            let error = host
                .write(note("Shared", "stale process b update", Scope::Global), 1)
                .unwrap_err();
            assert!(error.to_string().contains("version conflict"));
            std::fs::write(root.join("conflict-b"), b"conflict").unwrap();
        }
        return;
    }
    let title = if role == "a" {
        "Process: policy"
    } else {
        "Process policy"
    };
    host.write(
        note(title, &format!("real process {role}"), Scope::Global),
        1,
    )
    .unwrap();
}

#[test]
fn two_real_processes_load_before_writing_and_preserve_both_notes() {
    let tmp = tempfile::tempdir().unwrap();
    run_workers(tmp.path(), "create");
    let entries = base(tmp.path()).list();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| e.body == "real process a"));
    assert!(entries.iter().any(|e| e.body == "real process b"));
    assert_ne!(entries[0].id, entries[1].id);
}

#[test]
fn two_real_processes_reject_a_stale_same_note_update() {
    let tmp = tempfile::tempdir().unwrap();
    base(tmp.path())
        .write(note("Shared", "original", Scope::Global), 1)
        .unwrap();
    run_workers(tmp.path(), "edit");
    assert!(tmp.path().join("conflict-b").is_file());
    let entries = base(tmp.path()).list();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].body, "real process a update");
}

fn run_workers(root: &Path, mode: &str) {
    let spawn = |role: &str| {
        Worker(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "mutation_tests::vault_process_worker",
                    "--nocapture",
                ])
                .env("DEEPAGENT_H52_TEST_ROOT", root)
                .env("DEEPAGENT_H52_TEST_ROLE", role)
                .env("DEEPAGENT_H52_TEST_MODE", mode)
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let mut a = spawn("a");
    let mut b = spawn("b");
    wait_file(&root.join("ready-a"));
    wait_file(&root.join("ready-b"));
    std::fs::write(root.join("go"), b"go").unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut statuses = [None, None];
    while statuses.iter().any(Option::is_none) {
        for (worker, status) in [&mut a, &mut b].into_iter().zip(statuses.iter_mut()) {
            if status.is_none() {
                *status = worker.0.try_wait().unwrap();
            }
        }
        assert!(Instant::now() < deadline, "worker did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(statuses.iter().all(|status| status.unwrap().success()));
}
