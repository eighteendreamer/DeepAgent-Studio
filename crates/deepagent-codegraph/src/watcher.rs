//! Watcher support for incremental sync.
//!
//! The OS event backend is intentionally thin: file-change paths are normalised
//! into [`WatchChange`] values, debounced by [`DebouncedSync`], then applied by
//! calling [`CodeGraph::sync`](crate::CodeGraph::sync). The pure core lives in
//! this module so filtering and debounce behaviour can be tested deterministically.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use deepagent_core::{CoreError, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::types::Language;
use crate::{CodeGraph, IndexStats};

/// Default quiet window before a burst of filesystem events triggers sync.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(500);

const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    ".understand-anything",
    ".codegraph",
    ".deepagent",
];

/// One source-file change observed by the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchChange {
    /// POSIX-style path relative to the project root.
    pub relative_path: String,
}

/// Deterministic debounce accumulator.
#[derive(Debug, Clone)]
pub struct DebouncedSync {
    debounce: Duration,
    pending: BTreeSet<String>,
    last_event_at: Option<Instant>,
}

impl DebouncedSync {
    /// Create an empty accumulator with the given quiet window.
    pub fn new(debounce: Duration) -> Self {
        Self {
            debounce,
            pending: BTreeSet::new(),
            last_event_at: None,
        }
    }

    /// Add one path if it is a source file under an indexable directory.
    pub fn observe_path(&mut self, project_root: &Path, path: &Path, now: Instant) -> bool {
        let Some(change) = normalize_change(project_root, path) else {
            return false;
        };
        self.pending.insert(change.relative_path);
        self.last_event_at = Some(now);
        true
    }

    /// Returns true once there is pending work and the quiet window has elapsed.
    pub fn is_ready(&self, now: Instant) -> bool {
        self.last_event_at
            .map(|last| !self.pending.is_empty() && now.duration_since(last) >= self.debounce)
            .unwrap_or(false)
    }

    /// Drain pending paths when ready; otherwise return an empty batch.
    pub fn drain_ready(&mut self, now: Instant) -> Vec<String> {
        if !self.is_ready(now) {
            return Vec::new();
        }
        self.last_event_at = None;
        std::mem::take(&mut self.pending).into_iter().collect()
    }

    /// Number of unique source paths waiting for debounce.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

/// Small controller that converts debounced changes into graph sync calls.
#[derive(Debug)]
pub struct WatchController {
    project_root: PathBuf,
    debounce: DebouncedSync,
}

impl WatchController {
    pub fn new(project_root: PathBuf, debounce: Duration) -> Self {
        Self {
            project_root,
            debounce: DebouncedSync::new(debounce),
        }
    }

    pub fn observe_path(&mut self, path: &Path, now: Instant) -> bool {
        self.debounce.observe_path(&self.project_root, path, now)
    }

    pub fn sync_if_ready(
        &mut self,
        graph: &mut CodeGraph,
        now: Instant,
    ) -> Result<Option<IndexStats>> {
        let changed = self.debounce.drain_ready(now);
        if changed.is_empty() {
            return Ok(None);
        }
        tracing::debug!(changed = ?changed, "watcher debounce elapsed; syncing code graph");
        graph.sync().map(Some)
    }
}

/// Live filesystem watcher that automatically drives incremental sync.
pub struct CodeGraphWatcher {
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    controller: WatchController,
    debounce: Duration,
}

impl std::fmt::Debug for CodeGraphWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeGraphWatcher")
            .field("debounce", &self.debounce)
            .finish_non_exhaustive()
    }
}

impl CodeGraphWatcher {
    /// Start watching `project_root` recursively using the platform backend
    /// chosen by `notify` (FSEvents / inotify / ReadDirectoryChangesW / kqueue).
    pub fn open(project_root: PathBuf, debounce: Duration) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = tx.send(event);
            },
            Config::default(),
        )
        .map_err(map_notify_error)?;

        watcher
            .watch(&project_root, RecursiveMode::Recursive)
            .map_err(map_notify_error)?;

        Ok(Self {
            _watcher: watcher,
            receiver: rx,
            controller: WatchController::new(project_root, debounce),
            debounce,
        })
    }

    /// Process all currently queued watcher events and sync if the debounce
    /// window has elapsed.
    pub fn poll_once(&mut self, graph: &mut CodeGraph) -> Result<Option<IndexStats>> {
        let now = Instant::now();
        while let Ok(event) = self.receiver.try_recv() {
            self.observe_notify_result(event, now)?;
        }
        self.controller.sync_if_ready(graph, now)
    }

    /// Run forever, syncing after each quiet window. Intended for host code that
    /// owns a background thread.
    pub fn run_blocking(&mut self, graph: &mut CodeGraph) -> Result<()> {
        loop {
            match self.receiver.recv_timeout(self.debounce) {
                Ok(event) => {
                    let now = Instant::now();
                    self.observe_notify_result(event, now)?;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.controller.sync_if_ready(graph, Instant::now())?;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(CoreError::other("codegraph watcher channel disconnected"));
                }
            }
        }
    }

    fn observe_notify_result(
        &mut self,
        event: notify::Result<Event>,
        now: Instant,
    ) -> Result<usize> {
        let event = event.map_err(map_notify_error)?;
        Ok(self.controller.observe_notify_event(&event, now))
    }
}

impl WatchController {
    /// Feed one `notify` event into the debounce accumulator.
    pub fn observe_notify_event(&mut self, event: &Event, now: Instant) -> usize {
        if !is_source_change_event(&event.kind) {
            return 0;
        }
        event
            .paths
            .iter()
            .filter(|path| self.observe_path(path, now))
            .count()
    }
}

/// Normalize and filter a watcher path.
pub fn normalize_change(project_root: &Path, path: &Path) -> Option<WatchChange> {
    if should_ignore_path(path) || Language::from_path(path) == Language::Other {
        return None;
    }
    let relative = path.strip_prefix(project_root).unwrap_or(path);
    Some(WatchChange {
        relative_path: posix_path(relative),
    })
}

fn is_source_change_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

fn map_notify_error(error: notify::Error) -> CoreError {
    CoreError::other(format!("codegraph watcher error: {error}"))
}

fn should_ignore_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name) if name.to_str().map(|s| IGNORED_DIRS.contains(&s)).unwrap_or(false)
        )
    })
}

fn posix_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, DataChange, ModifyKind};

    #[test]
    fn filters_non_source_and_ignored_directories() {
        let root = Path::new("/repo");

        assert_eq!(
            normalize_change(root, Path::new("/repo/src/main.rs"))
                .unwrap()
                .relative_path,
            "src/main.rs"
        );
        assert!(normalize_change(root, Path::new("/repo/README.md")).is_none());
        assert!(normalize_change(root, Path::new("/repo/target/debug/app.rs")).is_none());
        assert!(normalize_change(root, Path::new("/repo/node_modules/pkg/index.js")).is_none());
    }

    #[test]
    fn debounce_merges_bursts_until_quiet_window() {
        let root = Path::new("/repo");
        let start = Instant::now();
        let mut debounced = DebouncedSync::new(Duration::from_millis(100));

        assert!(debounced.observe_path(root, Path::new("/repo/src/a.rs"), start));
        assert!(debounced.observe_path(
            root,
            Path::new("/repo/src/a.rs"),
            start + Duration::from_millis(10)
        ));
        assert!(debounced.observe_path(
            root,
            Path::new("/repo/src/b.py"),
            start + Duration::from_millis(20)
        ));

        assert_eq!(debounced.pending_len(), 2);
        assert!(debounced
            .drain_ready(start + Duration::from_millis(119))
            .is_empty());
        assert_eq!(
            debounced.drain_ready(start + Duration::from_millis(120)),
            vec!["src/a.rs".to_string(), "src/b.py".to_string()]
        );
        assert_eq!(debounced.pending_len(), 0);
    }

    #[test]
    fn notify_events_feed_source_paths_only() {
        let root = PathBuf::from("/repo");
        let start = Instant::now();
        let mut controller = WatchController::new(root, Duration::from_millis(100));

        let event = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Content)))
            .add_path(PathBuf::from("/repo/src/lib.rs"))
            .add_path(PathBuf::from("/repo/README.md"));

        assert_eq!(controller.observe_notify_event(&event, start), 1);
        assert_eq!(controller.debounce.pending_len(), 1);

        let access = Event::new(EventKind::Access(AccessKind::Any))
            .add_path(PathBuf::from("/repo/src/lib.rs"));
        assert_eq!(controller.observe_notify_event(&access, start), 0);
        assert_eq!(controller.debounce.pending_len(), 1);
    }
}
