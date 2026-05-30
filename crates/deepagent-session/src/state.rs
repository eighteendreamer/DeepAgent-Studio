//! The in-memory session projection.
//!
//! [`SessionState`] is a pure fold over [`EventPayload`]s. It contains no IO and
//! is fully deterministic, which is what makes replay reliable: given the same
//! event sequence you always reconstruct the same state.

use std::collections::BTreeMap;

use deepagent_core::event::EventPayload;
use deepagent_core::id::{SessionId, TaskId};
use deepagent_core::task::TaskState;

/// Snapshot of a single task within the session.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskView {
    /// Task id.
    pub id: TaskId,
    /// The goal text.
    pub goal: String,
    /// Current state.
    pub state: TaskState,
}

/// The folded state of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionState {
    /// Session id.
    pub id: SessionId,
    /// Optional title.
    pub title: Option<String>,
    /// The session run mode (set by `SessionStarted`).
    pub mode: deepagent_core::session_mode::SessionMode,
    /// Whether the session has ended.
    pub ended: bool,
    /// Number of conversation messages appended.
    pub message_count: usize,
    /// Number of tool calls requested.
    pub tool_calls_requested: usize,
    /// Number of tool calls completed.
    pub tool_calls_completed: usize,
    /// Tasks keyed by id (BTreeMap keeps a stable iteration order).
    tasks: BTreeMap<TaskId, TaskView>,
}

impl SessionState {
    /// An empty state for `id`.
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            title: None,
            mode: deepagent_core::session_mode::SessionMode::Normal,
            ended: false,
            message_count: 0,
            tool_calls_requested: 0,
            tool_calls_completed: 0,
            tasks: BTreeMap::new(),
        }
    }

    /// Build a state by folding a sequence of payloads.
    pub fn replay<'a, I>(id: SessionId, payloads: I) -> Self
    where
        I: IntoIterator<Item = &'a EventPayload>,
    {
        let mut state = Self::new(id);
        for p in payloads {
            state.apply(p);
        }
        state
    }

    /// Apply a single event payload to the state (the fold step).
    pub fn apply(&mut self, payload: &EventPayload) {
        match payload {
            EventPayload::SessionStarted { title, mode } => {
                self.title = title.clone();
                self.mode = *mode;
            }
            EventPayload::SessionEnded { .. } => {
                self.ended = true;
            }
            EventPayload::TaskCreated { task_id, goal } => {
                self.tasks.insert(
                    *task_id,
                    TaskView {
                        id: *task_id,
                        goal: goal.clone(),
                        state: TaskState::Queued,
                    },
                );
            }
            EventPayload::TaskStateChanged { task_id, to, .. } => {
                if let Some(t) = self.tasks.get_mut(task_id) {
                    t.state = *to;
                }
            }
            EventPayload::MessageAppended { .. } => {
                self.message_count += 1;
            }
            EventPayload::ToolCallRequested { .. } => {
                self.tool_calls_requested += 1;
            }
            EventPayload::ToolCallCompleted { .. } => {
                self.tool_calls_completed += 1;
            }
            EventPayload::ContextCompacted { .. } | EventPayload::Note { .. } => {}
            // `EventPayload` is `#[non_exhaustive]`; future variants that do
            // not affect the projection are intentionally ignored here.
            _ => {}
        }
    }

    /// Look up a task by id.
    pub fn task(&self, id: TaskId) -> Option<&TaskView> {
        self.tasks.get(&id)
    }

    /// All tasks in stable order.
    pub fn tasks(&self) -> Vec<&TaskView> {
        self.tasks.values().collect()
    }

    /// Count of tasks not in a terminal state.
    pub fn active_task_count(&self) -> usize {
        self.tasks.values().filter(|t| t.state.is_active()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_builds_expected_state() {
        let sid = SessionId::new();
        let task = TaskId::new();
        let payloads = [
            EventPayload::SessionStarted {
                title: Some("t".into()),
                mode: deepagent_core::session_mode::SessionMode::Normal,
            },
            EventPayload::TaskCreated {
                task_id: task,
                goal: "g".into(),
            },
            EventPayload::TaskStateChanged {
                task_id: task,
                from: TaskState::Queued,
                to: TaskState::Running,
            },
        ];
        let state = SessionState::replay(sid, payloads.iter());
        assert_eq!(state.title.as_deref(), Some("t"));
        assert_eq!(state.task(task).unwrap().state, TaskState::Running);
        assert_eq!(state.active_task_count(), 1);
    }

    #[test]
    fn replay_is_deterministic() {
        let sid = SessionId::new();
        let payloads = [
            EventPayload::Note { text: "a".into() },
            EventPayload::MessageAppended {
                message: deepagent_core::message::Message::user("x"),
            },
        ];
        let a = SessionState::replay(sid, payloads.iter());
        let b = SessionState::replay(sid, payloads.iter());
        assert_eq!(a, b);
        assert_eq!(a.message_count, 1);
    }
}
