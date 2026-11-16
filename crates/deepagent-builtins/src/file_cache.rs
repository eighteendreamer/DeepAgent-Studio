//! Shared byte snapshots with model-context-local read evidence.
//!
//! Beyond plain content caching this cache also records the **most recent
//! `read_file` call's `(offset, limit)`** for each path. Two helpers built on
//! top of that:
//!
//! - [`FileStateCache::last_read_matches`] supports the FILE_UNCHANGED_STUB
//!   path in `read_file`: when the model re-reads the same file with the same
//!   offset/limit and the cached content hasn't been invalidated by a write,
//!   the tool returns a short stub instead of the full body.
//! - Cached bytes alone do not authorize editing: successful reads record the
//!   delivered windows. Mutations compare those bytes with the current file
//!   and restrict edits to observed ranges; full replacement needs full coverage.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use deepagent_tools::ToolObservation;

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
    reads: Vec<ReadObservation>,
}

#[derive(Debug, Clone)]
struct ReadObservation {
    /// None is the legacy direct Tool API, never a wildcard for scoped runs.
    context: Option<ToolObservation>,
    last_read: Option<(Option<usize>, Option<usize>)>,
    observed: Vec<Range<usize>>,
}

impl ReadObservation {
    fn matches(&self, context: Option<&ToolObservation>) -> bool {
        match (&self.context, context) {
            (Some(own), Some(context)) => own.same_context(context),
            (None, None) => true,
            _ => false,
        }
    }
}

impl CacheEntry {
    fn read(&self, context: Option<&ToolObservation>) -> Option<&ReadObservation> {
        self.reads.iter().find(|read| read.matches(context))
    }

    fn read_mut(&mut self, context: Option<&ToolObservation>) -> Option<&mut ReadObservation> {
        // Reclaim expired generations lazily without clearing reusable bytes
        // or any live sibling's windows. Late old invocations cannot reinsert.
        self.reads.retain(|read| {
            read.context
                .as_ref()
                .is_none_or(ToolObservation::is_current)
        });
        if context.is_some_and(|context| !context.is_current()) {
            return None;
        }
        let index = self.reads.iter().position(|read| read.matches(context));
        let index = index.unwrap_or_else(|| {
            self.reads.push(ReadObservation {
                context: context.cloned(),
                last_read: None,
                observed: Vec::new(),
            });
            self.reads.len() - 1
        });
        self.reads.get_mut(index)
    }
}

pub(crate) type SharedFileStateCache = Arc<Mutex<FileStateCache>>;

#[derive(Clone)]
pub(crate) struct ObservedFile {
    pub content: String,
    pub ranges: Vec<Range<usize>>,
}

impl ObservedFile {
    pub fn covers(&self, range: &Range<usize>) -> bool {
        self.ranges
            .iter()
            .any(|seen| seen.start <= range.start && seen.end >= range.end)
    }
}

/// Byte cache keyed by resolved file path, with separate read evidence for each
/// model context. Direct unscoped callers retain their legacy cache-local view.
///
/// The cache stores full UTF-8 content and delivered ranges. Metadata freshness
/// is only an optimization hint for external callers; production read/mutate
/// paths compare actual bytes. Successful mutations invalidate entries.
#[derive(Debug, Default)]
pub struct FileStateCache {
    entries: HashMap<PathBuf, CacheEntry>,
}

impl FileStateCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Return a metadata-fresh candidate, not proof of unchanged content or
    /// edit authority. Callers must compare current bytes for either purpose.
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
                reads: Vec::new(),
            },
        );
        file
    }

    /// Remove a single path from the cache.
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }

    /// Whether a legacy unscoped read, rather than just a cache load, was recorded.
    /// This is not a version check and must not alone authorize a mutation.
    pub fn has_been_read(&self, path: &Path) -> bool {
        self.entries
            .get(path)
            .and_then(|entry| entry.read(None))
            .is_some_and(|read| read.last_read.is_some())
    }

    pub(crate) fn observed(
        &self,
        path: &Path,
        context: Option<&ToolObservation>,
    ) -> Option<ObservedFile> {
        let entry = self.entries.get(path)?;
        let read = entry.read(context)?;
        read.last_read?;
        Some(ObservedFile {
            content: entry.file.content.clone(),
            ranges: read.observed.clone(),
        })
    }

    pub(crate) fn matching_content(&self, path: &Path, content: &str) -> Option<CachedFile> {
        let entry = self.entries.get(path)?;
        (entry.file.content == content).then(|| entry.file.clone())
    }

    /// Record only the actual window delivered by the tool, including its raw
    /// line terminators. Repeated windows accumulate only for identical bytes.
    pub(crate) fn record_window(
        &mut self,
        path: &Path,
        context: Option<&ToolObservation>,
        start_line: usize,
        lines: usize,
    ) {
        let Some(entry) = self.entries.get_mut(path) else {
            return;
        };
        let mut start = 0;
        let mut end = 0;
        for (index, line) in entry.file.content.split_inclusive('\n').enumerate() {
            if index < start_line.saturating_sub(1) {
                start += line.len();
            }
            if index < start_line.saturating_sub(1).saturating_add(lines) {
                end += line.len();
            }
        }
        if lines == 0 && !entry.file.content.is_empty() {
            return;
        }
        let Some(read) = entry.read_mut(context) else {
            return;
        };
        read.observed.push(start..end);
        read.observed.sort_by_key(|range| range.start);
        let mut merged: Vec<Range<usize>> = Vec::new();
        for range in read.observed.drain(..) {
            if let Some(previous) = merged
                .last_mut()
                .filter(|previous| previous.end >= range.start)
            {
                previous.end = previous.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        read.observed = merged;
    }

    /// Record `(offset, limit)` as the most recent successful `read_file`
    /// unscoped invocation for `path`. No-op when `path` is not cached.
    pub fn record_read(&mut self, path: &Path, offset: Option<usize>, limit: Option<usize>) {
        self.record_read_in_context(path, None, offset, limit);
    }

    pub(crate) fn record_read_in_context(
        &mut self,
        path: &Path,
        context: Option<&ToolObservation>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) {
        if let Some(read) = self
            .entries
            .get_mut(path)
            .and_then(|entry| entry.read_mut(context))
        {
            read.last_read = Some((offset, limit));
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
        self.last_read_matches_in_context(path, None, offset, limit)
    }

    pub(crate) fn last_read_matches_in_context(
        &self,
        path: &Path,
        context: Option<&ToolObservation>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> bool {
        self.entries
            .get(path)
            .and_then(|entry| entry.read(context))
            .and_then(|read| read.last_read)
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
    fn cached_bytes_are_not_a_successful_read() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello");
        let mut cache = FileStateCache::new();
        assert!(!cache.has_been_read(&path));
        cache.store(path.clone(), "hello".into(), &meta);
        assert!(!cache.has_been_read(&path));
        cache.record_read(&path, None, None);
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

    #[test]
    fn expired_generations_are_reclaimed_without_revoking_live_sibling_windows() {
        let dir = tempdir().unwrap();
        let (path, meta) = write_and_meta(dir.path(), "a.txt", "hello\n");
        let mut cache = FileStateCache::new();
        cache.store(path.clone(), "hello\n".into(), &meta);
        let parent = deepagent_tools::ToolObservationScope::default();
        let parent_token = parent.capture();
        cache.record_read_in_context(&path, Some(&parent_token), None, None);
        cache.record_window(&path, Some(&parent_token), 1, 1);
        let sibling = deepagent_tools::ToolObservationScope::default();
        for _ in 0..64 {
            let old = sibling.capture();
            cache.record_read_in_context(&path, Some(&old), None, None);
            sibling.reset();
            // A late invocation cannot restore an expired record.
            cache.record_read_in_context(&path, Some(&old), None, None);
            cache.record_window(&path, Some(&old), 1, 1);
            assert!(cache.observed(&path, Some(&old)).is_none());
            assert_eq!(cache.entries[&path].reads.len(), 1);
            assert!(cache
                .observed(&path, Some(&parent_token))
                .unwrap()
                .covers(&(0..6)));
            assert!(cache.matching_content(&path, "hello\n").is_some());
        }
    }
}
