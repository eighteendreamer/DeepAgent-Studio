//! Test-only bounded-page consumer for existing lifecycle acceptance fixtures.
//! Keeping the original multi-MiB burst and terminal-count assertions requires
//! following the new protocol, not treating the first page as the whole ledger.

use super::*;
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Default)]
pub(super) struct HistoryCollector {
    merged: Option<Value>,
    seen: HashSet<(String, String, String)>,
    pages: usize,
    bytes: usize,
}

impl HistoryCollector {
    pub(super) fn push(&mut self, page: Value) -> Option<String> {
        self.pages += 1;
        self.bytes += serde_json::to_vec(&page).unwrap().len();
        assert!(
            self.pages <= 128 && self.bytes <= 32 * 1024 * 1024,
            "synthetic history consumer exceeded its independent finite window"
        );
        assert_eq!(page["historyPageVersion"], 1);
        assert!(serde_json::to_vec(&page).unwrap().len() <= 256 * 1024);
        let more = page["hasMore"].as_bool().unwrap();
        let cursor = page["nextCursor"].as_str().map(str::to_owned);
        assert_eq!(cursor.is_some(), more);
        if let Some(cursor) = &cursor {
            assert!(!cursor.is_empty() && cursor.len() <= 4096);
        }
        if self.merged.is_none() {
            let mut initial = page.clone();
            initial["events"] = json!([]);
            initial["runEvents"] = json!([]);
            self.merged = Some(initial);
        }
        let mut count = page["events"].as_array().unwrap().len();
        for event in page["events"].as_array().unwrap() {
            self.remember("session", "events", event, "id");
            self.merged.as_mut().unwrap()["events"]
                .as_array_mut()
                .unwrap()
                .push(event.clone());
        }
        for run in page["runEvents"].as_array().unwrap() {
            count += 1;
            let run_id = run["runId"].as_str().unwrap();
            // Duplicate rows are a failure, not silently deduplicated evidence.
            for (field, key) in [
                ("events", "sequence"),
                ("actions", "callId"),
                ("approvals", "approvalId"),
                ("artifacts", "id"),
            ] {
                count += run[field].as_array().unwrap().len();
                for row in run[field].as_array().unwrap() {
                    self.remember(run_id, field, row, key);
                }
            }
            count += run["graph"]["nodes"].as_array().unwrap().len();
            for row in run["graph"]["nodes"].as_array().unwrap() {
                self.remember(run_id, "graph", row, "runId");
            }
            let merged_runs = self.merged.as_mut().unwrap()["runEvents"]
                .as_array_mut()
                .unwrap();
            let index = match merged_runs
                .iter()
                .position(|prior| prior["runId"] == run_id)
            {
                Some(index) => index,
                None => {
                    let mut initial = run.clone();
                    for field in ["events", "actions", "approvals", "artifacts"] {
                        initial[field] = json!([]);
                    }
                    initial["graph"]["nodes"] = json!([]);
                    merged_runs.push(initial);
                    merged_runs.len() - 1
                }
            };
            let merged = &mut merged_runs[index];
            merged["state"] = run["state"].clone();
            merged["terminalKind"] = run["terminalKind"].clone();
            for field in ["events", "actions", "approvals", "artifacts"] {
                merged[field]
                    .as_array_mut()
                    .unwrap()
                    .extend(run[field].as_array().unwrap().iter().cloned());
            }
            merged["graph"]["nodes"]
                .as_array_mut()
                .unwrap()
                .extend(run["graph"]["nodes"].as_array().unwrap().iter().cloned());
        }
        assert!(count <= 500, "all projections share one item budget");
        let merged = self.merged.as_mut().unwrap();
        merged["hasMore"] = json!(more);
        merged["nextCursor"] = page["nextCursor"].clone();
        cursor
    }

    fn remember(&mut self, run: &str, field: &str, row: &Value, key: &str) {
        assert!(!row[key].is_null());
        assert!(
            self.seen
                .insert((run.into(), field.into(), row[key].to_string())),
            "duplicate history row in {run}/{field}"
        );
    }

    pub(super) fn finish(self) -> Value {
        let merged = self.merged.unwrap();
        assert_eq!(
            merged["hasMore"], false,
            "must verify the complete synthetic history"
        );
        eprintln!(
            "H-218 test consumer: {} bounded pages, {} JSON result bytes, no duplicate rows",
            self.pages, self.bytes
        );
        merged
    }
}

pub(super) fn read_history(state: &mut ServerState, mut params: Value) -> Value {
    let mut collector = HistoryCollector::default();
    loop {
        let (response, launch) = state.dispatch(
            RpcRequest {
                jsonrpc: "2.0".into(),
                id: json!(1),
                method: "thread/read".into(),
                params: params.clone(),
            },
            None,
        );
        assert!(response.is_success(), "{response:?}");
        assert!(launch.is_none());
        assert!(serde_json::to_vec(&response).unwrap().len() <= 256 * 1024);
        match collector.push(response.result().clone()) {
            Some(cursor) => params["cursor"] = json!(cursor),
            None => return collector.finish(),
        }
    }
}
