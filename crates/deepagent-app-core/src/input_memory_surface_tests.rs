fn h53_attachment(id: &str) -> ResponseInputItem {
    deepagent_context::memory_attachment::memory_attachment(&[
        deepagent_context::memory_attachment::RelevantMemory {
            id: id.into(),
            block: format!("excerpt {id}"),
        },
    ])
    .unwrap()
}

#[test]
fn h53_replay_uses_visible_attachments_not_historical_injection_ids() {
    use deepagent_context::memory_attachment::surfaced_memory_ids;
    let sid = SessionId::nil();
    let mut events = vec![
        event(
            sid,
            0,
            EventPayload::MemoriesInjected {
                ids: vec!["absent".into()],
                count: 1,
                latency_ms: 0,
                trigger: "prefetch".into(),
            },
        ),
        event(
            sid,
            1,
            EventPayload::ResponseItemAppended {
                item: h53_attachment("old"),
            },
        ),
        event(
            sid,
            2,
            EventPayload::ResponseItemAppended {
                item: h53_attachment("kept"),
            },
        ),
    ];
    assert_eq!(
        surfaced_memory_ids(&response_items_from_events(&events)),
        vec!["old", "kept"]
    );
    for sequence in [3, 4] {
        events.push(event(
            sid,
            sequence,
            boundary("summary mentions old but has no attachment", Some(2)),
        ));
        assert_eq!(
            surfaced_memory_ids(&response_items_from_events(&events)),
            vec!["kept"]
        );
    }
    // An old ID-only log cannot establish that an excerpt survived.
    assert!(surfaced_memory_ids(&response_items_from_events(&events[..1])).is_empty());
}

#[test]
fn h53_replay_snip_releases_removed_attachment_but_keeps_protected_tail() {
    use deepagent_context::memory_attachment::surfaced_memory_ids;
    let sid = SessionId::nil();
    let mut payloads = vec![
        EventPayload::ResponseItemAppended {
            item: ResponseInputItem::Message {
                role: "user".into(),
                content: "old task [id:u1]".into(),
            },
        },
        EventPayload::ResponseItemAppended {
            item: h53_attachment("old"),
        },
        EventPayload::ResponseItemAppended {
            item: ResponseInputItem::Message {
                role: "user".into(),
                content: "new task [id:u2]".into(),
            },
        },
    ];
    payloads.extend((0..10).map(|index| EventPayload::ResponseItemAppended {
        item: ResponseInputItem::Message {
            role: "assistant".into(),
            content: format!("tail {index}"),
        },
    }));
    payloads.push(EventPayload::ResponseItemAppended {
        item: h53_attachment("kept"),
    });
    payloads.push(EventPayload::ContextSnipped {
        tags: vec!["u1".into()],
    });
    let events: Vec<_> = payloads
        .into_iter()
        .enumerate()
        .map(|(index, payload)| event(sid, index as u64, payload))
        .collect();
    assert_eq!(
        surfaced_memory_ids(&response_items_from_events(&events)),
        vec!["kept"]
    );
}
