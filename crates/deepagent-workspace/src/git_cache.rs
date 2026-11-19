//! Short-lived Git context shared by all model turns in this host process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::git::{detect_git_context, GitContext};

const TTL: Duration = Duration::from_secs(30);
const MAX_WORKSPACES: usize = 50;

#[derive(Default)]
struct GitContextCache {
    entries: Mutex<HashMap<PathBuf, CacheEntry>>,
    changed: Condvar,
}

struct CacheEntry {
    value: Option<GitContext>,
    loaded_at: Option<Instant>,
    last_used: Instant,
    generation: u64,
    loading: bool,
}

impl GitContextCache {
    fn get_or_load(
        &self,
        root: &Path,
        now: impl Fn() -> Instant,
        load: impl Fn() -> Option<GitContext>,
    ) -> Option<GitContext> {
        let key = worktree_key(root);
        loop {
            let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            let started = now();
            if let Some(entry) = entries.get_mut(&key) {
                entry.last_used = started;
                if entry
                    .loaded_at
                    .is_some_and(|at| started.saturating_duration_since(at) < TTL)
                {
                    tracing::debug!("reusing Git context snapshot");
                    return entry.value.clone();
                }
                if entry.loading {
                    drop(
                        self.changed
                            .wait(entries)
                            .unwrap_or_else(|p| p.into_inner()),
                    );
                    continue;
                }
            } else {
                if entries.len() >= MAX_WORKSPACES {
                    // Never evict a live refresh: invalidation must still be
                    // able to find it. Capacity waiters release the map lock.
                    let oldest = entries
                        .iter()
                        .filter(|(_, e)| !e.loading)
                        .min_by_key(|(_, e)| e.last_used)
                        .map(|(key, _)| key.clone());
                    if let Some(oldest) = oldest {
                        entries.remove(&oldest);
                    } else {
                        drop(
                            self.changed
                                .wait(entries)
                                .unwrap_or_else(|p| p.into_inner()),
                        );
                        continue;
                    }
                }
                entries.insert(
                    key.clone(),
                    CacheEntry {
                        value: None,
                        loaded_at: None,
                        last_used: started,
                        generation: 0,
                        loading: false,
                    },
                );
            }
            let entry = entries.get_mut(&key).expect("refresh slot exists");
            entry.loading = true;
            let generation = entry.generation;
            drop(entries);

            // Git runs outside the lock. An unwinding loader must also wake
            // waiters rather than leaving a permanently occupied slot.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&load));
            let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
            let entry = entries
                .get_mut(&key)
                .expect("live refresh is never evicted");
            entry.loading = false;
            self.changed.notify_all();
            let value = match result {
                Ok(value) => value,
                Err(payload) => {
                    drop(entries);
                    std::panic::resume_unwind(payload);
                }
            };
            if entry.generation != generation {
                continue;
            }
            entry.loaded_at = Some(started);
            entry.value = value.clone();
            return value;
        }
    }

    fn invalidate(&self, path: &Path) {
        self.invalidate_matching(Some(path));
    }

    fn invalidate_matching(&self, path: Option<&Path>) {
        let paths = path.map(|path| {
            let target = physical_path(path);
            // Replacing/removing a final symlink or junction changes the entry
            // in its parent worktree, independently of the external target.
            let entry = path
                .file_name()
                .zip(path.parent())
                .map(|(name, parent)| physical_path(parent).join(name))
                .unwrap_or_else(|| target.clone());
            #[cfg(windows)]
            let entry = PathBuf::from(entry.to_string_lossy().to_lowercase());
            [target, entry]
        });
        let mut entries = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        for (root, entry) in entries.iter_mut() {
            if paths.as_ref().is_none_or(|paths| {
                paths
                    .iter()
                    .any(|p| p.starts_with(root) || root.starts_with(p))
            }) {
                entry.loaded_at = None;
                entry.generation = entry.generation.wrapping_add(1);
            }
        }
        self.changed.notify_all();
    }
}

// This is a best-effort cache identity, never a filesystem authorization.
// Resolve existing ancestors too so creates/deletes invalidate the same key.
fn physical_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let physical = absolute
        .ancestors()
        .find_map(|ancestor| {
            let canonical = std::fs::canonicalize(ancestor).ok()?;
            Some(canonical.join(absolute.strip_prefix(ancestor).ok()?))
        })
        .unwrap_or(absolute);
    #[cfg(windows)]
    let physical = PathBuf::from(physical.to_string_lossy().to_lowercase());
    physical
}

fn worktree_key(root: &Path) -> PathBuf {
    let root = physical_path(root);
    root.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .unwrap_or(&root)
        .to_path_buf()
}

fn cache() -> &'static GitContextCache {
    static CACHE: OnceLock<GitContextCache> = OnceLock::new();
    CACHE.get_or_init(GitContextCache::default)
}

/// Reuse a workspace's snapshot for at most thirty seconds.
pub fn cached_git_context(root: &Path) -> Option<GitContext> {
    cache().get_or_load(root, Instant::now, || detect_git_context(root))
}

/// Invalidate snapshots affected by an internal file or Git operation.
pub fn invalidate_git_context(path: &Path) {
    cache().invalidate(path);
}

/// Invalidates on entry and exit, including errors, unwinding and cancellation.
#[must_use]
pub struct GitContextMutation(Option<PathBuf>);

impl GitContextMutation {
    pub fn new(path: &Path) -> Self {
        // Preserve the logical entry: its target can change during a mutation.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        invalidate_git_context(&path);
        Self(Some(path))
    }

    /// Arbitrary shell/hook commands can target another open workspace.
    /// Invalidate the bounded host cache without interpreting command text.
    pub fn all_workspaces() -> Self {
        cache().invalidate_matching(None);
        Self(None)
    }
}

impl Drop for GitContextMutation {
    fn drop(&mut self) {
        cache().invalidate_matching(self.0.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc, Barrier};

    fn context(branch: &str) -> Option<GitContext> {
        Some(GitContext {
            branch: branch.into(),
            has_uncommitted: false,
            changed_files: Vec::new(),
            recent_commits: Vec::new(),
        })
    }

    #[test]
    fn reuses_snapshot_until_exactly_thirty_seconds_and_isolates_workspaces() {
        let cache = GitContextCache::default();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let millis = AtomicU64::new(0);
        let now = || start + Duration::from_millis(millis.load(Ordering::SeqCst));
        let loads = AtomicUsize::new(0);
        let load = || {
            loads.fetch_add(1, Ordering::SeqCst);
            context("main")
        };
        cache.get_or_load(first.path(), now, load);
        millis.store(29_999, Ordering::SeqCst);
        assert_eq!(cache.get_or_load(first.path(), now, load), context("main"));
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "fresh snapshot was reloaded"
        );
        assert_eq!(
            cache.get_or_load(second.path(), now, || context("other")),
            context("other")
        );
        millis.store(30_000, Ordering::SeqCst);
        cache.get_or_load(first.path(), now, load);
        assert_eq!(
            loads.load(Ordering::SeqCst),
            2,
            "TTL boundary did not refresh"
        );
    }

    #[test]
    fn negative_results_expire_and_do_not_hide_repository_creation_forever() {
        let cache = GitContextCache::default();
        let root = tempfile::tempdir().unwrap();
        let start = Instant::now();
        assert!(cache.get_or_load(root.path(), || start, || None).is_none());
        assert!(cache
            .get_or_load(root.path(), || start, || context("new"))
            .is_none());
        assert_eq!(
            cache.get_or_load(root.path(), || start + TTL, || context("new")),
            context("new")
        );
    }

    #[test]
    fn file_invalidation_includes_subdirectory_aliases_and_preserves_other_workspace() {
        let cache = GitContextCache::default();
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        cache.get_or_load(root.path(), Instant::now, || context("old"));
        cache.get_or_load(other.path(), Instant::now, || context("other"));
        assert_eq!(
            cache.get_or_load(&root.path().join("src"), Instant::now, || context("wrong")),
            context("old")
        );
        cache.invalidate(&root.path().join("src/not-created-yet.rs"));
        assert_eq!(
            cache.get_or_load(root.path(), Instant::now, || context("new")),
            context("new")
        );
        assert_eq!(
            cache.get_or_load(other.path(), Instant::now, || context("wrong")),
            context("other")
        );
    }

    #[test]
    fn concurrent_queries_share_one_refresh() {
        let cache = Arc::new(GitContextCache::default());
        let root = tempfile::tempdir().unwrap();
        let start = Arc::new(Barrier::new(8));
        let loads = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            let mut tasks = Vec::new();
            for _ in 0..8 {
                tasks.push(scope.spawn(|| {
                    start.wait();
                    cache.get_or_load(root.path(), Instant::now, || {
                        loads.fetch_add(1, Ordering::SeqCst);
                        context("shared")
                    })
                }));
            }
            for task in tasks {
                assert_eq!(task.join().unwrap(), context("shared"));
            }
        });
        assert_eq!(loads.load(Ordering::SeqCst), 1, "duplicate Git refreshes");
    }

    #[test]
    fn invalidation_during_refresh_discards_the_in_flight_snapshot() {
        let cache = GitContextCache::default();
        let root = tempfile::tempdir().unwrap();
        let loads = AtomicUsize::new(0);
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            let cache_ref = &cache;
            let root_ref = root.path();
            let loads_ref = &loads;
            let task = scope.spawn(move || {
                cache_ref.get_or_load(root_ref, Instant::now, || {
                    if loads_ref.fetch_add(1, Ordering::SeqCst) == 0 {
                        started_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                        context("stale")
                    } else {
                        context("fresh")
                    }
                })
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            cache.invalidate(root.path());
            release_tx.send(()).unwrap();
            assert_eq!(task.join().unwrap(), context("fresh"));
        });
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn capacity_is_bounded_and_evicts_the_oldest_idle_snapshot() {
        let cache = GitContextCache::default();
        let root = tempfile::tempdir().unwrap();
        for index in 0..=MAX_WORKSPACES {
            let path = root.path().join(index.to_string());
            std::fs::create_dir(&path).unwrap();
            cache.get_or_load(&path, Instant::now, || context("cached"));
        }
        assert_eq!(
            cache.get_or_load(&root.path().join("0"), Instant::now, || context("evicted")),
            context("evicted")
        );
        assert_eq!(
            cache.get_or_load(
                &root.path().join(MAX_WORKSPACES.to_string()),
                Instant::now,
                || context("wrong")
            ),
            context("cached")
        );
    }

    #[test]
    fn unknown_command_scope_invalidates_every_workspace_on_entry_and_exit() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for root in [first.path(), second.path()] {
            cache().get_or_load(root, Instant::now, || context("before"));
        }
        let mutation = GitContextMutation::all_workspaces();
        for root in [first.path(), second.path()] {
            assert_eq!(
                cache().get_or_load(root, Instant::now, || context("during")),
                context("during")
            );
        }
        drop(mutation);
        for root in [first.path(), second.path()] {
            assert_eq!(
                cache().get_or_load(root, Instant::now, || context("after")),
                context("after")
            );
        }
    }

    #[cfg(any(windows, unix))]
    #[test]
    fn removing_directory_alias_invalidates_the_containing_worktree() {
        let workspace = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".git")).unwrap();
        let alias = workspace.path().join("alias");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&alias)
                .arg(target.path())
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(target.path(), &alias).unwrap();
        cache().get_or_load(workspace.path(), Instant::now, || context("before"));
        let mutation = GitContextMutation::new(&alias);
        #[cfg(windows)]
        std::fs::remove_dir(&alias).unwrap();
        #[cfg(unix)]
        std::fs::remove_file(&alias).unwrap();
        drop(mutation);
        assert_eq!(
            cache().get_or_load(workspace.path(), Instant::now, || context("after")),
            context("after"),
            "alias replacement only invalidated its external target"
        );
    }
}
