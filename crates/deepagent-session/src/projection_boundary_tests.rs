use super::*;
use deepagent_core::clock::FixedClock;
use deepagent_core::message::{Message, ToolCall};

fn rich_message() -> Message {
    let mut message = Message::assistant("retained assistant text");
    message.reasoning_content = Some("retained reasoning".into());
    message.tool_calls = vec![
        ToolCall {
            id: "read-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path":"a.txt"}),
        },
        ToolCall {
            id: "patch-1".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"patch":"fixture patch"}),
        },
    ];
    message
}

#[test]
fn every_projection_cursor_survives_disk_reopen_fork_and_raw_rewind() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let clock = FixedClock::new(1000);
    let source;
    let anchor;
    let end;
    let expected;
    {
        let db = Database::open(&path).unwrap();
        let mut session = Session::create(&db, &clock, None).unwrap();
        source = session.id();
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("earlier"),
            })
            .unwrap();
        anchor = session
            .append(EventPayload::MessageAppended {
                message: rich_message(),
            })
            .unwrap()
            .sequence;
        expected = EventStore::new(&db).load_session(source).unwrap();
        end = expected.last().unwrap().sequence;
        assert_eq!(
            end,
            anchor + 4,
            "reasoning, function, custom, and text are one group"
        );
        session
            .append(EventPayload::MessageAppended {
                message: Message::user("discarded"),
            })
            .unwrap();
    }
    let db = Database::open(&path).unwrap();
    let store = EventStore::new(&db);
    for cursor in anchor..=end {
        assert_eq!(store.resolve_branch_sequence(source, cursor).unwrap(), end);
        let child = SessionId::new();
        store
            .fork_session(source, child, cursor, clock.now())
            .unwrap();
        let copied = store.load_session(child).unwrap();
        assert_eq!(
            copied
                .iter()
                .map(|event| &event.payload)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|event| &event.payload)
                .collect::<Vec<_>>()
        );
        // A second rewrite must retain the span, not flatten it into raw records.
        store
            .append(
                child,
                EventPayload::Note {
                    text: "discarded tail".into(),
                },
                clock.now(),
            )
            .unwrap();
        assert_eq!(store.truncate_after(child, cursor).unwrap(), 1);
        assert_eq!(
            store.load_session(child).unwrap().last().unwrap().sequence,
            end
        );
        let mut recovered = Session::recover(&db, &clock, child).unwrap();
        let next = recovered
            .append(EventPayload::MessageAppended {
                message: Message::user("next"),
            })
            .unwrap();
        assert_eq!(
            next.sequence,
            end + 1,
            "SQLite cursor must include all projection items"
        );
    }
    assert!(store.load_session(source).unwrap().last().unwrap().sequence > end);
}

#[test]
fn complete_legacy_single_event_groups_are_resolved_and_preserved() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1000);
    let mut source = Session::create(&db, &clock, None).unwrap();
    source
        .append(EventPayload::MessageAppended {
            message: Message::user("same repeated user"),
        })
        .unwrap();
    source
        .append(EventPayload::MessageAppended {
            message: Message::user("same repeated user"),
        })
        .unwrap();
    let anchor = source
        .append(EventPayload::MessageAppended {
            message: rich_message(),
        })
        .unwrap()
        .sequence;
    let store = EventStore::new(&db);
    let events = store.load_session(source.id()).unwrap();
    let legacy = Session::create(&db, &clock, None).unwrap().id();
    // Raw append uses exactly the old single Event record format, without spans.
    for event in events.iter().skip(1) {
        store
            .append(legacy, event.payload.clone(), event.timestamp)
            .unwrap();
    }
    let end = events.last().unwrap().sequence;
    assert_eq!(store.resolve_branch_sequence(legacy, anchor).unwrap(), end);
    Session::recover(&db, &clock, legacy).unwrap();
    let child = Session::fork(&db, &clock, legacy, anchor + 1).unwrap();
    assert_eq!(store.load_session(child.id()).unwrap().len(), events.len());
    assert_eq!(
        store
            .resolve_branch_sequence(child.id(), anchor + 2)
            .unwrap(),
        end
    );
}

#[test]
fn legacy_native_first_history_recovers_but_ambiguous_ownership_is_rejected() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1000);
    let id = Session::create(&db, &clock, None).unwrap().id();
    let store = EventStore::new(&db);
    // Old model-backed runs wrote native calls before compatibility requests.
    let call = ToolCall {
        id: "legacy-read".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path":"a.txt","offset":0}),
    };
    for payload in [
        EventPayload::ResponseItemAppended {
            item: ResponseItem::FunctionCall {
                call_id: call.id.clone(),
                name: call.name.clone(),
                arguments: r#"{ "path": "a.txt", "offset": 0 }"#.into(),
            },
        },
        EventPayload::ToolCallRequested { call },
        EventPayload::ResponseItemAppended {
            item: ResponseItem::Reasoning {
                id: Some("provider-reasoning-id".into()),
                content: "legacy reasoning".into(),
            },
        },
        EventPayload::ResponseItemAppended {
            item: ResponseItem::Message {
                role: "assistant".into(),
                content: "legacy answer".into(),
            },
        },
        EventPayload::MessageAppended {
            message: Message::assistant("legacy answer").with_reasoning("legacy reasoning"),
        },
    ] {
        store.append(id, payload, clock.now()).unwrap();
    }
    Session::recover(&db, &clock, id).unwrap();
    let child = Session::fork(&db, &clock, id, 5).unwrap();
    assert_eq!(store.load_session(child.id()).unwrap().len(), 6);

    let ambiguous = Session::create(&db, &clock, None).unwrap().id();
    for payload in [
        EventPayload::ResponseItemAppended {
            item: ResponseItem::Message {
                role: "user".into(),
                content: "same".into(),
            },
        },
        EventPayload::MessageAppended {
            message: Message::user("same"),
        },
        EventPayload::ResponseItemAppended {
            item: ResponseItem::Message {
                role: "user".into(),
                content: "same".into(),
            },
        },
    ] {
        store.append(ambiguous, payload, clock.now()).unwrap();
    }
    let before = store.load_session(ambiguous).unwrap();
    let error = store.load_session_for_model(ambiguous).unwrap_err();
    assert!(error
        .to_string()
        .contains("ambiguous legacy source/native ownership"));
    assert!(store.truncate_after(ambiguous, 2).is_err());
    assert_eq!(store.load_session(ambiguous).unwrap(), before);
}

#[test]
fn incomplete_legacy_projection_fails_without_mutating_raw_history() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1000);
    let mut session = Session::create(&db, &clock, None).unwrap();
    session
        .append(EventPayload::MessageAppended {
            message: Message::user("same repeated user"),
        })
        .unwrap();
    let store = EventStore::new(&db);
    let orphan = store
        .append(
            session.id(),
            EventPayload::MessageAppended {
                message: Message::user("same repeated user"),
            },
            clock.now(),
        )
        .unwrap();
    let before = store.load_session(session.id()).unwrap();
    assert!(Session::recover(&db, &clock, session.id()).is_err());
    assert!(store
        .resolve_branch_sequence(session.id(), orphan.sequence)
        .is_err());
    assert!(store.truncate_after(session.id(), orphan.sequence).is_err());
    let child = SessionId::new();
    assert!(store
        .fork_session(session.id(), child, orphan.sequence, clock.now())
        .is_err());
    assert!(store.get_session(child).unwrap().is_none());
    assert_eq!(store.load_session(session.id()).unwrap(), before);
    // A valid earlier prefix must remain a recovery option even if a discarded
    // tail is ambiguous. Validation must follow the requested retention scope.
    store.truncate_after(session.id(), 0).unwrap();
    Session::recover(&db, &clock, session.id()).unwrap();

    let partial = Session::create(&db, &clock, None).unwrap().id();
    let anchor = store
        .append(
            partial,
            EventPayload::MessageAppended {
                message: rich_message(),
            },
            clock.now(),
        )
        .unwrap();
    store
        .append(
            partial,
            EventPayload::ResponseItemAppended {
                item: ResponseItem::Reasoning {
                    id: None,
                    content: "retained reasoning".into(),
                },
            },
            clock.now(),
        )
        .unwrap();
    assert!(Session::recover(&db, &clock, partial).is_err());
    assert!(store
        .resolve_branch_sequence(partial, anchor.sequence)
        .is_err());
}

#[test]
fn native_only_decision_and_custom_result_survive_recovery_and_branching() {
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1000);
    let mut session = Session::create(&db, &clock, None).unwrap();
    session
        .append_response_item(ResponseItem::Message {
            role: "assistant".into(),
            content: "provider text".into(),
        })
        .unwrap();
    let source = session
        .append_without_response_projection(EventPayload::MessageAppended {
            message: Message::assistant("UI text changed by completion hook"),
        })
        .unwrap();
    Session::recover(&db, &clock, session.id()).unwrap();
    let store = EventStore::new(&db);
    assert_eq!(
        store
            .resolve_branch_sequence(session.id(), source.sequence)
            .unwrap(),
        source.sequence
    );
    session
        .append_response_item(ResponseItem::CustomToolCall {
            call_id: "patch-1".into(),
            name: "apply_patch".into(),
            input: "fixture patch".into(),
        })
        .unwrap();
    let result = session
        .append(EventPayload::ToolCallCompleted {
            call_id: "patch-1".into(),
            ok: true,
            output: serde_json::json!({"changed":true}),
            duration_ms: 1,
        })
        .unwrap();
    let child = Session::fork(&db, &clock, session.id(), result.sequence).unwrap();
    let events = store.load_session(child.id()).unwrap();
    assert_eq!(events.last().unwrap().sequence, result.sequence + 1);
    assert!(
        matches!(&events.last().unwrap().payload, EventPayload::ResponseItemAppended { item: ResponseItem::CustomToolCallOutput { call_id, .. } } if call_id == "patch-1")
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.payload,
                EventPayload::ResponseItemAppended {
                    item: ResponseItem::Message { .. }
                }
            ))
            .count(),
        1
    );
}

#[test]
fn concurrent_session_handles_commit_whole_groups_without_interleaving() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    let clock = FixedClock::new(1000);
    let db = Database::open(&path).unwrap();
    let id = Session::create(&db, &clock, None).unwrap().id();
    std::thread::scope(|scope| {
        for _ in 0..2 {
            let path = &path;
            let clock = &clock;
            scope.spawn(move || {
                let db = Database::open(path).unwrap();
                for _ in 0..8 {
                    EventStore::new(&db)
                        .append_with_response_projection(
                            id,
                            EventPayload::MessageAppended {
                                message: rich_message(),
                            },
                            true,
                            clock.now(),
                        )
                        .unwrap();
                }
            });
        }
    });
    let store = EventStore::new(&db);
    let events = store.load_session_for_model(id).unwrap();
    assert_eq!(events.len(), 1 + 16 * 5);
    for group in events[1..].chunks_exact(5) {
        assert!(matches!(
            group[0].payload,
            EventPayload::MessageAppended { .. }
        ));
        assert!(group[1..]
            .iter()
            .all(|event| matches!(event.payload, EventPayload::ResponseItemAppended { .. })));
        assert_eq!(
            store
                .resolve_branch_sequence(id, group[0].sequence)
                .unwrap(),
            group[4].sequence
        );
    }
}
