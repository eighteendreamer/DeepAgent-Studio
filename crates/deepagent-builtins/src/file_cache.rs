//! Per-run file state cache for built-in file tools.
//!
//! Beyond plain content caching this cache also records the **most recent
//! `read_file` call's `(offset, limit)`** for each path. Two helpers built on
//! top of that:
//!
//! - [`FileStateCache::last_read_matches`] supports the FILE_UNCHANGED_STUB
//!   path in `read_file`: when the model re-reads the same file with the same
//!   offset/limit and the cached content hasn't been invalidated by a write,
//!   the tool returns a short stub instead of the full body.
//! - [`FileStateCache::has_been_read`] supports the read-before-edit invariant
//!   for `edit_file` / `multi_edit` / `write_file`: an entry is created the
//!   moment `read_file` succeeds, so checking `entries.contains_key(path)` is
//!   equivalent to "the model has seen this file in this session".

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A cached text file snapshot.
#[derive(Debug, Clone)]
pub struct CachedFile {
    /// Full UTF-8 content captured at read time.
    pub content: String,
    /// Stable hash of the cached content within this process.
    pub content_hash: u64,
    /// Number of lines in the full cached file.
    pub line_count: usize,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    file: CachedFile,
    modified: Option<SystemTime>,
    len: u64,
    /// `(offset, limit)` of the most recent read_file invocation that matched
    /// this entry. `None` until the first call to [`FileStateCache::record_read`].
    last_read: Option<(Option<usize>, Option<usize>)>,
}

/// Per-run cache keyed by resolved file path.
///
/// The cache stores the full UTF-8 content plus cheap metadata. Reads can reuse
/// content when `len` and `modified` still match; write/edit tools invalidate
/// entries after successful writes.
#[derive(Debug, Default)]
pub struct FileStateCache {
    entries: HashMap<PathBuf, CacheEntry>,
}

impl FileStateCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a cached snapshot when filesystem metadata still matches.
    pub fn get_fresh(&self, path: &Path, metadata: &std::fs::Metadata) -> Option<CachedFile> {
        let entry = self.entries.get(path)?;
        if entry.len == metadata.len() && entry.modified == metadata.modified().ok() {
            Some(entry.file.clone())
        } else {
            None
        }
    }

    /// Store a text snapshot for `path` and return the cached representation.
    /// Resets the recorded last-read parameters: a freshly-loaded file has no
    /// prior read history (the caller will record one immediately afterwards).
    pub fn store(
        &mut self,
        path: PathBuf,
        content: String,
        metadata: &std::fs::Metadata,
    ) -> CachedFile {
        let file = CachedFile {
            content_hash: content_hash(&content),
            line_count: content.lines().count(),
            content,
        };
        self.entries.insert(
            path,
            CacheEntry {
                file: file.clone(),
                modified: metadata.modified().ok(),
                len: metadata.len(),
                last_read: None,
            },
        );
        file
    }

    /// Remove a single path from the cache.
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    /// Whether this path has been read at least once in this session and the
    /// entry still exists (i.e. no intervening write has invalidated it).
    /// Used by edit/multi_edit/write to enforce read-before-edit.
    pub fn has_been_read(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    /// Record `(offset, limit)` as the most recent successful `read_file`
    /// invocation for `path`. No-op when `path` is not cached.
    pub fn record_read(&mut self, path: &Path, offset: Option<usize>, limit: Option<usize>) {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.last_read = Some((offset, limit));
        }
    }

    /// Whether the most recent recorded read for `path` used exactly the given
    /// `(offset, limit)`. Returns `false` when the path has never been read,
    /// when the cache entry is missing, or when the parameters differ.
    pub fn last_read_matches(
        &self,
        path: &Path,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> bool {
        self.entries
            .get(path)
            .and_then(|e| e.last_read)
            .is_some_and(|prev| prev == (offset, limit))
    }
}

fn content_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_and_meta(dir: &Path, name: &str, content: &str) -> (PathBuf, std::fs::Metadata) {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        let meta = fs::metadata(&path).unwrap();
        (path, meta)
    }

    #[test]
    fn record_read_then_last_read_matches() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello");
        let mut cache = FileStateCache::new();
        cache.store(path.clone(), "hello".into(), &meta);
        cache.record_read(&path, None, None);
        assert!(cache.last_read_matches(&path, None, None));
        assert!(!cache.last_read_matches(&path, Some(2), None));
        assert!(!cache.last_read_matches(&path, None, Some(50)));
    }

    #[test]
    fn last_read_matches_returns_false_before_record() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello");
        let mut cache = FileStateCache::new();
        cache.store(path.clone(), "hello".into(), &meta);
        // store() resets last_read to None — first read can never match.
        assert!(!cache.last_read_matches(&path, None, None));
    }

    #[test]
    fn store_resets_last_read_history() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello");
        let mut cache = FileStateCache::new();
        cache.store(path.clone(), "hello".into(), &meta);
        cache.record_read(&path, Some(5), Some(10));
        assert!(cache.last_read_matches(&path, Some(5), Some(10)));

        // Re-store (e.g. after external modification): last_read clears.
        cache.store(path.clone(), "world".into(), &meta);
        assert!(!cache.last_read_matches(&path, Some(5), Some(10)));
    }

    #[test]
    fn invalidate_removes_read_history() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello");
        let mut cache = FileStateCache::new();
        cache.store(path.clone(), "hello".into(), &meta);
        cache.record_read(&path, None, None);
        assert!(cache.has_been_read(&path));

        cache.invalidate(&path);
        assert!(!cache.has_been_read(&path));
        assert!(!cache.last_read_matches(&path, None, None));
    }

    #[test]
    fn has_been_read_tracks_store() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello");
        let mut cache = FileStateCache::new();
        assert!(!cache.has_been_read(&path));
        cache.store(path.clone(), "hello".into(), &meta);
        assert!(cache.has_been_read(&path));
    }

    #[test]
    fn record_read_on_unknown_path_is_noop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.txt");
        let mut cache = FileStateCache::new();
        cache.record_read(&path, Some(1), None);
        assert!(!cache.has_been_read(&path));
        assert!(!cache.last_read_matches(&path, Some(1), None));
    }
}
