//! Per-run file state cache for built-in file tools.

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
            },
        );
        file
    }

    /// Remove a single path from the cache.
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }
}

fn content_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}
