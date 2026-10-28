//! The single durable plan-mode projection over the existing session source.

use super::*;

pub(super) const VERSION: u32 = 1;

/// Provenance of the mode observed at a run/tool admission boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPlanMode {
    pub active: bool,
    pub source_sequence: Option<Sequence>,
    pub history_generation: u64,
}

fn project(loaded: &session_files::LoadedSessionFile) -> Result<Option<SessionPlanMode>> {
    if loaded
        .header
        .plan_mode_version
        .is_some_and(|version| version != VERSION)
    {
        return Err(CoreError::invalid(
            "unsupported persisted plan mode header version",
        ));
    }
    let mut state = loaded.header.plan_mode_version.map(|_| SessionPlanMode {
        active: false,
        source_sequence: None,
        history_generation: loaded.generation,
    });
    for event in &loaded.events {
        if let EventPayload::PlanModeChanged { version, active } = event.payload {
            if version != VERSION {
                return Err(CoreError::invalid(
                    "unsupported persisted plan mode event version",
                ));
            }
            state = Some(SessionPlanMode {
                active,
                source_sequence: Some(event.sequence),
                history_generation: loaded.generation,
            });
        }
    }
    Ok(state)
}

impl EventStore<'_> {
    /// Replay the authoritative source, including changes made by another
    /// handle or by fork/rewind. A missing legacy state is not an implicit exit.
    pub fn session_plan_mode(&self, id: SessionId) -> Result<SessionPlanMode> {
        let loaded = self
            .load_session_file(id)?
            .ok_or_else(|| CoreError::not_found(format!("session {id}")))?;
        project(&loaded)?.ok_or_else(|| CoreError::invalid(
            "Plan mode state is unknown for this legacy session; explicitly select /plan or /execute before continuing"
        ))
    }

    /// Acknowledge a selection only after the original append commit succeeds.
    /// Keep validation and append under the existing activity/I/O/file locks;
    /// unknown legacy state may be selected, unsupported versions may not.
    pub fn set_session_plan_mode(
        &self,
        id: SessionId,
        active: bool,
        now: Timestamp,
    ) -> Result<Event> {
        let _activity = self.acquire_activity(id)?;
        let _io = self.db.lock_session_io()?;
        let _file_lock = session_files::lock_session(&self.db.session_files_root(), id)?;
        let loaded = session_files::load(&self.db.session_files_root(), id)?
            .ok_or_else(|| CoreError::not_found(format!("session {id}")))?;
        verify_contiguous(&loaded.events, id, 0)?;
        project(&loaded)?;
        self.append_locked(
            id,
            EventPayload::PlanModeChanged {
                version: VERSION,
                active,
            },
            now,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(store: &EventStore<'_>) -> SessionId {
        let id = SessionId::new();
        store
            .create_session(id, None, Timestamp::from_millis(1))
            .unwrap();
        store
            .append(
                id,
                EventPayload::SessionStarted {
                    title: None,
                    mode: SessionMode::Normal,
                },
                Timestamp::from_millis(1),
            )
            .unwrap();
        id
    }

    fn old_header(db: &Database, version: Option<u32>) -> SessionId {
        let id = SessionId::new();
        session_files::create(
            &db.session_files_root(),
            SessionFileHeader {
                id,
                title: None,
                mode: SessionMode::Normal,
                project: None,
                created_at: Timestamp::from_millis(1),
                execution_config: None,
                plan_mode_version: version,
            },
        )
        .unwrap();
        EventStore::new(db)
            .recover_orphaned_session_files()
            .unwrap();
        id
    }

    #[test]
    fn plan_mode_new_default_and_versioned_receipts() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let id = create(&store);
        let initial = store.session_plan_mode(id).unwrap();
        assert!(!initial.active);
        assert_eq!(initial.source_sequence, None);
        for active in [true, false, true] {
            let event = store
                .set_session_plan_mode(id, active, Timestamp::from_millis(2))
                .unwrap();
            assert_eq!(event.kind(), "plan_mode_changed");
            assert_eq!(
                event.payload,
                EventPayload::PlanModeChanged {
                    version: VERSION,
                    active
                }
            );
            let state = store.session_plan_mode(id).unwrap();
            assert_eq!(state.active, active);
            assert_eq!(state.source_sequence, Some(event.sequence));
        }
        assert!(store.session_plan_mode(SessionId::new()).is_err());
    }

    #[test]
    fn plan_mode_reopens_and_observes_another_database_handle() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic.sqlite");
        let id = {
            let db = Database::open(&path).unwrap();
            let store = EventStore::new(&db);
            let id = create(&store);
            store
                .set_session_plan_mode(id, true, Timestamp::from_millis(2))
                .unwrap();
            id
        };
        let first = Database::open(&path).unwrap();
        let second = Database::open(&path).unwrap();
        assert!(
            EventStore::new(&first)
                .session_plan_mode(id)
                .unwrap()
                .active
        );
        EventStore::new(&second)
            .set_session_plan_mode(id, false, Timestamp::from_millis(3))
            .unwrap();
        assert!(
            !EventStore::new(&first)
                .session_plan_mode(id)
                .unwrap()
                .active
        );
        let other = create(&EventStore::new(&first));
        EventStore::new(&first)
            .set_session_plan_mode(other, true, Timestamp::from_millis(4))
            .unwrap();
        assert!(
            !EventStore::new(&second)
                .session_plan_mode(id)
                .unwrap()
                .active
        );
        assert!(
            EventStore::new(&second)
                .session_plan_mode(other)
                .unwrap()
                .active
        );
    }

    #[test]
    fn plan_mode_fork_rewind_and_compaction_use_the_preserved_prefix() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let id = create(&store);
        let enter = store
            .set_session_plan_mode(id, true, Timestamp::from_millis(2))
            .unwrap();
        store
            .append(
                id,
                EventPayload::ContextCompacted {
                    tokens_before: 100,
                    tokens_after: 10,
                    strategy: "manual".into(),
                    summary: Some("synthetic summary without plan wording".into()),
                    first_kept_sequence: None,
                },
                Timestamp::from_millis(3),
            )
            .unwrap();
        assert!(store.session_plan_mode(id).unwrap().active);
        store
            .set_session_plan_mode(id, false, Timestamp::from_millis(4))
            .unwrap();
        let child = SessionId::new();
        store
            .fork_session(id, child, enter.sequence, Timestamp::from_millis(5))
            .unwrap();
        assert!(store.session_plan_mode(child).unwrap().active);
        assert!(!store.session_plan_mode(id).unwrap().active);
        let old_generation = store.session_plan_mode(id).unwrap().history_generation;
        store.truncate_after(id, enter.sequence).unwrap();
        let restored = store.session_plan_mode(id).unwrap();
        assert!(restored.active);
        assert!(restored.history_generation > old_generation);
        assert_eq!(restored.source_sequence, Some(enter.sequence));
        store.truncate_after(id, 0).unwrap();
        assert!(!store.session_plan_mode(id).unwrap().active);
        assert!(store.session_plan_mode(child).unwrap().active);
    }

    #[test]
    fn plan_mode_legacy_unknown_requires_selection_not_transcript_inference() {
        let db = Database::open_in_memory().unwrap();
        let id = old_header(&db, None);
        let store = EventStore::new(&db);
        store
            .append(
                id,
                EventPayload::MessageAppended {
                    message: deepagent_core::message::Message::assistant("Entered Plan mode"),
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        assert!(store
            .session_plan_mode(id)
            .unwrap_err()
            .to_string()
            .contains("explicitly select"));
        let chosen = store
            .set_session_plan_mode(id, true, Timestamp::from_millis(3))
            .unwrap();
        assert!(store.session_plan_mode(id).unwrap().active);
        let child = SessionId::new();
        store
            .fork_session(id, child, chosen.sequence, Timestamp::from_millis(4))
            .unwrap();
        assert!(store.session_plan_mode(child).unwrap().active);
        store.truncate_after(id, 0).unwrap();
        assert!(store.session_plan_mode(id).is_err());
        store
            .set_session_plan_mode(id, false, Timestamp::from_millis(5))
            .unwrap();
        assert!(!store.session_plan_mode(id).unwrap().active);
    }

    #[test]
    fn plan_mode_future_versions_cannot_be_hidden_by_an_explicit_selection() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let future = old_header(&db, Some(VERSION + 1));
        assert!(store.session_plan_mode(future).is_err());
        assert!(store
            .set_session_plan_mode(future, false, Timestamp::from_millis(2))
            .is_err());
        assert!(store.load_session(future).unwrap().is_empty());
        let id = create(&store);
        store
            .append(
                id,
                EventPayload::PlanModeChanged {
                    version: VERSION + 1,
                    active: true,
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        assert!(store.session_plan_mode(id).is_err());
        assert!(store
            .set_session_plan_mode(id, false, Timestamp::from_millis(3))
            .is_err());
        assert_eq!(store.load_session(id).unwrap().len(), 2);
        assert!(serde_json::from_value::<EventPayload>(serde_json::json!({
            "type":"plan_mode_changed","version":1,"active":"false"
        }))
        .is_err());
    }

    #[test]
    fn plan_mode_failed_history_control_admission_does_not_confirm_exit() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let id = create(&store);
        let enter = store
            .set_session_plan_mode(id, true, Timestamp::from_millis(2))
            .unwrap();
        let control = store.begin_control(id).unwrap();
        assert!(store
            .set_session_plan_mode(id, false, Timestamp::from_millis(3))
            .is_err());
        drop(control);
        let state = store.session_plan_mode(id).unwrap();
        assert!(state.active);
        assert_eq!(state.source_sequence, Some(enter.sequence));
        assert_eq!(store.load_session(id).unwrap().len(), 2);
    }

    #[test]
    fn plan_mode_concurrent_handles_keep_gapless_last_commit_wins() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("synthetic.sqlite");
        let db = Database::open(&path).unwrap();
        let store = EventStore::new(&db);
        let id = create(&store);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let connections = (0..4)
            .map(|_| Database::open(&path).unwrap())
            .collect::<Vec<_>>();
        let workers = connections
            .into_iter()
            .enumerate()
            .map(|(worker, db)| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    (0..4)
                        .map(|turn| {
                            let active = (worker + turn) % 2 == 0;
                            let event = EventStore::new(&db)
                                .set_session_plan_mode(id, active, Timestamp::from_millis(2))
                                .unwrap();
                            (event.sequence, active)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect::<Vec<_>>();
        let receipts = workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        let (sequence, active) = receipts
            .into_iter()
            .max_by_key(|(sequence, _)| *sequence)
            .unwrap();
        let state = store.session_plan_mode(id).unwrap();
        assert_eq!(state.source_sequence, Some(sequence));
        assert_eq!(state.active, active);
        let events = store.load_session(id).unwrap();
        assert_eq!(events.len(), 17);
        assert!(events
            .iter()
            .enumerate()
            .all(|(sequence, event)| event.sequence == sequence as u64));
    }
}
