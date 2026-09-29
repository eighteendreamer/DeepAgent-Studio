//! Scheduled-task persistence (`scheduled_tasks.json`).
//!
//! Rust counterpart to Claude Code's `cronTasks.ts`. Tasks live in a single
//! JSON file under the workspace's `.deepagent/` directory:
//!
//! ```json
//! { "tasks": [ { "id", "cron", "prompt", "createdAt", "recurring"?, "lastFiredAt"?, "permanent"? } ] }
//! ```
//!
//! Two flavors, matching CC:
//! - **One-shot** (`recurring` absent/false) — fire once, then auto-delete.
//! - **Recurring** (`recurring: true`) — fire, reschedule from now, persist
//!   until deleted via `CronDelete` or auto-expiry after 7 days.
//!
//! File reads are tolerant: a missing/empty/malformed file yields an empty
//! list, and entries with invalid cron strings or missing required fields are
//! skipped so a single bad entry never blocks the whole file. Writes create
//! `.deepagent/` if missing and always write `{ "tasks": [...] }` (an empty
//! file, never a delete) so a file watcher sees the last-task-removed change.

use std::fs;
use std::path::{Path, PathBuf};

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A persisted scheduled task.
///
/// Field shape mirrors CC's `CronTask` on disk (`camelCase`); only `permanent`
/// is deliberately excluded from the `CronCreate` tool surface (CC goes further
/// and reserves it for assistant-mode built-ins — we keep the field for parity
/// but do not populate it from any tool).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    /// Short id (first 8 hex chars of a v4 UUID).
    pub id: String,
    /// 5-field cron string in local time — validated on write and re-validated
    /// on read.
    pub cron: String,
    /// Prompt to run when the task fires.
    pub prompt: String,
    /// Epoch ms when the task was created. Anchor for missed-task detection.
    pub created_at: i64,
    /// Epoch ms of the most recent fire. Written back after each recurring fire
    /// so next-fire reconstruction survives restarts. Never set for one-shots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired_at: Option<i64>,
    /// When true, the task reschedules after firing instead of being deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    /// When true, exempt from the 7-day recurring auto-expiry (assistant
    /// escape hatch, CC parity). Not settable through the normal API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent: Option<bool>,
}

impl ScheduledTask {
    pub fn is_recurring(&self) -> bool {
        self.recurring.unwrap_or(false)
    }

    /// The anchor for next-fire computation: `last_fired_at` when the task has
    /// fired before (so a restart reconstructs the same next-fire the prior
    /// process had in memory), otherwise `created_at`.
    pub fn schedule_anchor_ms(&self) -> i64 {
        self.last_fired_at.unwrap_or(self.created_at)
    }
}

/// JSON document shape on disk.
#[derive(Debug, Serialize, Deserialize)]
struct CronFile {
    tasks: Vec<ScheduledTask>,
}

/// Default recurring auto-expiry: 7 days after creation (CC
/// `DEFAULT_CRON_JITTER_CONFIG.recurringMaxAgeMs`). `0` = unlimited.
pub const RECURRING_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// File-backed store for scheduled tasks.
///
/// The file writes are deliberately synchronous (small, low-frequency) so the
/// store stays dependency-light and trivially testable; the scheduler itself
/// wraps the store in its async tick loop.
#[derive(Debug, Clone)]
pub struct ScheduledTaskStore {
    path: PathBuf,
}

impl ScheduledTaskStore {
    /// Store rooted at `workspace/.deepagent/scheduled-tasks.json`.
    pub fn new(workspace: &Path) -> Self {
        Self {
            path: workspace.join(".deepagent").join("scheduled-tasks.json"),
        }
    }

    /// Store at an explicit path (used by tests).
    pub fn new_at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read and validate all tasks. Tolerant of missing/empty/malformed files;
    /// invalid rows are skipped and logged via `tracing`.
    pub fn read(&self) -> Result<Vec<ScheduledTask>> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(CoreError::other(format!(
                    "failed to read {}: {e}",
                    self.path.display()
                )));
            }
        };
        if raw.trim().is_empty() {
            return Ok(Vec::new());
        }
        let file: CronFile = match serde_json::from_str(&raw) {
            Ok(file) => file,
            Err(e) => {
                tracing::warn!(
                    path = %self.path.display(),
                    "scheduled-tasks file unparseable, starting empty: {e}"
                );
                return Ok(Vec::new());
            }
        };
        let mut out = Vec::new();
        for t in file.tasks {
            if t.id.trim().is_empty() || t.cron.trim().is_empty() || t.prompt.trim().is_empty() {
                tracing::warn!("skipping malformed scheduled task without id/cron/prompt");
                continue;
            }
            if crate::schedule::cron::parse_5_field(&t.cron).is_err() {
                tracing::warn!(id = %t.id, cron = %t.cron, "skipping task with invalid cron");
                continue;
            }
            out.push(t);
        }
        Ok(out)
    }

    /// Overwrite the file with `tasks`. Creates `.deepagent/` if missing.
    /// Empty lists write an empty file (never delete) so watchers see changes.
    pub fn write(&self, tasks: &[ScheduledTask]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CoreError::other(format!("failed to create {}: {e}", parent.display()))
            })?;
        }
        let body = CronFile {
            tasks: tasks.to_vec(),
        };
        let json = serde_json::to_string_pretty(&body)
            .map_err(|e| CoreError::invalid(format!("serialize scheduled-tasks: {e}")))?;
        fs::write(&self.path, format!("{json}\n"))
            .map_err(|e| CoreError::other(format!("failed to write {}: {e}", self.path.display())))
    }

    /// Append a task. Returns the new task with its generated id. The caller is
    /// responsible for having validated the cron string.
    pub fn add(
        &self,
        cron: String,
        prompt: String,
        recurring: bool,
        now_ms: i64,
    ) -> Result<ScheduledTask> {
        let task = ScheduledTask {
            id: Uuid::new_v4().to_string()[..8].to_string(),
            cron,
            prompt,
            created_at: now_ms,
            last_fired_at: None,
            recurring: recurring.then_some(true),
            permanent: None,
        };
        let mut tasks = self.read()?;
        tasks.push(task.clone());
        self.write(&tasks)?;
        Ok(task)
    }

    /// Remove tasks by id. No-op when none match (another session may have
    /// raced). Returns the number removed.
    pub fn remove(&self, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let id_set: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let tasks = self.read()?;
        let before = tasks.len();
        let remaining: Vec<ScheduledTask> = tasks
            .into_iter()
            .filter(|t| !id_set.contains(t.id.as_str()))
            .collect();
        let removed = before - remaining.len();
        if removed > 0 {
            self.write(&remaining)?;
        }
        Ok(removed)
    }

    /// Stamp `last_fired_at` on the matching tasks and write back, so a recurring
    /// task's next-fire anchor survives a restart. No-op if none match (deleted
    /// between fire and write).
    pub fn mark_fired(&self, ids: &[String], fired_at_ms: i64) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let id_set: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let tasks = self.read()?;
        let mut changed = 0;
        let updated: Vec<ScheduledTask> = tasks
            .into_iter()
            .map(|mut t| {
                if id_set.contains(t.id.as_str()) {
                    t.last_fired_at = Some(fired_at_ms);
                    changed += 1;
                }
                t
            })
            .collect();
        if changed > 0 {
            self.write(&updated)?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (ScheduledTaskStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = ScheduledTaskStore::new_at(dir.path().join("sub").join("scheduled-tasks.json"));
        (s, dir)
    }

    #[test]
    fn missing_file_reads_empty() {
        let (s, _dir) = store();
        assert!(s.read().unwrap().is_empty());
    }

    #[test]
    fn add_write_read_roundtrip() {
        let (s, _dir) = store();
        let t = s
            .add(
                "0 9 * * *".into(),
                "morning standup".into(),
                true,
                1_700_000_000_000,
            )
            .unwrap();
        assert_eq!(t.id.len(), 8);
        let loaded = s.read().unwrap();
        assert_eq!(loaded, vec![t]);
    }

    #[test]
    fn remove_only_matching_ids() {
        let (s, _dir) = store();
        let a = s.add("0 9 * * *".into(), "a".into(), false, 1_000).unwrap();
        let b = s
            .add("10 10 * * *".into(), "b".into(), true, 1_000)
            .unwrap();
        assert_eq!(s.remove(std::slice::from_ref(&a.id)).unwrap(), 1);
        let rest = s.read().unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].id, b.id);
        // Re-removing the same id is a no-op.
        assert_eq!(s.remove(&[a.id]).unwrap(), 0);
    }

    #[test]
    fn mark_fired_sets_last_fired_and_survives_rewrite() {
        let (s, _dir) = store();
        let t = s.add("0 9 * * *".into(), "p".into(), true, 5_000).unwrap();
        assert_eq!(s.mark_fired(std::slice::from_ref(&t.id), 9_000).unwrap(), 1);
        let loaded = s.read().unwrap();
        assert_eq!(loaded[0].last_fired_at, Some(9_000));
        // Anchor switches from created_at to last_fired_at once fired.
        assert_eq!(loaded[0].schedule_anchor_ms(), 9_000);
        assert_ne!(t.schedule_anchor_ms(), loaded[0].schedule_anchor_ms());
    }

    #[test]
    fn invalid_rows_are_skipped_not_poisonous() {
        let (s, _dir) = store();
        let good = s
            .add("0 9 * * *".into(), "ok".into(), false, 1_000)
            .unwrap();
        // Re-write the file including a row with a broken cron string and a
        // row missing a required field; the good row must still load.
        s.write(&[
            good.clone(),
            ScheduledTask {
                id: "beefface".into(),
                cron: "not a cron".into(),
                prompt: "bad".into(),
                created_at: 1,
                last_fired_at: None,
                recurring: None,
                permanent: None,
            },
        ])
        .unwrap();
        let loaded = s.read().unwrap();
        assert_eq!(loaded, vec![good]);
    }
}
