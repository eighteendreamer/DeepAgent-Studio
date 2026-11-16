//! Read-only unified graph projection over main runs and persisted sub-agent
//! runs. The underlying `runs` and `subagent_runs` tables remain the facts;
//! this view only gives transports one stable shape.

use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::bounded_read::{
    json_len, HistoryCursor, HistoryPage, HistoryReadBudget, HistoryRow, HistorySnapshot,
    MAX_HISTORY_BYTES, MAX_HISTORY_ITEMS,
};
use deepagent_persistence::run_control::{ExecutionLeaseRecord, RunControlStore};
use deepagent_persistence::run_store::RunStore;
use deepagent_persistence::subagent_store::SubagentRunRecord;
use deepagent_persistence::subagent_store::SubagentRunStore;
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunGraphNodeDto {
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub origin_run_id: Option<String>,
    pub state: String,
    pub execution_location: String,
    pub worker_id: Option<String>,
    pub lease_id: Option<String>,
    pub lease_epoch: Option<u64>,
    pub join_token_hash: Option<String>,
    pub background: bool,
    pub resume_policy: String,
    pub agent_type: Option<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunGraphViewDto {
    pub root_run_id: String,
    pub nodes: Vec<RunGraphNodeDto>,
}

pub fn load(db: &Arc<Database>, root_run_id: &str) -> Result<RunGraphViewDto> {
    let root = RunStore::new(db)
        .get(root_run_id)?
        .ok_or_else(|| CoreError::not_found(format!("run {root_run_id} not found")))?;
    let root_lease = active_lease(db, &root.id);
    let mut nodes = vec![node(root.id, root.state, root_lease)];
    for child in SubagentRunStore::new(db).list_for_parent(root_run_id)? {
        let lease = active_lease(db, &child.id);
        nodes.push(child_node(child, lease));
    }
    Ok(RunGraphViewDto {
        root_run_id: root_run_id.into(),
        nodes,
    })
}

fn node(run_id: String, state: String, lease: Option<ExecutionLeaseRecord>) -> RunGraphNodeDto {
    RunGraphNodeDto {
        run_id,
        parent_run_id: None,
        origin_run_id: None,
        state,
        execution_location: "local".into(),
        worker_id: lease.as_ref().map(|lease| lease.owner.clone()),
        lease_id: lease.as_ref().map(|lease| lease.lease_id.clone()),
        lease_epoch: lease.as_ref().map(|lease| lease.epoch),
        join_token_hash: None,
        background: false,
        resume_policy: "manual".into(),
        agent_type: None,
        summary: None,
    }
}

fn child_node(child: SubagentRunRecord, lease: Option<ExecutionLeaseRecord>) -> RunGraphNodeDto {
    let mut node = node(child.id, child.state, lease);
    node.parent_run_id = Some(child.parent_run_id);
    node.origin_run_id = Some(child.origin_parent_run_id);
    node.agent_type = Some(child.agent_type);
    node.summary = child.summary;
    node
}

/// An opaque transport must protect these storage coordinates. This cursor is
/// a read checkpoint, never dispatch, lease, approval or authorization state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphHistoryCursor {
    root_run_id: String,
    root_emitted: bool,
    children: Option<HistoryCursor>,
    snapshot: HistorySnapshot,
}

/// Bounded graph projection using the same node builders and repositories as
/// `load`. Mutable state/leases are current projections; only membership is
/// cut. Unlike the legacy optional lease view, errors are never suppressed.
pub fn load_page(
    db: &Arc<Database>,
    root_run_id: &str,
    cursor: Option<&GraphHistoryCursor>,
    budget: HistoryReadBudget,
) -> Result<HistoryPage<RunGraphNodeDto, GraphHistoryCursor>> {
    if !(1..=MAX_HISTORY_ITEMS).contains(&budget.max_rows)
        || !(1..=MAX_HISTORY_BYTES).contains(&budget.max_bytes)
    {
        return Err(CoreError::invalid("invalid graph history page budget"));
    }
    let mut position = match cursor {
        Some(cursor)
            if cursor.root_run_id == root_run_id
                && budget
                    .snapshot
                    .is_none_or(|snapshot| snapshot == cursor.snapshot) =>
        {
            cursor.clone()
        }
        Some(_) => return Err(CoreError::invalid("foreign graph history cursor")),
        None => GraphHistoryCursor {
            root_run_id: root_run_id.into(),
            root_emitted: false,
            children: None,
            snapshot: match budget.snapshot {
                Some(snapshot) => snapshot,
                None => HistorySnapshot::capture(db)?,
            },
        },
    };
    let start_cursor = position.clone();
    let root = RunStore::new(db)
        .get_bounded(root_run_id, budget.max_bytes)?
        .ok_or_else(|| CoreError::not_found("graph root run"))?;
    let mut rows = Vec::new();
    let mut bytes = 2;
    if !position.root_emitted {
        let value = node(
            root.id,
            root.state,
            bounded_active_lease(db, root_run_id, budget.max_bytes)?,
        );
        bytes += json_len(&value, budget.max_bytes)?;
        if bytes > budget.max_bytes {
            return Err(graph_too_large());
        }
        position.root_emitted = true;
        rows.push(HistoryRow {
            value,
            cursor: position.clone(),
        });
    }
    let children = SubagentRunStore::new(db).list_for_parent_page(
        root_run_id,
        position.children.as_ref(),
        HistoryReadBudget {
            max_rows: budget.max_rows.saturating_sub(rows.len()).max(1),
            snapshot: Some(position.snapshot),
            ..budget
        },
    )?;
    let mut has_more = children.has_more;
    for child in children.rows {
        if rows.len() == budget.max_rows {
            has_more = true;
            break;
        }
        let lease = bounded_active_lease(db, &child.value.id, budget.max_bytes)?;
        let value = child_node(child.value, lease);
        let next_bytes =
            bytes + json_len(&value, budget.max_bytes)? + usize::from(!rows.is_empty());
        if next_bytes > budget.max_bytes {
            if rows.is_empty() {
                return Err(graph_too_large());
            }
            has_more = true;
            break;
        }
        bytes = next_bytes;
        position.children = Some(child.cursor);
        rows.push(HistoryRow {
            value,
            cursor: position.clone(),
        });
    }
    Ok(HistoryPage {
        start_cursor,
        rows,
        has_more,
    })
}

fn bounded_active_lease(
    db: &Arc<Database>,
    run_id: &str,
    max_bytes: usize,
) -> Result<Option<ExecutionLeaseRecord>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| CoreError::other("clock before epoch while reading graph history"))?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    RunControlStore::new(db).active_lease_bounded("run", run_id, now, max_bytes)
}

fn graph_too_large() -> CoreError {
    CoreError::invalid("history_item_too_large: graph node exceeds page budget")
}

fn active_lease(
    db: &Arc<Database>,
    run_id: &str,
) -> Option<deepagent_persistence::run_control::ExecutionLeaseRecord> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    RunControlStore::new(db)
        .active_lease("run", run_id, now)
        .ok()?
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::Timestamp;
    use deepagent_core::id::SessionId;
    use deepagent_persistence::event_store::EventStore;
    use deepagent_persistence::run_control::NewExecutionLease;
    use deepagent_persistence::subagent_store::SubagentRunRecord;

    fn page_fixture(children: usize) -> Arc<Database> {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        EventStore::new(&db)
            .create_session(session, None, Timestamp::from_millis(1))
            .unwrap();
        RunStore::new(&db)
            .create("root", &session.to_string(), None, 1)
            .unwrap();
        for index in 0..children {
            add_child(&db, &format!("child-{index:03}"));
        }
        db
    }

    fn add_child(db: &Arc<Database>, id: &str) {
        SubagentRunStore::new(db)
            .create(&SubagentRunRecord {
                id: id.into(),
                parent_run_id: "root".into(),
                origin_parent_run_id: "root".into(),
                state: "succeeded".into(),
                agent_type: "general".into(),
                transcript_path: None,
                worktree_path: None,
                summary: Some("synthetic".into()),
                created_at: 2,
                updated_at: 2,
                finished_at: Some(2),
                resume_count: 0,
            })
            .unwrap();
    }

    fn budget(max_rows: usize) -> HistoryReadBudget {
        HistoryReadBudget {
            max_rows,
            max_bytes: 2048,
            snapshot: None,
        }
    }

    #[test]
    fn h218_graph_pages_match_full_projection_and_freeze_late_children() {
        let db = page_fixture(23);
        let expected = load(&db, "root").unwrap().nodes;
        let first = load_page(&db, "root", None, budget(3)).unwrap();
        assert_eq!(first.rows.len(), 3);
        let mut position = first.rows.last().unwrap().cursor.clone();
        let mut actual: Vec<_> = first.rows.into_iter().map(|row| row.value).collect();
        add_child(&db, "late-child");
        for _ in 0..30 {
            let page = load_page(&db, "root", Some(&position), budget(3)).unwrap();
            let retry = load_page(&db, "root", Some(&position), budget(3)).unwrap();
            assert_eq!(
                page.rows.iter().map(|row| &row.value).collect::<Vec<_>>(),
                retry.rows.iter().map(|row| &row.value).collect::<Vec<_>>()
            );
            assert!(page.rows.len() <= 3);
            assert!(
                serde_json::to_vec(&page.rows.iter().map(|row| &row.value).collect::<Vec<_>>())
                    .unwrap()
                    .len()
                    <= 2048
            );
            if let Some(last) = page.rows.last() {
                assert_ne!(position, last.cursor);
                position = last.cursor.clone();
            }
            actual.extend(page.rows.into_iter().map(|row| row.value));
            if !page.has_more {
                break;
            }
        }
        assert_eq!(actual, expected);
        let foreign = load_page(&db, "foreign", Some(&position), budget(3)).unwrap_err();
        assert!(foreign.to_string().contains("foreign"));
    }

    #[test]
    fn h218_graph_root_only_and_single_node_pages_have_explicit_exhaustion() {
        let db = page_fixture(0);
        let page = load_page(&db, "root", None, budget(1)).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert!(!page.has_more);
        add_child(&db, "one-child");
        let first = load_page(&db, "root", None, budget(1)).unwrap();
        assert!(first.has_more);
        assert_eq!(first.rows[0].value.run_id, "root");
        let next = load_page(&db, "root", Some(&first.rows[0].cursor), budget(1)).unwrap();
        assert_eq!(next.rows.len(), 1);
        assert_eq!(next.rows[0].value.run_id, "one-child");
        assert!(!next.has_more);
    }

    #[test]
    fn h218_graph_bounded_leases_match_normal_view_and_oversize_is_not_silently_hidden() {
        let db = page_fixture(1);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        RunControlStore::new(&db)
            .acquire_lease(&NewExecutionLease {
                lease_id: "synthetic-lease",
                resource_kind: "run",
                resource_id: "child-000",
                owner: "synthetic-worker",
                fencing_token_hash: "synthetic-hash",
                expires_at: now + 60000,
                now,
            })
            .unwrap();
        let expected = load(&db, "root").unwrap().nodes;
        let page = load_page(&db, "root", None, budget(3)).unwrap();
        assert_eq!(
            page.rows
                .into_iter()
                .map(|row| row.value)
                .collect::<Vec<_>>(),
            expected
        );
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE execution_leases SET owner=?1 WHERE lease_id='synthetic-lease'",
                ["synthetic-sensitive".repeat(10000)],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
        .unwrap();
        let error = load_page(&db, "root", None, budget(3)).unwrap_err();
        assert!(error.to_string().contains("history_item_too_large"));
        assert!(!error.to_string().contains("synthetic-sensitive"));
    }

    #[test]
    fn h218_graph_rejects_oversized_child_or_root_metadata_without_echo() {
        for target in ["child", "root"] {
            let db = page_fixture(1);
            db.with_conn(|conn| {
                let query = if target == "child" {
                    "UPDATE subagent_runs SET summary=?1"
                } else {
                    "UPDATE runs SET terminal_reason=?1"
                };
                conn.execute(query, ["synthetic-private".repeat(10000)])
                    .map_err(|error| CoreError::Persistence(error.to_string()))?;
                Ok(())
            })
            .unwrap();
            let error = load_page(&db, "root", None, budget(3)).unwrap_err();
            assert!(error.to_string().contains("history_item_too_large"));
            assert!(!error.to_string().contains("synthetic-private"));
        }
    }

    #[test]
    fn graph_merges_root_and_child_without_creating_second_store() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session_id = SessionId::new();
        EventStore::new(&db)
            .create_session(session_id, Some("graph"), Timestamp::from_millis(1))
            .unwrap();
        RunStore::new(&db)
            .create("run-1", &session_id.to_string(), None, 1)
            .unwrap();
        SubagentRunStore::new(&db)
            .create(&SubagentRunRecord {
                id: "child-1".into(),
                parent_run_id: "run-1".into(),
                origin_parent_run_id: "run-1".into(),
                state: "running".into(),
                agent_type: "general".into(),
                transcript_path: None,
                worktree_path: None,
                summary: None,
                created_at: 2,
                updated_at: 2,
                finished_at: None,
                resume_count: 0,
            })
            .unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        RunControlStore::new(&db)
            .acquire_lease(&NewExecutionLease {
                lease_id: "worker-lease-1",
                resource_kind: "run",
                resource_id: "child-1",
                owner: "worker-a",
                fencing_token_hash: "hash-a",
                expires_at: now + 60_000,
                now,
            })
            .unwrap();
        let graph = load(&db, "run-1").unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].parent_run_id.as_deref(), Some("run-1"));
        assert_eq!(graph.nodes[1].worker_id.as_deref(), Some("worker-a"));
        assert_eq!(graph.nodes[1].lease_id.as_deref(), Some("worker-lease-1"));
        assert_eq!(graph.nodes[1].lease_epoch, Some(1));
        assert_eq!(
            serde_json::to_value(graph).unwrap()["nodes"][1]["executionLocation"],
            "local"
        );
    }
}
