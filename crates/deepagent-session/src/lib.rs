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
use deepagent_core::response_item::ResponseItem;
use deepagent_persistence::event_store::{EventStore, SessionActivityGuard, SessionRecord};
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
        Self::create_configured(db, clock, title, mode, project, None)
    }

    /// Create a session with versioned explicit execution overrides in the
    /// same durable metadata boundary. Existing callers retain host defaults.
    pub fn create_configured(
        db: &'db Database,
        clock: &'db C,
        title: Option<&str>,
        mode: deepagent_core::SessionMode,
        project: Option<&str>,
        execution_config: Option<deepagent_core::execution_config::SessionExecutionConfig>,
    ) -> Result<Self> {
        let id = SessionId::new();
        let now = clock.now();
        let store = EventStore::new(db);
        store.create_session_configured(id, title, mode, project, execution_config, now)?;
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
        let events = store.load_session_for_model(id)?;
        if events.is_empty() {
            return Err(CoreError::not_found(format!("session {id}")));
        }
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
    /// A cursor within a source/native projection keeps the complete group.
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
    /// A cursor within a source/native projection keeps the complete group.
    ///
    /// Only open sessions can rewind. Ended/ending sessions remain read-only;
    /// use a fork for a fresh lifecycle. New events append gaplessly from the
    /// kept tail. Returns the number of events discarded.
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

    /// Protect a direct runtime invocation against concurrent rewind. AppCore
    /// holds the same shared gate from admission; embedders hold this guard
    /// through their final session write, not just through a model request.
    pub fn acquire_activity(&self) -> Result<SessionActivityGuard> {
        EventStore::new(self.db).acquire_activity(self.id)
    }

    /// Directory holding this session's authoritative event log.
    ///
    /// A compaction summary is a lossy rewrite, and this is the only handle the
    /// model gets on the detail the summary dropped, so the session that owns the
    /// log names its own location rather than having callers rebuild the layout.
    pub fn transcript_dir(&self) -> std::path::PathBuf {
        self.db.session_transcript_dir(self.id)
    }

    /// The current in-memory projection.
    pub fn state(&self) -> &SessionState {
        &self.state
    }

    /// Append a payload to the stream and update the projection. Returns the
    /// persisted [`Event`].
    pub fn append(&mut self, payload: EventPayload) -> Result<Event> {
        self.append_with_response_projection(payload, true)
    }

    /// Append a payload without deriving synthetic Responses items.
    ///
    /// Model-backed runtime turns call this after they have already persisted
    /// provider-native `response.output` items. Keeping this path explicit
    /// prevents duplicate model-history items while preserving the legacy
    /// `append` behavior for tests and non-model embedders.
    pub fn append_without_response_projection(&mut self, payload: EventPayload) -> Result<Event> {
        self.append_with_response_projection(payload, false)
    }

    /// Persist one provider-native Responses item exactly as returned by the
    /// model stream.
    pub fn append_response_item(&mut self, item: ResponseItem) -> Result<Event> {
        self.append_with_response_projection(EventPayload::ResponseItemAppended { item }, false)
    }

    /// A host-generated system context item is a complete commit group. It has
    /// no later model-message mirror; mark that explicitly using the existing
    /// projection frame so bounded readers never attach subsequent run events.
    pub fn append_host_context(&mut self, item: ResponseItem) -> Result<Event> {
        if !matches!(&item, ResponseItem::Message { role, .. } if role == "system") {
            return Err(CoreError::invalid(
                "host context requires a system message item",
            ));
        }
        let events = EventStore::new(self.db).append_with_response_projection(
            self.id,
            EventPayload::ResponseItemAppended { item },
            false,
            self.clock.now(),
        )?;
        for event in &events {
            self.state.apply(&event.payload);
        }
        Ok(events.into_iter().next().expect("committed host context"))
    }

    /// Record a completion with the executing host's authorized tool catalog.
    /// Source and native result remain one ordinary atomic projection frame.
    pub fn append_tool_completion(
        &mut self,
        payload: EventPayload,
        schema: &dyn Fn(&str) -> Option<serde_json::Value>,
    ) -> Result<Event> {
        if !matches!(payload, EventPayload::ToolCallCompleted { .. }) {
            return Err(CoreError::invalid("expected a tool completion"));
        }
        let events = EventStore::new(self.db).append_with_tool_schema_projection(
            self.id,
            payload,
            true,
            self.clock.now(),
            schema,
        )?;
        for event in &events {
            self.state.apply(&event.payload);
        }
        Ok(events
            .into_iter()
            .next()
            .expect("committed completion has a source"))
    }

    /// Preserve source input and its admitted hook context in one native group.
    pub fn append_user_prompt(
        &mut self,
        message: deepagent_core::message::Message,
        context: Option<ResponseItem>,
    ) -> Result<Event> {
        let Some(context) = context else {
            return self.append(EventPayload::MessageAppended { message });
        };
        let events = EventStore::new(self.db).append_user_prompt_with_context(
            self.id,
            message,
            context,
            self.clock.now(),
        )?;
        for event in &events {
            self.state.apply(&event.payload);
        }
        Ok(events
            .into_iter()
            .next()
            .expect("committed input has a source"))
    }

    fn append_with_response_projection(
        &mut self,
        payload: EventPayload,
        project_response_items: bool,
    ) -> Result<Event> {
        let now = self.clock.now();
        let store = EventStore::new(self.db);
        let events = if matches!(
            payload,
            EventPayload::MessageAppended { .. }
                | EventPayload::ToolCallRequested { .. }
                | EventPayload::ToolCallCompleted { .. }
        ) {
            store.append_with_response_projection(self.id, payload, project_response_items, now)?
        } else {
            vec![store.append(self.id, payload, now)?]
        };
        for event in &events {
            self.state.apply(&event.payload);
        }
        Ok(events
            .into_iter()
            .next()
            .expect("a committed group always has a source"))
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
        EventStore::new(self.db).end_session(self.id, reason, self.clock.now())?;
        self.state.apply(&EventPayload::SessionEnded {
            reason: reason.map(|s| s.to_string()),
        });
        Ok(())
    }

    /// Check current durable lifecycle before a runtime starts, not just this
    /// in-memory projection (another owner may have ended it since recovery).
    pub fn require_open(&self) -> Result<()> {
        if self.record()?.ended_at.is_some() {
            return Err(CoreError::invalid(
                "session has ended; create or fork a session to continue",
            ));
        }
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
mod projection_boundary_tests;

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
    fn recover_fileless_legacy_row_errors() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1);
        let id = SessionId::new();
        db.with_conn(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions
                     (id, title, mode, project, created_at, updated_at, ended_at)
                     VALUES (?1, 'legacy', 'normal', NULL, 1, 1, NULL)",
                    [id.to_string()],
                )
                .map_err(|error| {
                    deepagent_core::error::CoreError::Persistence(error.to_string())
                })?;
            Ok(())
        })
        .unwrap();

        assert!(matches!(
            Session::recover(&db, &clock, id),
            Err(CoreError::NotFound(_))
        ));
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
        assert!(!s.state().ended);
        assert_eq!(s.state().message_count, 4);

        // Responses item events follow each message. Keep through m1's item.
        // Stream is: 0=Started,1=m0,2=item,3=m1,4=item,5=m2,6=item,7=m3,
        // 8=item; truncating at 4 removes the final four events.
        let removed = s.rewind(4).unwrap();
        assert_eq!(removed, 4);
        assert_eq!(s.state().message_count, 2);
        assert!(!s.state().ended);

        // Recover confirms the truncation is durable.
        let recovered = Session::recover(&db, &clock, sid).unwrap();
        assert_eq!(recovered.state().message_count, 2);
        assert!(!recovered.state().ended);
    }

    #[test]
    fn explicitly_ended_session_cannot_be_reopened_by_rewind() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, None).unwrap();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("keep this history"),
            })
            .unwrap();
        session.end(Some("explicit end")).unwrap();
        session.end(Some("repeat end")).unwrap();
        let before = EventStore::new(&db).load_session(session.id()).unwrap();
        assert_eq!(
            before
                .iter()
                .filter(|event| matches!(event.payload, EventPayload::SessionEnded { .. }))
                .count(),
            1
        );
        assert!(session.rewind(0).unwrap_err().to_string().contains("ended"));
        assert_eq!(
            EventStore::new(&db).load_session(session.id()).unwrap(),
            before
        );
        assert!(
            Session::recover(&db, &clock, session.id())
                .unwrap()
                .state()
                .ended
        );
    }

    #[test]
    fn appending_tool_trajectory_persists_responses_items() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("items")).unwrap();
        session
            .append(EventPayload::ToolCallRequested {
                call: deepagent_core::message::ToolCall {
                    id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path":"a.txt"}),
                },
            })
            .unwrap();
        session
            .append(EventPayload::ToolCallCompleted {
                content: Vec::new(),
                call_id: "call-1".into(),
                ok: true,
                output: serde_json::json!({"content":"ok"}),
                duration_ms: 1,
            })
            .unwrap();
        let items: Vec<_> = EventStore::new(&db)
            .load_session(session.id())
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::ResponseItemAppended { item } => Some(item),
                _ => None,
            })
            .collect();
        assert!(matches!(
            &items[0],
            ResponseItem::FunctionCall {
                call_id,
                arguments,
                ..
            } if call_id == "call-1" && arguments == r#"{"path":"a.txt"}"#
        ));
        assert!(matches!(
            &items[1],
            ResponseItem::FunctionCallOutput { call_id, .. } if call_id == "call-1"
        ));
    }

    #[test]
    fn native_custom_tool_completion_keeps_custom_pairing() {
        let db = Database::open_in_memory().unwrap();
        let clock = FixedClock::new(1_000);
        let mut session = Session::create(&db, &clock, Some("custom items")).unwrap();
        session
            .append_response_item(ResponseItem::CustomToolCall {
                call_id: "patch-1".into(),
                name: "apply_patch".into(),
                input: "*** Begin Patch".into(),
            })
            .unwrap();
        session
            .append(EventPayload::ToolCallCompleted {
                content: Vec::new(),
                call_id: "patch-1".into(),
                ok: false,
                output: serde_json::json!({"cancelled": true}),
                duration_ms: 0,
            })
            .unwrap();

        let items: Vec<_> = EventStore::new(&db)
            .load_session(session.id())
            .unwrap()
            .into_iter()
            .filter_map(|event| match event.payload {
                EventPayload::ResponseItemAppended { item } => Some(item),
                _ => None,
            })
            .collect();
        assert!(matches!(
            &items[1],
            ResponseItem::CustomToolCallOutput { call_id, .. } if call_id == "patch-1"
        ));
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
