//! A Markdown vault — the source of truth for knowledge entries.
//!
//! Each entry is one `*.md` file under the vault root. The vault is responsible
//! for disk I/O only: scanning, writing, deleting, and generating safe file
//! names. All paths are confined to the root (no `..` traversal, no escaping
//! absolute paths) using the same lexical checks as the file-tool
//! `WorkspaceRoot` guard.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use deepagent_core::error::{CoreError, Result};
use fs2::FileExt;

use crate::entry::{KnowledgeEntry, Scope};

/// How long a writer waits for another process to release the vault before it
/// gives up. Kept short because saves run on the request path: a stalled UI is
/// worse than a retriable "locked" error.
const LOCK_WAIT: Duration = Duration::from_millis(500);

/// Where superseded entries go instead of being deleted. A dot-directory, so
/// [`Vault::scan`] never folds them back into the retrieval index and a human
/// can move a file straight back to recover it.
const SUPERSEDED_DIR: &str = ".superseded";

/// The OS-held exclusive lock over one vault directory.
///
/// `KnowledgeService`'s in-process mutex serialises a single process, but the
/// desktop app and the headless CLI can point at the same data root, and a
/// multi-file transaction (consolidation) needs one barrier over the whole
/// vault. Windows byte-range locks and POSIX `flock` are both released by the
/// OS when the holder exits, which is why there is no PID file to reclaim.
#[derive(Debug)]
pub struct VaultLock {
    file: std::fs::File,
}

impl Drop for VaultLock {
    fn drop(&mut self) {
        if let Err(error) = FileExt::unlock(&self.file) {
            tracing::warn!(%error, "knowledge vault: unlock failed");
        }
    }
}

impl VaultLock {
    /// Overwrite one entry while holding the vault exclusively.
    pub fn write(&self, vault: &Vault, entry: &KnowledgeEntry) -> Result<PathBuf> {
        vault.write_inner(entry)
    }

    /// Remove one entry while holding the vault exclusively.
    pub fn delete(&self, vault: &Vault, id: &str) -> Result<bool> {
        vault.delete_inner(id)
    }

    /// Move one entry into the superseded archive while holding the vault
    /// exclusively.
    pub fn archive(&self, vault: &Vault, id: &str) -> Result<bool> {
        vault.archive_inner(id)
    }
}

/// Whether a failed `try_lock_exclusive` means "someone else is inside the
/// vault" rather than "the lock cannot be taken at all".
///
/// POSIX reports contention as `EAGAIN`/`EACCES`, which std already classifies.
/// Windows reports a contended immediate lock as `ERROR_LOCK_FAILED` (33) with
/// an `Uncategorized` kind, so its raw code has to be checked — measured here
/// rather than assumed.
fn contention(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::PermissionDenied
    ) {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(33) | Some(36))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// A single markdown vault directory.
#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
    scope: Scope,
}

impl Vault {
    /// Open a vault at `root` with the given scope. The directory need not yet
    /// exist; [`Vault::write`] creates it on demand.
    pub fn new(root: impl Into<PathBuf>, scope: Scope) -> Self {
        Self {
            root: root.into(),
            scope,
        }
    }

    /// The vault scope.
    pub fn scope(&self) -> Scope {
        self.scope
    }

    /// The vault root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Scan the vault for `*.md` files and parse each into a [`KnowledgeEntry`].
    /// Bad/unreadable files are skipped with a `tracing::warn` so a single bad
    /// file never blocks the whole load. Returns entries in id order.
    pub fn scan(&self) -> Result<Vec<KnowledgeEntry>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let read_dir = std::fs::read_dir(&self.root)
            .map_err(|e| CoreError::Persistence(format!("read knowledge vault: {e}")))?;

        let mut entries = Vec::new();
        for dirent in read_dir {
            let dirent = match dirent {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("skipping unreadable vault entry: {e}");
                    continue;
                }
            };
            let path = dirent.path();
            if !is_markdown(&path) {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) if !stem.is_empty() => stem.to_string(),
                _ => {
                    tracing::warn!("skipping vault file with no usable name: {path:?}");
                    continue;
                }
            };
            match std::fs::read_to_string(&path) {
                Ok(raw) => entries.push(KnowledgeEntry::from_markdown(&id, self.scope, &raw)),
                Err(e) => tracing::warn!("skipping unreadable knowledge file {path:?}: {e}"),
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entries)
    }

    /// Write (create or overwrite) the `.md` file for `entry`. The resolved
    /// path is confined to the vault root. Returns the file path written.
    ///
    /// The bytes land through a sibling temp file plus a rename, so neither a
    /// concurrent reader nor a crash can leave a half-written entry, and the
    /// result is read back before the call reports success. A transaction that
    /// touches several files takes one [`VaultLock`] and calls
    /// [`VaultLock::write`] instead, so it never waits on its own lock.
    pub fn write(&self, entry: &KnowledgeEntry) -> Result<PathBuf> {
        self.lock()?.write(self, entry)
    }

    /// Delete the `.md` file for `id`. Returns whether a file was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        self.lock()?.delete(self, id)
    }

    /// Move one entry into `<root>/.superseded/` instead of removing it, so a
    /// consolidation decision stays reversible. Returns whether a file moved.
    pub fn archive(&self, id: &str) -> Result<bool> {
        self.lock()?.archive(self, id)
    }

    /// The archive body, reachable only while a [`VaultLock`] is held.
    fn archive_inner(&self, id: &str) -> Result<bool> {
        let source = self.resolve(id)?;
        if !source.is_file() {
            return Ok(false);
        }
        let dir = self.root.join(SUPERSEDED_DIR);
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Persistence(format!("create superseded dir: {e}")))?;
        let name = source
            .file_name()
            .ok_or_else(|| CoreError::invalid("knowledge id has no file name"))?;
        std::fs::rename(&source, dir.join(name))
            .map_err(|e| CoreError::Persistence(format!("archive knowledge file: {e}")))?;
        Ok(true)
    }

    /// The write body, reachable only while a [`VaultLock`] is held.
    fn write_inner(&self, entry: &KnowledgeEntry) -> Result<PathBuf> {
        let path = self.resolve(&entry.id)?;
        let markdown = entry.to_markdown();
        std::fs::create_dir_all(&self.root)
            .map_err(|e| CoreError::Persistence(format!("create knowledge vault dir: {e}")))?;
        let temp = staging_path(&path);
        std::fs::write(&temp, &markdown)
            .map_err(|e| CoreError::Persistence(format!("stage knowledge file: {e}")))?;
        if let Err(error) = std::fs::rename(&temp, &path) {
            let _ = std::fs::remove_file(&temp);
            return Err(CoreError::Persistence(format!(
                "commit knowledge file: {error}"
            )));
        }
        let read_back = std::fs::read_to_string(&path)
            .map_err(|e| CoreError::Persistence(format!("read back knowledge file: {e}")))?;
        if read_back != markdown {
            return Err(CoreError::Persistence(format!(
                "knowledge file did not read back as written: {}",
                path.display()
            )));
        }
        Ok(path)
    }

    /// The delete body, reachable only while a [`VaultLock`] is held.
    fn delete_inner(&self, id: &str) -> Result<bool> {
        let path = self.resolve(id)?;
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| CoreError::Persistence(format!("delete knowledge file: {e}")))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Take the vault's exclusive lock, waiting at most [`LOCK_WAIT`] for
    /// whoever holds it. The returned guard releases the lock when dropped, and
    /// a multi-file transaction is expected to hold one guard for its whole
    /// body — never across a model call.
    pub fn lock(&self) -> Result<VaultLock> {
        let lock_path = self.root.join(".locks").join("vault.lock");
        if let Some(dir) = lock_path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CoreError::Persistence(format!("create vault lock dir: {e}")))?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| CoreError::Persistence(format!("open vault lock file: {e}")))?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(VaultLock { file }),
                Err(error) if contention(&error) => {
                    if Instant::now() >= deadline {
                        return Err(CoreError::Persistence(format!(
                            "knowledge vault is held by another writer: {}",
                            lock_path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(CoreError::Persistence(format!(
                        "lock knowledge vault: {error}"
                    )));
                }
            }
        }
    }

    /// The drafts subdirectory (`<root>/.drafts`), physically isolated from
    /// active entries so drafts never enter the retrieval index.
    fn drafts_dir(&self) -> PathBuf {
        self.root.join(".drafts")
    }

    /// Scan the drafts subdirectory for `*.md` files (same tolerant rules as
    /// [`Vault::scan`]). Entries are returned with [`crate::entry::EntryStatus`]
    /// taken from their frontmatter.
    pub fn scan_drafts(&self) -> Result<Vec<KnowledgeEntry>> {
        let dir = self.drafts_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let read_dir = std::fs::read_dir(&dir)
            .map_err(|e| CoreError::Persistence(format!("read knowledge drafts: {e}")))?;

        let mut entries = Vec::new();
        for dirent in read_dir {
            let dirent = match dirent {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("skipping unreadable draft entry: {e}");
                    continue;
                }
            };
            let path = dirent.path();
            if !is_markdown(&path) {
                continue;
            }
            let id = match path.file_stem().and_then(|s| s.to_str()) {
                Some(stem) if !stem.is_empty() => stem.to_string(),
                _ => continue,
            };
            match std::fs::read_to_string(&path) {
                Ok(raw) => entries.push(KnowledgeEntry::from_markdown(&id, self.scope, &raw)),
                Err(e) => tracing::warn!("skipping unreadable draft file {path:?}: {e}"),
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(entries)
    }

    /// Write a draft `.md` into the drafts subdirectory. Path confined to the
    /// drafts dir. Returns the file path written.
    pub fn write_draft(&self, entry: &KnowledgeEntry) -> Result<PathBuf> {
        let path = self.resolve_draft(&entry.id)?;
        let dir = self.drafts_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Persistence(format!("create knowledge drafts dir: {e}")))?;
        std::fs::write(&path, entry.to_markdown())
            .map_err(|e| CoreError::Persistence(format!("write knowledge draft: {e}")))?;
        Ok(path)
    }

    /// Delete a draft `.md` by id. Returns whether a file was removed.
    pub fn delete_draft(&self, id: &str) -> Result<bool> {
        let path = self.resolve_draft(id)?;
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| CoreError::Persistence(format!("delete knowledge draft: {e}")))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Resolve a draft id to its confined `<root>/.drafts/<id>.md` path.
    fn resolve_draft(&self, id: &str) -> Result<PathBuf> {
        self.validate_id(id)?;
        let path = self.drafts_dir().join(format!("{id}.md"));
        if !path.starts_with(self.drafts_dir()) {
            return Err(CoreError::invalid(format!(
                "knowledge draft id escapes the vault: {id}"
            )));
        }
        Ok(path)
    }

    /// Resolve an entry id to its confined `<root>/<id>.md` path, rejecting any
    /// id that would escape the vault root.
    fn resolve(&self, id: &str) -> Result<PathBuf> {
        self.validate_id(id)?;
        let path = self.root.join(format!("{id}.md"));
        if !path.starts_with(&self.root) {
            return Err(CoreError::invalid(format!(
                "knowledge id escapes the vault: {id}"
            )));
        }
        Ok(path)
    }

    /// Validate that an id is a flat, traversal-free slug (shared by active and
    /// draft path resolution).
    fn validate_id(&self, id: &str) -> Result<()> {
        if id.trim().is_empty() {
            return Err(CoreError::invalid("knowledge entry id is empty"));
        }
        let candidate = Path::new(id);
        // No traversal, no nested separators, no absolute paths — ids are flat
        // slugs that map to a single file directly under the (drafts) root.
        if candidate
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(CoreError::invalid(format!(
                "knowledge id must be a flat slug: {id}"
            )));
        }
        if id.contains('/') || id.contains('\\') {
            return Err(CoreError::invalid(format!(
                "knowledge id must not contain path separators: {id}"
            )));
        }
        Ok(())
    }

    /// Generate a stable, filesystem-safe slug from a title.
    ///
    /// Lowercases, maps any non-alphanumeric run to a single `-`, trims leading
    /// and trailing `-`, and truncates to a bounded length. Unicode letters and
    /// digits are kept (so CJK titles remain meaningful). Empty results fall
    /// back to `"note"`.
    pub fn slug(title: &str) -> String {
        let mut out = String::new();
        let mut prev_dash = false;
        for ch in title.trim().chars() {
            if ch.is_alphanumeric() {
                for lc in ch.to_lowercase() {
                    out.push(lc);
                }
                prev_dash = false;
            } else if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        }
        let slug = out.trim_matches('-').to_string();
        let slug = truncate_chars(&slug, 80);
        if slug.is_empty() {
            "note".to_string()
        } else {
            slug
        }
    }

    /// Generate a slug for `title` guaranteed unique against `existing` ids by
    /// appending a short stable hash suffix on collision.
    pub fn unique_slug(title: &str, existing: &BTreeSet<String>) -> String {
        let base = Self::slug(title);
        if !existing.contains(&base) {
            return base;
        }
        // Deterministic short suffix derived from the title.
        let suffix = short_hash(title);
        let candidate = format!("{base}-{suffix}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        // Extremely unlikely; walk a counter to guarantee termination.
        for n in 2.. {
            let c = format!("{base}-{suffix}-{n}");
            if !existing.contains(&c) {
                return c;
            }
        }
        unreachable!("unique slug search always terminates")
    }
}

/// Whether a path is a `.md` file.
fn is_markdown(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
}

/// The staging file a write goes through before it is renamed into place. The
/// name is unique per process and per call, and deliberately does not end in
/// `.md`, so a vault scan can never mistake a half-written file for an entry.
fn staging_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("entry.md");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{}-{nonce:x}.tmp", std::process::id()))
}

/// Truncate to at most `max` characters (not bytes), trimming a trailing dash.
fn truncate_chars(s: &str, max: usize) -> String {
    let truncated: String = s.chars().take(max).collect();
    truncated.trim_matches('-').to_string()
}

/// A short, stable hex hash of `s` (FNV-1a, 4 bytes).
fn short_hash(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryKind;

    fn entry(id: &str, title: &str, body: &str) -> KnowledgeEntry {
        KnowledgeEntry {
            id: id.to_string(),
            title: title.to_string(),
            kind: EntryKind::Note,
            tags: vec![],
            created_at: 1,
            updated_at: 1,
            source_session: None,
            scope: Scope::Project,
            status: crate::entry::EntryStatus::Active,
            tier: None,
            importance: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn slug_basics() {
        assert_eq!(Vault::slug("Hello World"), "hello-world");
        assert_eq!(Vault::slug("  fix: the thing!! "), "fix-the-thing");
        assert_eq!(Vault::slug("a___b---c"), "a-b-c");
        assert_eq!(Vault::slug("***"), "note");
        // CJK kept.
        assert!(Vault::slug("配置 数据库").contains('配'));
    }

    #[test]
    fn unique_slug_appends_hash_on_collision() {
        let mut existing = BTreeSet::new();
        existing.insert("hello-world".to_string());
        let s = Vault::unique_slug("Hello World", &existing);
        assert_ne!(s, "hello-world");
        assert!(s.starts_with("hello-world-"));
    }

    #[test]
    fn write_then_scan_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("knowledge"), Scope::Project);
        let e = entry("note-a", "Note A", "body a");
        vault.write(&e).unwrap();
        vault.write(&entry("note-b", "Note B", "body b")).unwrap();

        let scanned = vault.scan().unwrap();
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].id, "note-a");
        assert_eq!(scanned[0].title, "Note A");
        assert!(scanned[0].body.contains("body a"));
    }

    #[test]
    fn scan_skips_bad_files_but_loads_good_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("knowledge");
        std::fs::create_dir_all(&dir).unwrap();
        // A good entry.
        let vault = Vault::new(&dir, Scope::Project);
        vault.write(&entry("good", "Good", "ok")).unwrap();
        // A non-markdown file is ignored.
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        // A markdown file with no frontmatter still loads (tolerant).
        std::fs::write(dir.join("loose.md"), "just a body").unwrap();

        let scanned = vault.scan().unwrap();
        let ids: Vec<&str> = scanned.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"good"));
        assert!(ids.contains(&"loose"));
        assert!(!ids.contains(&"notes"));
    }

    #[test]
    fn scan_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("does-not-exist"), Scope::Global);
        assert!(vault.scan().unwrap().is_empty());
    }

    #[test]
    fn delete_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        vault.write(&entry("gone", "Gone", "x")).unwrap();
        assert!(vault.delete("gone").unwrap());
        assert!(!vault.delete("gone").unwrap());
        assert!(vault.scan().unwrap().is_empty());
    }

    #[test]
    fn resolve_rejects_traversal_and_separators() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        assert!(vault.delete("../escape").is_err());
        assert!(vault.delete("nested/path").is_err());
        assert!(vault.delete("nested\\path").is_err());
        assert!(vault.delete("").is_err());
    }

    #[test]
    fn drafts_are_isolated_from_active_scan() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        // An active entry and a draft entry.
        vault
            .write(&entry("active-one", "Active One", "active body"))
            .unwrap();
        let mut d = entry("draft-one", "Draft One", "draft body");
        d.status = crate::entry::EntryStatus::Draft;
        vault.write_draft(&d).unwrap();

        // Active scan sees only the active entry (drafts live in a subdir).
        let active = vault.scan().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "active-one");

        // Draft scan sees only the draft, with status preserved.
        let drafts = vault.scan_drafts().unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].id, "draft-one");
        assert_eq!(drafts[0].status, crate::entry::EntryStatus::Draft);
    }

    #[test]
    fn draft_write_delete_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        let mut d = entry("d", "D", "x");
        d.status = crate::entry::EntryStatus::Draft;
        vault.write_draft(&d).unwrap();
        assert_eq!(vault.scan_drafts().unwrap().len(), 1);
        assert!(vault.delete_draft("d").unwrap());
        assert!(!vault.delete_draft("d").unwrap());
        assert!(vault.scan_drafts().unwrap().is_empty());
    }

    #[test]
    fn scan_drafts_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        assert!(vault.scan_drafts().unwrap().is_empty());
    }

    #[test]
    fn draft_resolve_rejects_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        assert!(vault.delete_draft("../escape").is_err());
        assert!(vault.delete_draft("nested/path").is_err());
    }

    /// A write must land whole: the entry reads back through `scan`, and no
    /// staging file survives in the vault directory.
    #[test]
    fn write_lands_whole_and_leaves_no_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        vault
            .write(&entry("note-1", "Note one", "the body"))
            .unwrap();

        let scanned = vault.scan().unwrap();
        assert_eq!(scanned.len(), 1);
        assert_eq!(scanned[0].body, "the body");

        let leftovers: Vec<_> = std::fs::read_dir(vault.root())
            .unwrap()
            .filter_map(|dir| dir.ok())
            .filter(|dir| {
                dir.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"))
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging files left behind: {leftovers:?}"
        );
    }

    /// Two writers may not be inside one vault at the same time. The contended
    /// write must fail with a retriable error instead of hanging, and go through
    /// as soon as the holder is done. A second handle in this process is the
    /// same OS barrier a second process would meet.
    #[test]
    fn a_held_lock_blocks_the_next_writer_until_released() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);

        let guard = vault.lock().unwrap();
        let error = vault
            .write(&entry("note-1", "Note one", "first"))
            .expect_err("a locked vault must reject the writer");
        assert!(
            error.to_string().contains("another writer"),
            "unexpected error: {error}"
        );
        drop(guard);

        vault.write(&entry("note-1", "Note one", "second")).unwrap();
        assert_eq!(vault.scan().unwrap()[0].body, "second");
    }

    /// An abandoned handle must not strand the vault. Closing the handle with no
    /// explicit unlock is how a killed process disappears, and the OS releases
    /// the lock for it — which is why there is no PID file to reclaim here.
    #[test]
    fn a_closed_handle_releases_the_vault_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        let lock_path = vault.root().join(".locks").join("vault.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        file.lock_exclusive().unwrap();
        drop(file);

        let guard = vault.lock().unwrap();
        guard
            .write(&vault, &entry("note-1", "Note one", "after the crash"))
            .unwrap();
        drop(guard);
        assert_eq!(vault.scan().unwrap()[0].body, "after the crash");
    }

    /// One guard covers a whole transaction: a merge that rewrites one file and
    /// deletes another must not wait on the lock it already holds.
    #[test]
    fn one_guard_spans_a_multi_file_transaction() {
        let tmp = tempfile::tempdir().unwrap();
        let vault = Vault::new(tmp.path().join("k"), Scope::Project);
        vault.write(&entry("keep", "Keep", "survivor")).unwrap();
        vault.write(&entry("drop-me", "Drop", "duplicate")).unwrap();

        let guard = vault.lock().unwrap();
        guard
            .write(&vault, &entry("keep", "Keep", "merged body"))
            .unwrap();
        assert!(guard.delete(&vault, "drop-me").unwrap());
        drop(guard);

        let scanned = vault.scan().unwrap();
        assert_eq!(scanned.len(), 1, "the duplicate must be gone");
        assert_eq!(scanned[0].id, "keep");
        assert_eq!(scanned[0].body, "merged body");
        // The lock lives under a dot-directory, so it never becomes an entry.
        assert_eq!(vault.scan().unwrap().len(), 1, "the lock is not an entry");
    }
}
