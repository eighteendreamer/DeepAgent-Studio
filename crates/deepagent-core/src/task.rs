//! Task state machine.
//!
//! Mirrors the states described in 开发计划.md Phase 2:
//!
//! ```text
//! Queued -> Running -> { Paused | WaitingApproval | Failed | Completed }
//! ```
//!
//! Transitions are validated centrally so the runtime cannot drive a task into
//! an illegal state (e.g. resurrecting a `Completed` task into `Running`).

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Lifecycle state of a runtime task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Created but not yet started.
    Queued,
    /// Actively executing in the runtime loop.
    Running,
    /// Temporarily suspended; can resume to [`TaskState::Running`].
    Paused,
    /// Blocked awaiting human approval (e.g. a dangerous tool call).
    WaitingApproval,
    /// Terminated unsuccessfully.
    Failed,
    /// Terminated successfully.
    Completed,
}

impl TaskState {
    /// Whether this is a terminal state (no further transitions allowed).
    pub const fn is_terminal(&self) -> bool {
        matches!(self, TaskState::Failed | TaskState::Completed)
    }

    /// Whether the task is actively or potentially progressing.
    pub const fn is_active(&self) -> bool {
        matches!(
            self,
            TaskState::Queued | TaskState::Running | TaskState::Paused | TaskState::WaitingApproval
        )
    }

    /// Whether a transition `self -> next` is permitted.
    pub const fn can_transition_to(&self, next: TaskState) -> bool {
        use TaskState::*;
        matches!(
            (self, next),
            (Queued, Running)
                | (Queued, Failed)
                | (Running, Paused)
                | (Running, WaitingApproval)
                | (Running, Failed)
                | (Running, Completed)
                | (Paused, Running)
                | (Paused, Failed)
                | (WaitingApproval, Running)
                | (WaitingApproval, Failed)
        )
    }

    /// Attempt a transition, returning the next state or an error describing
    /// the illegal transition.
    pub fn transition(self, next: TaskState) -> Result<TaskState> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(CoreError::IllegalTransition {
                from: format!("{self:?}"),
                to: format!("{next:?}"),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskState::*;
    use super::*;

    #[test]
    fn happy_path_runs_to_completion() {
        let s = Queued.transition(Running).unwrap();
        let s = s.transition(Completed).unwrap();
        assert!(s.is_terminal());
    }

    #[test]
    fn cannot_resurrect_completed() {
        let err = Completed.transition(Running).unwrap_err();
        assert!(matches!(err, CoreError::IllegalTransition { .. }));
    }

    #[test]
    fn approval_loop() {
        let s = Queued.transition(Running).unwrap();
        let s = s.transition(WaitingApproval).unwrap();
        let s = s.transition(Running).unwrap();
        assert_eq!(s, Running);
    }

    #[test]
    fn pause_resume() {
        assert!(Running.can_transition_to(Paused));
        assert!(Paused.can_transition_to(Running));
        assert!(!Paused.can_transition_to(Completed));
    }

    #[test]
    fn terminal_and_active_classification() {
        assert!(Failed.is_terminal());
        assert!(!Failed.is_active());
        assert!(Running.is_active());
        assert!(!Running.is_terminal());
    }
}
