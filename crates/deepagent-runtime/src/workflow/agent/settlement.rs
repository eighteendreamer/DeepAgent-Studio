//! Settlement belongs to the current walk, not the first end in storage order.
use super::*;

impl WorkflowAgent {
    /// Reuse the admitted scope and the statuses that drive edge activation.
    /// No extra think step is needed when the final member has just settled.
    pub(super) fn settle_walk(&self) -> Result<AgentDecision> {
        if self.step < self.walk.len() {
            return Ok(AgentDecision::Continue);
        }
        let mut ends = BTreeMap::new();
        for &index in &self.walk {
            let node = &self.compiled.definition.nodes[index];
            match self.node_statuses.get(&node.id) {
                Some(NodeExecutionStatus::Completed) => {
                    if node.kind == "end" {
                        let output = self.outputs.get(&node.id).ok_or_else(|| {
                            CoreError::invalid("InvalidWorkflowGraph: completed end has no output")
                        })?;
                        ends.insert(node.id.as_str(), output);
                    }
                }
                Some(NodeExecutionStatus::Skipped) => {}
                _ => return Err(CoreError::invalid(
                    "InvalidWorkflowGraph: workflow exhausted with an unsettled or unsuccessful node",
                )),
            }
        }
        if self.item.is_some() {
            return Ok(AgentDecision::Complete("iteration item completed".into()));
        }
        if let Some(target) = &self.target_node_id {
            if self.node_statuses.get(target) != Some(&NodeExecutionStatus::Completed) {
                return Err(CoreError::invalid(
                    "InvalidWorkflowTarget: target was not selected by control flow",
                ));
            }
            if !ends.contains_key(target.as_str()) {
                let output = self.outputs.get(target).ok_or_else(|| {
                    CoreError::invalid("InvalidWorkflowTarget: completed target has no output")
                })?;
                values::check_value(output, "target result")?;
                return Ok(AgentDecision::Complete(serde_json::to_string(output)?));
            }
        }
        let summary = match ends.len() {
            0 => {
                return Err(CoreError::invalid(
                    "InvalidWorkflowGraph: workflow exhausted without completing an end node",
                ))
            }
            // Preserve the existing successful single-end summary contract.
            1 => {
                let output = ends.values().next().expect("one completed end");
                output
                    .get("output")
                    .or_else(|| output.as_object().and_then(|m| m.values().next()))
                    .map(Value::to_string)
                    .unwrap_or_else(|| "workflow completed".into())
            }
            _ => {
                // Local v1 join: deterministic IDs, every completed end's full
                // output, and the existing 1 MiB serialized value boundary.
                values::check_serialized(&ends, "end settlement")?;
                serde_json::to_string(&ends)?
            }
        };
        Ok(AgentDecision::Complete(summary))
    }

    /// Kernel/item-owner cleanup precedes durable run settlement. An abort
    /// cannot turn unvisited members into success or leave them unexplained.
    pub(super) fn settle_unvisited_nodes(&mut self) {
        let (status, reason) = if self.is_cancelled() {
            (
                NodeExecutionStatus::Cancelled,
                "workflow cancelled before node execution",
            )
        } else {
            (
                NodeExecutionStatus::Skipped,
                "workflow stopped before node execution",
            )
        };
        for &index in &self.walk {
            let id = &self.compiled.definition.nodes[index].id;
            if self
                .node_statuses
                .get(id)
                .is_some_and(|status| status.is_terminal())
            {
                continue;
            }
            self.node_statuses.insert(id.clone(), status);
            self.emit_status(id, status, 1, 0, None, Some(reason.into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{RuntimeEvent, RuntimeEventSink};
    use crate::workflow::compile;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Capture(Mutex<Vec<NodeExecutionEvent>>);
    impl RuntimeEventSink for Capture {
        fn emit(&self, event: RuntimeEvent) {
            if let RuntimeEvent::WorkflowNode { event } = event {
                self.0.lock().unwrap().push(event);
            }
        }
    }

    fn fixture() -> WorkflowAgent {
        WorkflowAgent::new(compile(serde_json::from_value(json!({"version":1,"nodes":[
            {"id":"start","kind":"start"}, {"id":"end","kind":"end"},
            {"id":"tail","kind":"variable-assigner","config":{"assignments":[]}}
        ],"edges":[{"id":"a","source":"start","target":"end"},{"id":"b","source":"start","target":"tail"}]})).unwrap()).unwrap(),
            Map::new(), None, NodeEventPublisher::default()).unwrap()
    }

    #[tokio::test]
    async fn h194_settlement_does_not_spend_an_extra_kernel_step() {
        use crate::{AgentKernel, RunRequest, RuntimeConfig, TerminalKind};
        use deepagent_core::clock::FixedClock;
        use deepagent_persistence::{run_store::RunStore, Database};
        use deepagent_session::Session;
        use deepagent_tools::ToolRegistry;
        use deepagent_tracing::metrics::Metrics;
        for steps in [2, 3] {
            let mut agent = fixture();
            let capture = Arc::new(Capture::default());
            agent.publisher = NodeEventPublisher::new(capture.clone());
            let db = Arc::new(Database::open_in_memory().unwrap());
            let clock = FixedClock::new(1);
            let mut session = Session::create(&db, &clock, None).unwrap();
            let task = session.create_task("synthetic walk budget").unwrap();
            let registry = ToolRegistry::new();
            let kernel = AgentKernel::<FixedClock>::new(
                db.clone(),
                &registry,
                Metrics::new(),
                RuntimeConfig {
                    max_steps: Some(steps),
                    ..Default::default()
                },
                "h194-step-budget",
            )
            .with_events(capture.clone());
            let terminal = kernel
                .start(RunRequest::new(&mut session, task, &mut agent))
                .await
                .unwrap();
            assert_eq!(
                terminal.kind,
                if steps == 3 {
                    TerminalKind::Succeeded
                } else {
                    TerminalKind::MaxTurns
                }
            );
            assert_eq!(agent.step, steps);
            let events = capture.0.lock().unwrap();
            let tails: Vec<_> = events
                .iter()
                .filter(|e| e.node_id == "tail" && e.status.is_terminal())
                .collect();
            assert_eq!(tails.len(), 1);
            assert_eq!(
                tails[0].status,
                if steps == 3 {
                    NodeExecutionStatus::Completed
                } else {
                    NodeExecutionStatus::Skipped
                }
            );
            let ledger = RunStore::new(&db)
                .events_after("h194-step-budget", None)
                .unwrap();
            assert_eq!(
                ledger
                    .iter()
                    .filter(|e| e.event_type == "run_terminal")
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn h194_exhausted_walk_with_missing_running_failed_or_cancelled_member_fails_closed() {
        for status in [
            None,
            Some(NodeExecutionStatus::Running),
            Some(NodeExecutionStatus::Failed),
            Some(NodeExecutionStatus::Cancelled),
        ] {
            let mut agent = fixture();
            for step in 0..3 {
                agent.think(step, &[]).await.unwrap();
            }
            agent.node_statuses.remove("tail");
            if let Some(status) = status {
                agent.node_statuses.insert("tail".into(), status);
            }
            assert!(agent
                .settle_walk()
                .unwrap_err()
                .to_string()
                .contains("unsettled or unsuccessful"));
        }
        let mut agent = fixture();
        for step in 0..3 {
            agent.think(step, &[]).await.unwrap();
        }
        agent.outputs.remove("end");
        assert!(agent
            .settle_walk()
            .unwrap_err()
            .to_string()
            .contains("completed end has no output"));
    }

    #[tokio::test]
    async fn h195_join_result_budget_counts_escaped_bytes_before_allocating_summary() {
        let mut agent = WorkflowAgent::new(compile(serde_json::from_value(json!({"version":1,"nodes":[
            {"id":"start","kind":"start"},{"id":"a","kind":"end"},{"id":"b","kind":"end"}
        ],"edges":[{"id":"a","source":"start","target":"a"},{"id":"b","source":"start","target":"b"}]})).unwrap()).unwrap(),
            Map::new(), None, NodeEventPublisher::default()).unwrap();
        for step in 0..3 {
            agent.think(step, &[]).await.unwrap();
        }
        for id in ["a", "b"] {
            agent
                .outputs
                .insert(id.into(), json!({"report":"\"".repeat(300_000)}));
        }
        assert!(agent
            .settle_walk()
            .unwrap_err()
            .to_string()
            .contains("end settlement: serialized value exceeds 1 MiB"));
        for id in ["a", "b"] {
            agent
                .outputs
                .insert(id.into(), json!({"report":"\"".repeat(200_000)}));
        }
        let AgentDecision::Complete(summary) = agent.settle_walk().unwrap() else {
            panic!("bounded join must succeed")
        };
        assert_eq!(
            serde_json::from_str::<Value>(&summary).unwrap()["a"]["report"],
            "\"".repeat(200_000)
        );
    }
}
