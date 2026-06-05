//! A Markdown vault — the source of truth for knowledge entries.
//!
//! Each entry is one `*.md` file under the vault root. The vault is responsible
//! for disk I/O only: scanning, writing, deleting, and generating safe file
//! names. All paths are confined to the root (no `..` traversal, no escaping
//! absolute paths) using the same lexical checks as the file-tool
//! `WorkspaceRoot` guard.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use deepagent_core::error::{CoreError, Result};

use crate::entry::{KnowledgeEntry, Scope};

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
    pub fn write(&self, entry: &KnowledgeEntry) -> Result<PathBuf> {
        let path = self.resolve(&entry.id)?;
        std::fs::create_dir_all(&self.root)
            .map_err(|e| CoreError::Persistence(format!("create knowledge vault dir: {e}")))?;
        std::fs::write(&path, entry.to_markdown())
            .map_err(|e| CoreError::Persistence(format!("write knowledge file: {e}")))?;
        Ok(path)
    }

    /// Delete the `.md` file for `id`. Returns whether a file was removed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let path = self.resolve(id)?;
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| CoreError::Persistence(format!("delete knowledge file: {e}")))?;
            Ok(true)
        } else {
            Ok(false)
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
}
