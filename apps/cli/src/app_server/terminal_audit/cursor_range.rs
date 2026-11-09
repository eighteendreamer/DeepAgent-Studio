//! H-298: complete paginated projections, not just the first bounded page.
use super::*;
use std::collections::BTreeMap;

fn filtered(
    full: &Value,
    session_after: Option<u64>,
    run_after: Option<u64>,
    per_run: &BTreeMap<String, u64>,
) -> Value {
    let mut expected = full.clone();
    expected["events"] = json!(full["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| session_after
            .is_none_or(|after| event["streamSequence"].as_u64().unwrap() > after))
        .cloned()
        .collect::<Vec<_>>());
    for run in expected["runEvents"].as_array_mut().unwrap() {
        let id = run["runId"].as_str().unwrap();
        let after = per_run.get(id).copied().or(run_after);
        run["events"] = json!(run["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(
                |event| after.is_none_or(|after| event["streamSequence"].as_u64().unwrap() > after)
            )
            .cloned()
            .collect::<Vec<_>>());
    }
    expected
}

#[tokio::test]
async fn h298_every_cursor_field_and_override_preserves_two_durable_runs() {
    let fixture = Fixture::new(cursor_range_response()).await;
    let first = fixture.run_turn().await;
    let second = fixture.run_turn().await;
    let full = fixture.read();
    for run in [&first, &second] {
        assert_durable_terminal(&full, run, "succeeded");
    }
    let tail = full["runEvents"][0]["events"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["streamSequence"]
        .as_u64()
        .unwrap();
    let session_middle = full["events"][full["events"].as_array().unwrap().len() / 2]
        ["streamSequence"]
        .as_u64()
        .unwrap();
    let notifications = fixture.events.lock().unwrap().clone();
    let mut observations = 0;
    for reopened in [false, true] {
        let mut independent = fixture.reopened_state();
        let mut live = fixture.state.lock().unwrap();
        let state = if reopened {
            &mut independent
        } else {
            &mut live
        };
        for field in [
            "afterSequence",
            "sessionAfterSequence",
            "runAfterSequence",
            "runAfterSequences",
        ] {
            for after in [
                None,
                Some(0),
                Some(1),
                Some(tail / 2),
                Some(tail),
                Some(tail + 1),
                Some(i64::MAX as u64),
                Some(i64::MAX as u64 + 1),
                Some(u64::MAX),
            ] {
                let mut params = json!({"threadId":fixture.thread_id,"pageSize":3});
                let mut runs = BTreeMap::new();
                if let Some(after) = after {
                    if field == "runAfterSequences" {
                        runs = BTreeMap::from([(first.clone(), after), (second.clone(), after)]);
                        params[field] = json!(runs);
                    } else {
                        params[field] = json!(after);
                    }
                }
                let session_after = matches!(field, "afterSequence" | "sessionAfterSequence")
                    .then_some(after)
                    .flatten();
                let run_after = matches!(field, "afterSequence" | "runAfterSequence")
                    .then_some(after)
                    .flatten();
                let read = read_history(state, params);
                assert_eq!(
                    read,
                    filtered(&full, session_after, run_after, &runs),
                    "field={field}, after={after:?}, reopened={reopened}"
                );
                observations += 1;
            }
        }
        for (fallback, session_after, run_after, first_after) in [
            (u64::MAX, session_middle, u64::MAX, tail / 2),
            (0, u64::MAX, tail / 2, u64::MAX),
        ] {
            let per_run = BTreeMap::from([(first.clone(), first_after), (second.clone(), 0)]);
            let params = json!({"threadId":fixture.thread_id,"pageSize":3,
                "afterSequence":fallback,"sessionAfterSequence":session_after,
                "runAfterSequence":run_after,"runAfterSequences":per_run});
            assert_eq!(
                read_history(state, params),
                filtered(&full, Some(session_after), Some(run_after), &per_run)
            );
            observations += 1;
        }
        assert_eq!(
            read_history(state, json!({"threadId":fixture.thread_id})),
            full
        );
    }
    assert_eq!(
        *fixture.events.lock().unwrap(),
        notifications,
        "reads cannot publish new runtime events"
    );
    assert_eq!(observations, 76);
    eprintln!("H-298: {observations} fully collected cursor/override observations, two durable runs, live/reopened dispatcher; no launch or history mutation");
}
