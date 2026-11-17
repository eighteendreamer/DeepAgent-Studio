use super::*;
use deepagent_core::clock::FixedClock;
use deepagent_core::message::{Message, ToolCall};

#[test]
fn h156_new_function_and_native_custom_keep_identity_across_disk_reopen_and_fork() {
    for native in [false, true] {
        for custom in [false, true] {
            if custom && !native {
                continue; // Custom requires an explicit native call, never just a reserved name.
            }
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("identity.sqlite");
            let clock = FixedClock::new(1);
            let (id, end, expected) = {
                let db = Database::open(&path).unwrap();
                let mut session = Session::create(&db, &clock, None).unwrap();
                let call = ToolCall {
                    id: "same-name".into(),
                    name: "apply_patch".into(),
                    arguments: if custom {
                        serde_json::json!({"patch":"synthetic patch"})
                    } else {
                        serde_json::json!({"value":7})
                    },
                };
                if native {
                    session
                        .append_response_item(if custom {
                            ResponseItem::CustomToolCall {
                                call_id: call.id.clone(),
                                name: call.name.clone(),
                                input: "synthetic patch".into(),
                            }
                        } else {
                            ResponseItem::FunctionCall {
                                call_id: call.id.clone(),
                                name: call.name.clone(),
                                arguments: call.arguments.to_string(),
                            }
                        })
                        .unwrap();
                    session
                        .append_without_response_projection(EventPayload::ToolCallRequested {
                            call,
                        })
                        .unwrap();
                } else {
                    session
                        .append(EventPayload::ToolCallRequested { call })
                        .unwrap();
                }
                let completion = session
                    .append(EventPayload::ToolCallCompleted {
                        call_id: "same-name".into(),
                        ok: true,
                        output: serde_json::json!({"value":8}),
                        content: Vec::new(),
                        duration_ms: 1,
                    })
                    .unwrap();
                let events = EventStore::new(&db).load_session(session.id()).unwrap();
                let items: Vec<_> = events
                    .iter()
                    .filter_map(|event| match &event.payload {
                        EventPayload::ResponseItemAppended { item } => Some(item),
                        _ => None,
                    })
                    .collect();
                assert_eq!(items.len(), 2);
                assert_eq!(
                    matches!(items[0], ResponseItem::CustomToolCall { .. }),
                    custom
                );
                assert_eq!(
                    matches!(items[1], ResponseItem::CustomToolCallOutput { .. }),
                    custom
                );
                assert_eq!(
                    matches!(items[1], ResponseItem::FunctionCallOutput { .. }),
                    !custom
                );
                (session.id(), completion.sequence, events)
            };
            let db = Database::open(&path).unwrap();
            Session::recover(&db, &clock, id).unwrap();
            let store = EventStore::new(&db);
            assert_eq!(store.load_session(id).unwrap(), expected);
            let child = Session::fork(&db, &clock, id, end).unwrap();
            assert_eq!(
                store
                    .load_session(child.id())
                    .unwrap()
                    .iter()
                    .map(|event| &event.payload)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|event| &event.payload)
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn h156_legacy_source_first_and_native_first_records_keep_the_recorded_type() {
    use deepagent_persistence::event_store::SessionPageBudget;
    for custom in [false, true] {
        for source_first in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("legacy.sqlite");
            let clock = FixedClock::new(1);
            let (id, expected) = {
                let db = Database::open(&path).unwrap();
                let id = Session::create(&db, &clock, None).unwrap().id();
                let store = EventStore::new(&db);
                let call = ToolCall {
                    id: "same-name".into(),
                    name: "apply_patch".into(),
                    arguments: serde_json::json!({"patch":"synthetic patch"}),
                };
                let native = if custom {
                    ResponseItem::from_legacy_tool_call(&call)
                } else {
                    ResponseItem::from_function_call(&call)
                };
                let source = EventPayload::MessageAppended {
                    message: Message::assistant("").with_tool_calls(vec![call]),
                };
                let native = EventPayload::ResponseItemAppended { item: native };
                let payloads = if source_first {
                    vec![source, native]
                } else {
                    vec![native, source]
                };
                for payload in payloads {
                    store.append(id, payload, clock.now()).unwrap();
                }
                store
                    .append(
                        id,
                        EventPayload::ToolCallCompleted {
                            call_id: "same-name".into(),
                            ok: true,
                            output: serde_json::json!({"value":8}),
                            content: Vec::new(),
                            duration_ms: 1,
                        },
                        clock.now(),
                    )
                    .unwrap();
                let output = serde_json::json!({"status":"ok","result":{"value":8}})
                    .to_string()
                    .into();
                store
                    .append(
                        id,
                        EventPayload::ResponseItemAppended {
                            item: if custom {
                                ResponseItem::CustomToolCallOutput {
                                    call_id: "same-name".into(),
                                    output,
                                }
                            } else {
                                ResponseItem::FunctionCallOutput {
                                    call_id: "same-name".into(),
                                    output,
                                }
                            },
                        },
                        clock.now(),
                    )
                    .unwrap();
                (id, store.load_session(id).unwrap())
            };
            let db = Database::open(&path).unwrap();
            Session::recover(&db, &clock, id).unwrap();
            let store = EventStore::new(&db);
            let mut cursor = None;
            let mut actual = Vec::new();
            loop {
                let page = store
                    .read_session_page(
                        id,
                        None,
                        cursor,
                        SessionPageBudget {
                            max_events: 8,
                            max_bytes: 4096,
                        },
                    )
                    .unwrap();
                actual.extend(page.events);
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            assert_eq!(actual, expected);
            let child = Session::fork(&db, &clock, id, expected.last().unwrap().sequence).unwrap();
            assert_eq!(
                store
                    .load_session(child.id())
                    .unwrap()
                    .iter()
                    .map(|event| &event.payload)
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|event| &event.payload)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                store.load_session(id).unwrap(),
                expected,
                "legacy reads and branches must not rewrite the source"
            );
        }
    }
}

#[test]
fn h168_host_context_is_an_explicit_standalone_group_for_bounded_history() {
    use deepagent_persistence::event_store::SessionPageBudget;
    let db = Database::open_in_memory().unwrap();
    let clock = FixedClock::new(1);
    let mut session = Session::create(&db, &clock, None).unwrap();
    let context = ResponseItem::Message {
        role: "system".into(),
        content: "synthetic host context".into(),
    };
    session.append_host_context(context).unwrap();
    session.create_task("later task metadata").unwrap();
    session
        .append(EventPayload::Note {
            text: "later independent event".into(),
        })
        .unwrap();
    let store = EventStore::new(&db);
    let expected = store.load_session(session.id()).unwrap();
    let mut cursor = None;
    let mut actual = Vec::new();
    loop {
        let page = store
            .read_session_page(
                session.id(),
                None,
                cursor,
                SessionPageBudget {
                    max_events: 1,
                    max_bytes: 4096,
                },
            )
            .unwrap();
        assert!(page.events.len() <= 1);
        actual.extend(page.events);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(actual, expected);
    assert!(session
        .append_host_context(ResponseItem::Message {
            role: "assistant".into(),
            content: "provider output".into()
        })
        .is_err());
    assert_eq!(store.load_session(session.id()).unwrap(), expected);
}

#[test]
fn h152_native_discovery_completion_is_one_durable_projection_group() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("native.sqlite");
    let clock = FixedClock::new(1);
    let (id, source, expected) = {
        let db = Database::open(&path).unwrap();
        let mut session = Session::create(&db, &clock, None).unwrap();
        session
            .append_response_item(
                serde_json::from_value(serde_json::json!({
                    "type":"tool_search_call", "call_id":"native", "execution":"client",
                    "status":"completed", "arguments":{"query":"calendar"}
                }))
                .unwrap(),
            )
            .unwrap();
        let result =
            serde_json::json!({"matches":[{"name":"calendar_create"},{"name":"unregistered"}]});
        let schema = serde_json::json!({"type":"function", "name":"calendar_create", "parameters":{"type":"object"}});
        let completion = session
            .append_tool_completion(
                EventPayload::ToolCallCompleted {
                    call_id: "native".into(),
                    ok: true,
                    output: result,
                    content: Vec::new(),
                    duration_ms: 1,
                },
                &|name| (name == "calendar_create").then(|| schema.clone()),
            )
            .unwrap();
        let store = EventStore::new(&db);
        let events = store.load_session_for_model(session.id()).unwrap();
        let tail = events.last().unwrap();
        assert_eq!(tail.sequence, completion.sequence + 1);
        assert!(
            matches!(&tail.payload, EventPayload::ResponseItemAppended { item: ResponseItem::ToolSearchOutput { tools, .. }} if tools == &vec![schema])
        );
        assert_eq!(
            store
                .resolve_branch_sequence(session.id(), completion.sequence)
                .unwrap(),
            tail.sequence
        );
        (session.id(), completion.sequence, events)
    };
    let db = Database::open(&path).unwrap();
    let store = EventStore::new(&db);
    assert_eq!(store.load_session_for_model(id).unwrap(), expected);
    assert_eq!(
        store.resolve_branch_sequence(id, source).unwrap(),
        source + 1
    );
}

#[test]
fn h140_admitted_prompt_context_is_one_replay_branch_and_rewind_group() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("context.sqlite");
    let clock = FixedClock::new(1);
    let context = ResponseItem::Message {
        role: "user".into(),
        content: r#"{"kind":"hook_additional_context","contexts":[{"text":"synthetic"}]}"#.into(),
    };
    let (id, anchor) = {
        let db = Database::open(&path).unwrap();
        let mut session = Session::create(&db, &clock, None).unwrap();
        let anchor = session
            .append_user_prompt(Message::user("unchanged source"), Some(context.clone()))
            .unwrap()
            .sequence;
        (session.id(), anchor)
    };
    let db = Database::open(&path).unwrap();
    let store = EventStore::new(&db);
    let events = store.load_session(id).unwrap();
    assert_eq!(events.last().unwrap().sequence, anchor + 2);
    assert_eq!(
        store.resolve_branch_sequence(id, anchor).unwrap(),
        anchor + 2
    );
    assert!(
        matches!(&events.last().unwrap().payload,EventPayload::ResponseItemAppended {item} if *item==context)
    );
    let child = SessionId::new();
    store.fork_session(id, child, anchor, clock.now()).unwrap();
    let copied = store.load_session(child).unwrap();
    assert_eq!(
        events.iter().map(|e| &e.payload).collect::<Vec<_>>(),
        copied.iter().map(|e| &e.payload).collect::<Vec<_>>()
    );
    let mut session = Session::recover(&db, &clock, id).unwrap();
    session
        .append(EventPayload::MessageAppended {
            message: Message::assistant("later"),
        })
        .unwrap();
    session.rewind(anchor).unwrap();
    assert_eq!(store.load_session(id).unwrap(), events);
    let before = store.load_session(id).unwrap();
    let bad = ResponseItem::Message {
        role: "system".into(),
        content: "not user data".into(),
    };
    assert!(session
        .append_user_prompt(Message::user("must not partially commit"), Some(bad))
        .is_err());
    assert_eq!(store.load_session(id).unwrap(), before);
}

#[test]
fn admitted_images_keep_native_refs_and_legacy_mirror_branch_boundaries() {
    use deepagent_core::message::MessageAttachment;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("images.sqlite");
    let clock = FixedClock::new(1000);
    let message = Message::user("image turn").with_attachments(vec![MessageAttachment {
        sha256: Some("a".repeat(64)),
        id: "image-ref".into(),
        kind: "image".into(),
        media_type: Some("image/png".into()),
        path: Some("image.png".into()),
    }]);
    for legacy in [false, true] {
        let source;
        let anchor;
        {
            let db = Database::open(&path).unwrap();
            let mut session = Session::create(&db, &clock, None).unwrap();
            source = session.id();
            let store = EventStore::new(&db);
            let payload = EventPayload::MessageAppended {
                message: message.clone(),
            };
            anchor = if legacy {
                let anchor = store.append(source, payload, clock.now()).unwrap().sequence;
                store
                    .append(
                        source,
                        EventPayload::ResponseItemAppended {
                            item: ResponseItem::Message {
                                role: "user".into(),
                                content: message.content.clone(),
                            },
                        },
                        clock.now(),
                    )
                    .unwrap();
                anchor
            } else {
                session.append(payload).unwrap().sequence
            };
        }
        let db = Database::open(&path).unwrap();
        let store = EventStore::new(&db);
        let original = store.load_session(source).unwrap();
        assert_eq!(
            store.resolve_branch_sequence(source, anchor).unwrap(),
            anchor + 1
        );
        if !legacy {
            assert!(matches!(&original.last().unwrap().payload,
                EventPayload::ResponseItemAppended { item: ResponseItem::InputMessage { attachments, .. } }
                    if attachments == &message.attachments));
        }
        let child = SessionId::new();
        store
            .fork_session(source, child, anchor, clock.now())
            .unwrap();
        let copied = store.load_session(child).unwrap();
        assert_eq!(
            copied.iter().map(|e| &e.payload).collect::<Vec<_>>(),
            original.iter().map(|e| &e.payload).collect::<Vec<_>>()
        );
        let recovered = Session::recover(&db, &clock, child).unwrap();
        assert_eq!(recovered.state().message_count, 1);
        assert!(copied.iter().any(|event| matches!(&event.payload,
            EventPayload::MessageAppended { message: source }
                if source.attachments == message.attachments)));
    }
}

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
            "reasoning, two functions, and text are one group"
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
                signature: None,
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
                    signature: None,
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
            content: Vec::new(),
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
