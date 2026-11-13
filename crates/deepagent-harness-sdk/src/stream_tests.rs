use super::*;
use serde_json::{json, Value};
use std::time::Duration;

fn response(value: Value) -> RpcResponse {
    RpcResponse::success(json!(1), value)
}
fn event(value: Value) -> HarnessEvent {
    serde_json::from_value(value).unwrap()
}
fn complete(kind: &str, thread: Option<&str>, turn: &str) -> HarnessEvent {
    let mut value = json!({"type":kind,"turnId":turn,"message":"synthetic","reason":"synthetic"});
    if let Some(thread) = thread {
        value["threadId"] = json!(thread);
    }
    event(value)
}

#[tokio::test]
async fn h214_thread_scope_ignores_other_threads_and_ends_after_its_started() {
    let (events, _) = broadcast::channel(8);
    let (_, closed) = watch::channel(crate::connection::ConnectionState::Open);
    let mut stream = RequestEvents::thread(
        response(json!({"threadId":"A"})),
        events.subscribe(),
        closed,
    )
    .unwrap();
    for id in ["B", "A", "A"] {
        events
            .send(event(json!({"type":"thread.started","threadId":id,"protocolVersion":1})).into())
            .unwrap();
    }
    assert_eq!(stream.thread_id(), "A");
    assert_eq!(stream.turn_id(), None);
    assert!(
        matches!(stream.recv().await.unwrap(), Some(HarnessEvent::ThreadStarted { thread_id, .. }) if thread_id=="A")
    );
    assert!(stream.recv().await.unwrap().is_none());
    assert_eq!(events.receiver_count(), 0);
}

#[tokio::test]
async fn h214_turn_scope_filters_conflicting_ids_and_retains_v1_optional_thread_context() {
    let (events, _) = broadcast::channel(16);
    let (_closed, closed) = watch::channel(crate::connection::ConnectionState::Open);
    let mut stream = RequestEvents::turn(
        response(json!({"threadId":"A","turnId":"run"})),
        events.subscribe(),
        closed,
        "A",
    )
    .unwrap();
    events
        .send(complete("turn.completed", Some("B"), "run").into())
        .unwrap();
    events
        .send(complete("turn.completed", Some("A"), "other").into())
        .unwrap();
    for kind in [
        "turn.started",
        "item.started",
        "item.updated",
        "item.completed",
        "approval.requested",
    ] {
        events
            .send(
                event(
                    json!({"type":kind,"turnId":"run","threadId":"A","reason":"fixture",
            "item":{"kind":"content_delta","text":"synthetic"}}),
                )
                .into(),
            )
            .unwrap();
    }
    events
        .send(complete("turn.completed", None, "run").into())
        .unwrap();
    let mut kinds = Vec::new();
    while let Some(value) = stream.recv().await.unwrap() {
        kinds.push(serde_json::to_value(value).unwrap()["type"].clone());
    }
    assert_eq!(
        kinds,
        vec![
            json!("turn.started"),
            json!("item.started"),
            json!("item.updated"),
            json!("item.completed"),
            json!("approval.requested"),
            json!("turn.completed")
        ]
    );
    assert_eq!(events.receiver_count(), 0);
}

#[tokio::test]
async fn h84_diagnostic_is_streamed_without_becoming_an_actionable_approval() {
    let (events, _) = broadcast::channel(4);
    let (_closed, closed) = watch::channel(crate::connection::ConnectionState::Open);
    let mut stream = RequestEvents::turn(
        response(json!({"threadId":"A","turnId":"run"})),
        events.subscribe(),
        closed,
        "A",
    )
    .unwrap();
    let diagnostic = event(json!({"type":"item.updated","threadId":"A","turnId":"run",
        "item":{"kind":"runtime","eventType":"tool_blocked",
        "data":{"name":"read_file","reason":"hook ask","needs_approval":true}}}));
    events.send(diagnostic.clone().into()).unwrap();
    events
        .send(complete("turn.completed", Some("A"), "run").into())
        .unwrap();
    assert_eq!(stream.recv().await.unwrap(), Some(diagnostic));
    assert!(matches!(
        stream.recv().await.unwrap(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    assert!(stream.recv().await.unwrap().is_none());
}

#[tokio::test]
async fn h214_each_terminal_wins_over_later_duplicates_and_eof() {
    for kind in ["turn.completed", "turn.failed", "turn.interrupted"] {
        let (events, _) = broadcast::channel(4);
        let (closed, receiver) = watch::channel(crate::connection::ConnectionState::Open);
        let mut stream = RequestEvents::turn(
            response(json!({"threadId":"A","turnId":"run"})),
            events.subscribe(),
            receiver,
            "A",
        )
        .unwrap();
        let terminal = complete(kind, Some("A"), "run");
        events.send(terminal.clone().into()).unwrap();
        events.send(terminal.clone().into()).unwrap();
        crate::connection::ConnectionState::retire(
            &closed,
            crate::connection::ConnectionState::Closed,
        );
        assert_eq!(stream.recv().await.unwrap(), Some(terminal));
        assert!(stream.recv().await.unwrap().is_none());
        assert_eq!(events.receiver_count(), 0);
    }
}

#[tokio::test]
async fn h214_eof_and_lag_are_explicit_failures_and_release_the_receiver() {
    for lagged in [false, true] {
        let (events, _) = broadcast::channel(2);
        let (closed, receiver) = watch::channel(crate::connection::ConnectionState::Open);
        let mut stream = RequestEvents::turn(
            response(json!({"threadId":"A","turnId":"run"})),
            events.subscribe(),
            receiver,
            "A",
        )
        .unwrap();
        if lagged {
            for _ in 0..5 {
                events
                    .send(complete("turn.completed", Some("A"), "run").into())
                    .unwrap();
            }
        }
        crate::connection::ConnectionState::retire(
            &closed,
            crate::connection::ConnectionState::Closed,
        );
        let error = stream.recv().await.unwrap_err();
        assert!(if lagged {
            matches!(error, SdkError::EventLagged { skipped } if skipped>0)
        } else {
            matches!(error, SdkError::EventStreamClosed)
        });
        assert_eq!(events.receiver_count(), 0);
        assert!(stream.recv().await.unwrap().is_none());
    }
    // A dropped watch producer must not create a busy loop either.
    let (events, _) = broadcast::channel(2);
    let (_, receiver) = watch::channel(crate::connection::ConnectionState::Open);
    let mut stream = RequestEvents::thread(
        response(json!({"threadId":"A"})),
        events.subscribe(),
        receiver,
    )
    .unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), stream.recv())
            .await
            .unwrap(),
        Err(SdkError::EventStreamClosed)
    ));
}

#[tokio::test]
async fn h214_receive_cancellation_is_safe_and_explicit_close_and_drop_only_unsubscribe() {
    let (events, _) = broadcast::channel(4);
    let (_closed, receiver) = watch::channel(crate::connection::ConnectionState::Open);
    let mut stream = RequestEvents::turn(
        response(json!({"threadId":"A","turnId":"run"})),
        events.subscribe(),
        receiver.clone(),
        "A",
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(5), stream.recv())
            .await
            .is_err()
    );
    events
        .send(complete("turn.completed", Some("A"), "run").into())
        .unwrap();
    assert!(matches!(
        stream.recv().await.unwrap(),
        Some(HarnessEvent::TurnCompleted { .. })
    ));
    let mut stream = RequestEvents::thread(
        response(json!({"threadId":"A"})),
        events.subscribe(),
        receiver.clone(),
    )
    .unwrap();
    stream.close();
    assert!(stream.recv().await.unwrap().is_none());
    assert_eq!(events.receiver_count(), 0);
    let stream = RequestEvents::thread(
        response(json!({"threadId":"A"})),
        events.subscribe(),
        receiver,
    )
    .unwrap();
    assert_eq!(events.receiver_count(), 1);
    drop(stream);
    assert_eq!(events.receiver_count(), 0);
}

#[test]
fn h214_invalid_response_identity_never_panics_leaks_values_or_retains_a_subscription() {
    let (events, _) = broadcast::channel(4);
    let (_closed, receiver) = watch::channel(crate::connection::ConnectionState::Open);
    for value in [
        json!({}),
        json!({"threadId":null}),
        json!({"threadId":" \t"}),
        json!({"threadId":123}),
    ] {
        let error = RequestEvents::thread(response(value), events.subscribe(), receiver.clone())
            .err()
            .unwrap();
        assert!(matches!(error, SdkError::InvalidStreamIdentity { .. }));
        assert_eq!(events.receiver_count(), 0);
    }
    for value in [
        json!({"threadId":"A"}),
        json!({"threadId":"A","turnId":""}),
        json!({"threadId":"PRIVATE_BAD_ID","turnId":"run"}),
    ] {
        let error = RequestEvents::turn(response(value), events.subscribe(), receiver.clone(), "A")
            .err()
            .unwrap();
        assert!(!error.to_string().contains("PRIVATE_BAD_ID"));
        assert_eq!(events.receiver_count(), 0);
    }
    let mut malformed = response(json!({"threadId":"A"}));
    malformed.result = None;
    assert!(RequestEvents::thread(malformed, events.subscribe(), receiver).is_err());
    assert_eq!(events.receiver_count(), 0);
}

#[tokio::test]
async fn h205_owned_metadata_and_business_receives_share_one_receiver() {
    let (events, _) = broadcast::channel(8);
    let (_closed, closed) = watch::channel(crate::connection::ConnectionState::Open);
    let mut stream = RequestEvents::turn(
        response(json!({"threadId":"A","turnId":"run"})),
        events.subscribe(),
        closed,
        "A",
    )
    .unwrap();
    for (index, kind) in ["turn.started", "item.updated", "turn.completed"]
        .iter()
        .enumerate()
    {
        let payload = if *kind == "item.updated" {
            event(json!({"type":"item.updated","threadId":"A","turnId":"run",
                "item":{"kind":"content_delta","text":"synthetic"}}))
        } else {
            complete(kind, Some("A"), "run")
        };
        let mut notification: HarnessNotification = payload.into();
        notification.event_sequence = Some(index as u64 + 7);
        notification.sequence_status = if index == 0 {
            SequenceStatus::First
        } else {
            SequenceStatus::Continuous
        };
        events.send(notification).unwrap();
    }
    let first = stream.recv_notification().await.unwrap().unwrap();
    assert_eq!(first.event_sequence, Some(7));
    assert!(matches!(first.event, HarnessEvent::TurnStarted { .. }));
    assert!(matches!(
        stream.recv().await.unwrap(),
        Some(HarnessEvent::ItemUpdated { .. })
    ));
    let last = stream.recv_notification().await.unwrap().unwrap();
    assert_eq!(last.event_sequence, Some(9));
    assert!(matches!(last.event, HarnessEvent::TurnCompleted { .. }));
    assert!(stream.recv().await.unwrap().is_none());
    assert_eq!(events.receiver_count(), 0);
}

#[tokio::test]
async fn h205_scoped_views_cannot_hide_connection_gap_or_reorder_in_unrelated_scope() {
    for non_monotonic in [false, true] {
        let (events, _) = broadcast::channel(4);
        let (_closed, closed) = watch::channel(crate::connection::ConnectionState::Open);
        let mut stream = RequestEvents::turn(
            response(json!({"threadId":"A","turnId":"run"})),
            events.subscribe(),
            closed,
            "A",
        )
        .unwrap();
        let mut notification: HarnessNotification =
            complete("turn.started", Some("B"), "other").into();
        notification.event_sequence = Some(if non_monotonic { 7 } else { 10 });
        notification.sequence_status = if non_monotonic {
            SequenceStatus::NonMonotonic { previous: 8 }
        } else {
            SequenceStatus::Gap {
                previous: 8,
                missing: 1,
            }
        };
        events.send(notification).unwrap();
        events
            .send(complete("turn.completed", Some("A"), "run").into())
            .unwrap();
        let error = stream.recv_notification().await.unwrap_err();
        assert!(match error {
            SdkError::EventSequenceGap {
                previous: 8,
                received: 10,
                missing: 1,
            } => !non_monotonic,
            SdkError::EventSequenceNonMonotonic {
                previous: 8,
                received: 7,
            } => non_monotonic,
            _ => false,
        });
        assert!(stream.recv().await.unwrap().is_none());
        assert_eq!(events.receiver_count(), 0);
    }
}
