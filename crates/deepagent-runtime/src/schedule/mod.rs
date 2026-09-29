//! Scheduled prompts (Claude Code `useScheduledTasks` / `cronTasks.ts` parity).
//!
//! A scheduled task is a prompt to run at cron-specified times, persisted in
//! `<workspace>/.deepagent/scheduled-tasks.json`. The pieces:
//!
//! - [`cron`] — 5-field cron parsing + next-match computation (pure calendar math).
//! - [`store`] — file-backed persistence with tolerant reads and batched writes.
//! - [`scheduler`] — the evaluation loop: a pure, time-injectable poll over the
//!   store + a thin `tokio::time` tick wrapper for real runs.

pub mod cron;
pub mod scheduler;
pub mod store;

pub use cron::{next_local_match, parse_5_field, CronFields};
pub use scheduler::{run_tick_loop, CronScheduler, FireOutcome};
pub use store::{ScheduledTask, ScheduledTaskStore};
