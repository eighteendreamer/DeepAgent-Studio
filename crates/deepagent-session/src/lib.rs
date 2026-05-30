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
    /// Start a brand new session in [`SessionMode::Normal`].
    pub fn create(db: &'db Database, clock: &'db C, title: Option<&str>) -> Result<Self> {
        Self::create_with_mode(db, clock, title, deepagent_core::SessionMode::Normal)
    }

    /// Start a brand new session with an explicit run mode, writing the initial
    /// `SessionStarted` event (run mode is durable session metadata).
    pub fn create_with_mode(
        db: &'db Database,
        clock: &'db C,
        title: Option<&str>,
        mode: deepagent_core::SessionMode,
    ) -> Result<Self> {
        Self::create_in_project(db, clock, title, mode, None)
    }

    /// Start a brand new session bound to a `project` (a folder path). The
    /// project is durable session metadata used to group sessions in the UI and
    /// to root the agent's file operations. Pass `None` for an unscoped session.
    pub fn create_in_project(
        db: &'db Database,
        clock: &'db C,
        title: Option<&str>,
        mode: deepagent_core::SessionMode,
        project: Option<&str>,
    ) -> Result<Self> {
        let id = SessionId::new();
        let now = clock.now();
        let store = EventStore::new(db);
        store.create_session_full(id, title, mode, project, now)?;
        let started = EventPayload::SessionStarted {
            title: title.map(|s| s.to_string()),
            mode,
        };
        store.append(id, started.clone(), now)?;

        let mut state = SessionState::new(id);
        // Fold the one event we just wrote.
        state.apply(&started);

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

    /// **Fork** this session at `at_seq`: create a new sibling session whose
    /// stream is a copy of events `0..=at_seq` from `source_id`. The source is
    /// left untouched, so forking is fully non-destructive (it branches the
    /// timeline). Returns the new, recovered [`Session`].
    ///
    /// Forking lets the user explore an alternative continuation from any point
    /// in the timeline without losing the original transcript.
    pub fn fork(
        db: &'db Database,
        clock: &'db C,
        source_id: SessionId,
        at_seq: deepagent_core::event::Sequence,
    ) -> Result<Self> {
        let store = EventStore::new(db);
        if store.get_session(source_id)?.is_none() {
            return Err(CoreError::not_found(format!("session {source_id}")));
        }
        let new_id = SessionId::new();
        let now = clock.now();
        store.fork_session(source_id, new_id, at_seq, now)?;
        tracing::info!(%source_id, %new_id, at_seq, "forked session");
        Self::recover(db, clock, new_id)
    }

    /// **Rewind** this session in place to `to_seq`: discard every event after
    /// `to_seq` and reload the projection. This is destructive (the discarded
    /// tail is gone) and is only reachable through an explicit user action.
    ///
    /// After rewinding, the session is "reopened" (any prior end is cleared)
    /// and new events append gaplessly from the kept tail. Returns the number
    /// of events discarded.
    pub fn rewind(&mut self, to_seq: deepagent_core::event::Sequence) -> Result<u64> {
        let store = EventStore::new(self.db);
        let removed = store.truncate_after(self.id, to_seq)?;
        // Rebuild the in-memory projection from the truncated stream.
        let events = store.load_session(self.id)?;
        self.state = SessionState::replay(self.id, events.iter().map(|e| &e.payload));
        tracing::info!(id = %self.id, to_seq, removed, "rewound session");
        Ok(removed)
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

    #[test]
    fn session_mode_persists_and_recovers() {
        use deepagent_core::SessionMode;

        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let sid;
        {
            let s = Session::create_with_mode(
                &db,
                &clock,
                Some("coordinated"),
                SessionMode::Coordinator,
            )
            .unwrap();
            sid = s.id();
            // Mode is reflected in the live projection.
            assert_eq!(s.state().mode, SessionMode::Coordinator);
        }
        // Mode survives a cold recover (from both the row and the event).
        let recovered = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(recovered.state().mode, SessionMode::Coordinator);
        // And the persisted session record carries it too.
        assert_eq!(recovered.record().unwrap().mode, SessionMode::Coordinator);
    }

    #[test]
    fn default_create_is_normal_mode() {
        use deepagent_core::SessionMode;
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let s = Session::create(&db, &clock, None).unwrap();
        assert_eq!(s.state().mode, SessionMode::Normal);
    }

    #[test]
    fn fork_branches_without_touching_source() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);

        let sid;
        {
            let mut s = Session::create(&db, &clock, Some("origin")).unwrap();
            sid = s.id();
            s.append(EventPayload::MessageAppended {
                message: Message::user("first"),
            })
            .unwrap();
            s.append(EventPayload::MessageAppended {
                message: Message::user("second"),
            })
            .unwrap();
        }
        // Stream is: 0=SessionStarted, 1=first, 2=second.
        // Fork at seq 1 → branch keeps SessionStarted + "first" only.
        let mut forked = Session::fork(&db, &clock, sid, 1).unwrap();
        assert_ne!(forked.id(), sid);
        assert_eq!(forked.state().message_count, 1);
        assert_eq!(forked.state().title.as_deref(), Some("origin"));

        // Branch can diverge.
        forked
            .append(EventPayload::MessageAppended {
                message: Message::user("branch-only"),
            })
            .unwrap();
        assert_eq!(forked.state().message_count, 2);

        // Source is unchanged (still 2 messages).
        let source = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(source.state().message_count, 2);
    }

    #[test]
    fn rewind_discards_tail_in_place() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);

        let mut s = Session::create(&db, &clock, Some("rw")).unwrap();
        let sid = s.id();
        for i in 0..4 {
            s.append(EventPayload::MessageAppended {
                message: Message::user(format!("m{i}")),
            })
            .unwrap();
        }
        s.end(Some("done")).unwrap();
        assert!(s.state().ended);
        assert_eq!(s.state().message_count, 4);

        // Rewind to seq 2 (SessionStarted=0, m0=1, m1=2). Keep 3 events.
        // Stream was: 0=Started,1=m0,2=m1,3=m2,4=m3,5=Ended → remove 3,4,5.
        let removed = s.rewind(2).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(s.state().message_count, 2);
        assert!(!s.state().ended);

        // Recover confirms the truncation is durable.
        let recovered = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(recovered.state().message_count, 2);
        assert!(!recovered.state().ended);
    }

    #[test]
    fn fork_unknown_session_errors() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        match Session::fork(&db, &clock, SessionId::new(), 0) {
            Err(CoreError::NotFound(_)) => {}
            other => panic!("expected NotFound, got {:?}", other.map(|_| "session")),
        }
    }
}
