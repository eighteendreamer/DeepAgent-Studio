//! Read-only follow over authoritative `thread/read` pages; no ACK or mutation.
use crate::{HarnessClient, SdkError};
use deepagent_harness_protocol::ThreadReadRequest;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

/// Save after processing a batch; restore on an explicitly reconnected client.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayCheckpoint {
    /// Bound on the first successful page; prevents cross-thread cursor reuse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// Exclusive session-file sequence, independent of each run's sequence.
    pub session_after_sequence: Option<u64>,
    /// Exclusive run-event high-water values keyed by durable run identity.
    pub run_after_sequences: BTreeMap<String, u64>,
}

/// A durable row with its independent stream identity, never a wire sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEvent {
    /// Absent for session-file rows.
    pub run_id: Option<String>,
    /// Durable stream sequence, including zero when present in stored history.
    pub sequence: u64,
    /// Original authoritative row, including its stream tags and payload.
    pub event: Value,
}

/// Pull-driven replay and follow. Dropping a pending wait stops only local IO.
pub struct ThreadReplay {
    client: HarnessClient,
    thread_id: String,
    checkpoint: ReplayCheckpoint,
    cursor: Option<String>,
    caught_up: bool,
}

impl HarnessClient {
    /// Local design: reuse paged reads, polling at 250ms after exhaustion.
    /// RPC failures are returned, never automatically retried or reconnected.
    pub fn watch_thread_events(
        &self,
        thread_id: String,
        checkpoint: ReplayCheckpoint,
    ) -> ThreadReplay {
        ThreadReplay {
            client: self.clone(),
            thread_id,
            checkpoint,
            cursor: None,
            caught_up: false,
        }
    }
}

impl ThreadReplay {
    /// Checkpoint to save only after processing the last returned batch.
    pub fn checkpoint(&self) -> &ReplayCheckpoint {
        &self.checkpoint
    }

    /// Read the next nonempty bounded batch, waiting while history is caught up.
    pub async fn next_batch(&mut self) -> Result<Vec<ReplayEvent>, SdkError> {
        if self
            .checkpoint
            .thread_id
            .as_deref()
            .is_some_and(|id| id != self.thread_id)
        {
            return Err(invalid_page());
        }
        loop {
            if self.caught_up {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            let continuation = self.cursor.is_some();
            let response = self
                .client
                .thread_read(ThreadReadRequest {
                    thread_id: self.thread_id.clone(),
                    page_size: None,
                    max_bytes: None,
                    cursor: self.cursor.clone(),
                    after_sequence: None,
                    session_after_sequence: if continuation {
                        None
                    } else {
                        self.checkpoint.session_after_sequence
                    },
                    run_after_sequence: None,
                    run_after_sequences: (!continuation)
                        .then(|| self.checkpoint.run_after_sequences.clone()),
                })
                .await?;
            let page = response.result.as_ref().ok_or_else(invalid_page)?;
            let (events, checkpoint, cursor) = read_page(page, &self.thread_id, &self.checkpoint)?;
            if cursor.is_some() && cursor == self.cursor {
                return Err(invalid_page());
            }
            self.checkpoint = checkpoint;
            self.caught_up = cursor.is_none();
            self.cursor = cursor;
            if !events.is_empty() {
                return Ok(events);
            }
        }
    }
}

fn invalid_page() -> SdkError {
    SdkError::InvalidStreamIdentity {
        method: "thread/read",
        field: "historyPage",
    }
}

fn read_page(
    page: &Value,
    thread: &str,
    checkpoint: &ReplayCheckpoint,
) -> Result<(Vec<ReplayEvent>, ReplayCheckpoint, Option<String>), SdkError> {
    if checkpoint
        .thread_id
        .as_deref()
        .is_some_and(|id| id != thread)
        || page["threadId"].as_str() != Some(thread)
        || page["historyPageVersion"] != 1
        || page["cursorMode"] != "stream_scoped"
    {
        return Err(invalid_page());
    }
    let cursor = match page["hasMore"].as_bool() {
        Some(true) => Some(
            page["nextCursor"]
                .as_str()
                .filter(|v| !v.is_empty())
                .ok_or_else(invalid_page)?
                .to_owned(),
        ),
        Some(false) if page.get("nextCursor").is_some_and(Value::is_null) => None,
        _ => return Err(invalid_page()),
    };
    let mut next = checkpoint.clone();
    next.thread_id = Some(thread.to_owned());
    let mut output = vec![];
    let mut collect = |rows: &[Value], run: Option<&str>| -> Result<(), SdkError> {
        let mut previous = None;
        for row in rows {
            if row["stream"].as_str() != Some(if run.is_some() { "run" } else { "session" })
                || run.is_some_and(|id| row["runId"].as_str() != Some(id))
            {
                return Err(invalid_page());
            }
            let sequence = row["streamSequence"].as_u64().ok_or_else(invalid_page)?;
            if previous.is_some_and(|value| sequence <= value) {
                return Err(invalid_page());
            }
            previous = Some(sequence);
            let high = match run {
                Some(id) => next.run_after_sequences.get(id).copied(),
                None => next.session_after_sequence,
            };
            if high.is_some_and(|value| sequence <= value) {
                continue;
            }
            output.push(ReplayEvent {
                run_id: run.map(str::to_owned),
                sequence,
                event: row.clone(),
            });
            if let Some(id) = run {
                next.run_after_sequences.insert(id.to_owned(), sequence);
            } else {
                next.session_after_sequence = Some(sequence);
            }
        }
        Ok(())
    };
    collect(page["events"].as_array().ok_or_else(invalid_page)?, None)?;
    for run in page["runEvents"].as_array().ok_or_else(invalid_page)? {
        let id = run["runId"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(invalid_page)?;
        collect(run["events"].as_array().ok_or_else(invalid_page)?, Some(id))?;
    }
    Ok((output, next, cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn page(session: &[u64], run: &[u64]) -> Value {
        json!({"threadId":"t","historyPageVersion":1,"cursorMode":"stream_scoped",
            "hasMore":false,"nextCursor":null,
            "events":session.iter().map(|n| json!({"stream":"session","streamSequence":n})).collect::<Vec<_>>(),
            "runEvents":[{"runId":"r","events":run.iter().map(|n| json!({"stream":"run","runId":"r","streamSequence":n})).collect::<Vec<_>>()}]})
    }
    #[test]
    fn replay_preserves_stream_domains_zero_and_resume_without_duplicates() {
        let (events, checkpoint, _) =
            read_page(&page(&[0, 1], &[0, 1]), "t", &ReplayCheckpoint::default()).unwrap();
        assert_eq!(events.len(), 4);
        let restored: ReplayCheckpoint =
            serde_json::from_str(&serde_json::to_string(&checkpoint).unwrap()).unwrap();
        let (events, next, _) =
            read_page(&page(&[0, 1, 2], &[0, 1, u64::MAX]), "t", &restored).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(next.run_after_sequences["r"], u64::MAX);
    }
    #[test]
    fn invalid_scope_cursor_and_order_do_not_advance_checkpoint() {
        let checkpoint = ReplayCheckpoint::default();
        assert!(read_page(&page(&[2, 1], &[]), "t", &checkpoint).is_err());
        let mut missing = page(&[], &[]);
        missing["hasMore"] = true.into();
        assert!(read_page(&missing, "t", &checkpoint).is_err());
        assert!(read_page(&page(&[], &[]), "foreign", &checkpoint).is_err());
        let (_, bound, _) = read_page(&page(&[], &[]), "t", &checkpoint).unwrap();
        let mut foreign = page(&[], &[]);
        foreign["threadId"] = "foreign".into();
        assert!(read_page(&foreign, "foreign", &bound).is_err());
        let mut wrong_stream = page(&[], &[1]);
        wrong_stream["runEvents"][0]["events"][0]["runId"] = "foreign".into();
        assert!(read_page(&wrong_stream, "t", &checkpoint).is_err());
        assert_eq!(checkpoint, ReplayCheckpoint::default());
    }
}
