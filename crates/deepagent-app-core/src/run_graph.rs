//! Read-only unified graph projection over main runs and persisted sub-agent
//! runs. The underlying `runs` and `subagent_runs` tables remain the facts;
//! this view only gives transports one stable shape.

use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use deepagent_persistence::run_store::RunStore;
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
    let mut nodes = vec![RunGraphNodeDto {
        run_id: root.id,
        parent_run_id: None,
        origin_run_id: None,
        state: root.state,
        execution_location: "local".into(),
        worker_id: None,
        lease_id: None,
        join_token_hash: None,
        background: false,
        resume_policy: "manual".into(),
        agent_type: None,
        summary: None,
    }];
    for child in SubagentRunStore::new(db).list_for_parent(root_run_id)? {
        nodes.push(RunGraphNodeDto {
            run_id: child.id,
            parent_run_id: Some(child.parent_run_id),
            origin_run_id: Some(child.origin_parent_run_id),
            state: child.state,
            execution_location: "local".into(),
            worker_id: None,
            lease_id: None,
            join_token_hash: None,
            background: false,
            resume_policy: "manual".into(),
            agent_type: Some(child.agent_type),
            summary: child.summary,
        });
    }
    Ok(RunGraphViewDto {
        root_run_id: root_run_id.to_string(),
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::Timestamp;
    use deepagent_core::id::SessionId;
    use deepagent_persistence::event_store::EventStore;
    use deepagent_persistence::subagent_store::SubagentRunRecord;

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
        let graph = load(&db, "run-1").unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].parent_run_id.as_deref(), Some("run-1"));
        assert_eq!(
            serde_json::to_value(graph).unwrap()["nodes"][1]["executionLocation"],
            "local"
        );
    }
}
