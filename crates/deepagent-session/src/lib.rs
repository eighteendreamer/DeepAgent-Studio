//! # deepagent-session
//!
//! Session management (开发计划.md Phase 2 §2): append-only sessions with
//! **replay** and **recovery**.
//!
//! A [`Session`] is the in-memory projection built by folding the append-only
//! event stream from [`deepagent_persistence::event_store::EventStore`]. The
//! key idea (开发提示词.md §3) is that the event log is authoritative: the
//! current state is always `replay(events)`, so a crash mid-task is recovered
//! simply by reloading and re-folding the stream.

pub mod state;

use deepagent_core::clock::{Clock, Timestamp};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload};
use deepagent_core::id::{SessionId, TaskId};
use deepagent_persistence::event_store::{EventStore, SessionRecord};
use deepagent_persistence::Database;

pub use state::SessionState;

/// A live session bound to a database. Mutating operations append events and
/// keep the in-memory [`SessionState`] projection in sync.
pub struct Session<'db, C: Clock> {
    db: &'db Database,
    clock: &'db C,
    id: SessionId,
    state: SessionState,
}

impl<'db, C: Clock> Session<'db, C> {
    /// Start a brand new session, writing the initial `SessionStarted` event.
    pub fn create(db: &'db Database, clock: &'db C, title: Option<&str>) -> Result<Self> {
        let id = SessionId::new();
        let now = clock.now();
        let store = EventStore::new(db);
        store.create_session(id, title, now)?;
        store.append(
            id,
            EventPayload::SessionStarted {
                title: title.map(|s| s.to_string()),
            },
            now,
        )?;

        let mut state = SessionState::new(id);
        // Fold the one event we just wrote.
        state.apply(&EventPayload::SessionStarted {
            title: title.map(|s| s.to_string()),
        });

        Ok(Self {
            db,
            clock,
            id,
            state,
        })
    }

    /// Recover an existing session by replaying its event stream.
    ///
    /// This is the crash-recovery path: regardless of where execution stopped,
    /// folding the durable events reconstructs the exact prior state.
    pub fn recover(db: &'db Database, clock: &'db C, id: SessionId) -> Result<Self> {
        let store = EventStore::new(db);
        if store.get_session(id)?.is_none() {
            return Err(CoreError::not_found(format!("session {id}")));
        }
        let events = store.load_session(id)?;
        let state = SessionState::replay(id, events.iter().map(|e| &e.payload));
        tracing::info!(%id, events = events.len(), "recovered session");
        Ok(Self {
            db,
            clock,
            id,
            state,
        })
    }

    /// The session id.
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// The current in-memory projection.
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Append a payload to the stream and update the projection. Returns the
    /// persisted [`Event`].
    pub fn append(&mut self, payload: EventPayload) -> Result<Event> {
        let now = self.clock.now();
        let store = EventStore::new(self.db);
        let event = store.append(self.id, payload, now)?;
        self.state.apply(&event.payload);
        Ok(event)
    }

    /// Convenience: create a task and append the corresponding event.
    pub fn create_task(&mut self, goal: impl Into<String>) -> Result<TaskId> {
        let task_id = TaskId::new();
        self.append(EventPayload::TaskCreated {
            task_id,
            goal: goal.into(),
        })?;
        Ok(task_id)
    }

    /// Transition a task's state, validating the transition and recording it.
    pub fn transition_task(
        &mut self,
        task_id: TaskId,
        to: deepagent_core::task::TaskState,
    ) -> Result<()> {
        let current = self
            .state
            .task(task_id)
            .ok_or_else(|| CoreError::not_found(format!("task {task_id}")))?
            .state;
        // Validate before persisting so illegal transitions never hit the log.
        current.transition(to)?;
        self.append(EventPayload::TaskStateChanged {
            task_id,
            from: current,
            to,
        })?;
        Ok(())
    }

    /// End the session gracefully.
    pub fn end(&mut self, reason: Option<&str>) -> Result<()> {
        self.append(EventPayload::SessionEnded {
            reason: reason.map(|s| s.to_string()),
        })?;
        Ok(())
    }

    /// The persisted session record (metadata).
    pub fn record(&self) -> Result<SessionRecord> {
        EventStore::new(self.db)
            .get_session(self.id)?
            .ok_or_else(|| CoreError::not_found(format!("session {}", self.id)))
    }

    /// When this session was last updated.
    pub fn updated_at(&self) -> Result<Timestamp> {
        Ok(self.record()?.updated_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::FixedClock;
    use deepagent_core::message::Message;
    use deepagent_core::task::TaskState;

    #[test]
    fn create_and_recover_roundtrip() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);

        let sid;
        let task;
        {
            let mut s = Session::create(&db, &clock, Some("demo")).unwrap();
            sid = s.id();
            task = s.create_task("build the thing").unwrap();
            s.transition_task(task, TaskState::Running).unwrap();
            s.append(EventPayload::MessageAppended {
                message: Message::user("hello"),
            })
            .unwrap();
        }

        // Recover from a cold load and verify the projection matches.
        let recovered = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(recovered.state().tasks().len(), 1);
        let t = recovered.state().task(task).unwrap();
        assert_eq!(t.state, TaskState::Running);
        assert_eq!(recovered.state().message_count, 1);
    }

    #[test]
    fn illegal_transition_is_rejected_before_persist() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let mut s = Session::create(&db, &clock, None).unwrap();
        let task = s.create_task("x").unwrap();
        s.transition_task(task, TaskState::Running).unwrap();
        s.transition_task(task, TaskState::Completed).unwrap();
        // Completed -> Running is illegal.
        let err = s.transition_task(task, TaskState::Running).unwrap_err();
        assert!(matches!(err, CoreError::IllegalTransition { .. }));
    }

    #[test]
    fn recover_unknown_session_errors() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        match Session::recover(&db, &clock, SessionId::new()) {
            Err(CoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {:?}", other.map(|_| "session")),
        }
    }
}
