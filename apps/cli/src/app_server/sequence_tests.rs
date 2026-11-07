//! H-91: real callback concurrency, one production writer and the existing ACK owner.
use super::*;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{mpsc as commands, Barrier};
use std::time::Duration;

fn started(producer: usize, round: usize) -> HarnessEvent {
    HarnessEvent::TurnStarted {
        thread_id: Some("synthetic-h91".into()),
        turn_id: format!("producer-{producer}-round-{round}"),
        step: Some(round),
    }
}

fn request(method: &str, params: Value) -> RpcRequest {
    RpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        id: json!(1),
        method: method.into(),
        params,
    }
}

fn ack(state: &mut ServerState, sequence: u64) -> RpcResponse {
    state
        .dispatch(
            request("event/ack", json!({"eventSequence":sequence})),
            None,
        )
        .0
}

pub(super) async fn concurrent_wire_with_ack(producers: usize, rounds: usize) {
    let (stdout, reader) = tokio::io::duplex(65536);
    let stdout = Arc::new(tokio::sync::Mutex::new(stdout));
    let (outbox, writer) = EventOutbox::new(stdout.clone());
    let output = outbox.output.clone();
    // This ACK-only fixture must not open an ambient package workspace vault.
    let workspace = tempfile::tempdir().unwrap();
    let mut state = ServerState::new_for_test_with_database(
        Arc::new(Database::open_in_memory().unwrap()),
        workspace.path().to_owned(),
    );
    assert!(state
        .dispatch(
            request(
                "initialize",
                json!({"clientName":"h91","clientVersion":"1","protocolVersion":PROTOCOL_VERSION})
            ),
            None
        )
        .0
        .is_success());
    state.output = output.clone();
    let barrier = Arc::new(Barrier::new(producers + 1));
    let mut senders = Vec::new();
    let mut workers = Vec::new();
    for producer in 0..producers {
        let (tx, rx) = commands::channel();
        senders.push(tx);
        let outbox = outbox.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            let mut round = 0;
            while rx.recv().is_ok() {
                barrier.wait();
                // This is the production entry point, not split allocation or
                // a test-only hook in the atomic-to-enqueue window.
                outbox.emit(started(producer, round));
                round += 1;
            }
        }));
    }
    let mut lines = tokio::io::BufReader::new(reader).lines();
    let mut observed = Vec::new();
    let mut rejected = Vec::new();
    let mut read_error = None;
    for _ in 0..rounds {
        for sender in &senders {
            sender.send(()).unwrap();
        }
        barrier.wait();
        // Drain each small batch before scheduling the next. A failed H-01
        // capacity gate must not masquerade as H-91 sequence reordering.
        let batch = tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..producers {
                let line = lines
                    .next_line()
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or("writer closed before all callbacks arrived")?;
                let record: RpcNotification =
                    serde_json::from_str(&line).map_err(|e| e.to_string())?;
                let sequence = record.event_sequence.ok_or("missing wire sequence")?;
                // Same flush/publication barrier as the production connection.
                {
                    let _flushed = stdout.lock().await;
                }
                let response = ack(&mut state, sequence);
                if !response.is_success() {
                    rejected.push((sequence, response.error_code()));
                }
                observed.push(record);
            }
            Ok::<(), String>(())
        })
        .await;
        match batch {
            Ok(Ok(())) => {}
            other => {
                read_error = Some(format!("{other:?}"));
                break;
            }
        }
    }
    // On read error, closing commands releases workers waiting for the next
    // batch as well. No sleep, abandoned thread or barrier on future batches.
    drop(senders);
    for worker in workers {
        worker.join().unwrap();
    }
    drop(outbox);
    tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    assert!(read_error.is_none(), "{read_error:?}");
    assert!(output.failure_reason().is_none());
    let total = producers * rounds;
    assert_eq!(observed.len(), total);
    let first_mismatch = observed
        .iter()
        .enumerate()
        .find(|(index, record)| record.event_sequence != Some(*index as u64 + 1))
        .map(|(index, record)| (index + 1, record.event_sequence));
    let identities: HashSet<_> = observed
        .iter()
        .map(|record| {
            assert_eq!(record.jsonrpc, JSONRPC_VERSION);
            assert_eq!(record.method, NOTIFICATION_METHOD);
            let HarnessEvent::TurnStarted { turn_id, .. } =
                serde_json::from_value(record.params.clone()).unwrap()
            else {
                panic!("unexpected synthetic event")
            };
            turn_id
        })
        .collect();
    assert_eq!(identities.len(), total, "no duplicate or missing callbacks");
    for producer in 0..producers {
        for round in 0..rounds {
            assert!(identities.contains(&format!("producer-{producer}-round-{round}")));
        }
    }
    eprintln!("H-91 real callbacks: producers={producers}, events={total}, first_wire_mismatch={first_mismatch:?}, rejected_acks={}", rejected.len());
    assert!(first_mismatch.is_none(), "{first_mismatch:?}");
    assert!(rejected.is_empty(), "{rejected:?}");
    assert_eq!(
        output.delivered_sequence.load(Ordering::Acquire),
        total as u64
    );
    assert!(ack(&mut state, total as u64).is_success());
    assert_eq!(
        ack(&mut state, total as u64 - 1).error_code(),
        Some(ERR_INVALID_PARAMS)
    );
    assert_eq!(
        ack(&mut state, total as u64 + 1).error_code(),
        Some(ERR_INVALID_PARAMS)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h91_actual_emitters_keep_wire_sequence_and_ack_order() {
    concurrent_wire_with_ack(16, 64).await;
}

#[tokio::test]
async fn h91_enqueue_winner_keeps_payload_order_and_new_connection_starts_at_one() {
    for _ in 0..2 {
        let (stdout, reader) = tokio::io::duplex(65536);
        let (outbox, writer) = EventOutbox::new(Arc::new(tokio::sync::Mutex::new(stdout)));
        let (release, wait) = commands::channel();
        let earlier = outbox.clone();
        let delayed = std::thread::spawn(move || {
            wait.recv().unwrap();
            earlier.emit(started(0, 0));
        });
        let later = outbox.clone();
        std::thread::spawn(move || later.emit(started(1, 0)))
            .join()
            .unwrap();
        release.send(()).unwrap();
        delayed.join().unwrap();
        let output = outbox.output.clone();
        drop(outbox);
        let mut lines = tokio::io::BufReader::new(reader).lines();
        for (sequence, producer) in [(1, 1), (2, 0)] {
            let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let record: RpcNotification = serde_json::from_str(&line).unwrap();
            assert_eq!(record.event_sequence, Some(sequence));
            assert_eq!(
                record.params["turnId"],
                format!("producer-{producer}-round-0")
            );
        }
        writer.await.unwrap();
        assert!(lines.next_line().await.unwrap().is_none());
        assert!(output.failure_reason().is_none());
        assert_eq!(output.delivered_sequence.load(Ordering::Acquire), 2);
    }
}

#[tokio::test]
async fn h91_sequence_exhaustion_fails_without_a_wrapped_notification() {
    let (stdout, reader) = tokio::io::duplex(65536);
    let (outbox, writer) = EventOutbox::new(Arc::new(tokio::sync::Mutex::new(stdout)));
    let output = outbox.output.clone();
    // Seed the existing writer-owned high-water, not a second allocator or
    // new production hook, to exercise the full-u64 boundary in finite time.
    output
        .delivered_sequence
        .store(u64::MAX - 1, Ordering::Release);
    outbox.emit(started(0, 0));
    outbox.emit(started(0, 1));
    drop(outbox);
    let mut lines = tokio::io::BufReader::new(reader).lines();
    let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let record: RpcNotification = serde_json::from_str(&line).unwrap();
    assert_eq!(record.event_sequence, Some(u64::MAX));
    tokio::time::timeout(Duration::from_secs(5), writer)
        .await
        .unwrap()
        .unwrap();
    assert!(lines.next_line().await.unwrap().is_none());
    assert_eq!(output.delivered_sequence.load(Ordering::Acquire), u64::MAX);
    assert_eq!(
        output.failure_reason().as_deref(),
        Some("app-server event sequence exhausted")
    );
}
