//! Item contexts are ordinary WorkflowAgents driven by the owning Kernel.
//! No tool dispatch, retries, run/store or approval owner lives here.

use futures::stream::{FuturesUnordered, StreamExt};

use super::*;
use crate::workflow::iteration::{scope_of, ErrorHandling, IterationPlan};

/// Aggregate *serialized output-store* budget across all live item contexts,
/// including suspended nested owners. This is not a claim about allocator RSS.
const MAX_ITEM_OUTPUT_BYTES: usize = values::MAX_BYTES * super::super::iteration::MAX_CONCURRENCY;

pub(super) struct OutputLease {
    budget: Arc<std::sync::Mutex<usize>>,
    bytes: usize,
}

impl OutputLease {
    pub(super) fn reserve(budget: Arc<std::sync::Mutex<usize>>, bytes: usize) -> Result<Self> {
        let mut lease = Self { budget, bytes: 0 };
        lease.resize(bytes)?;
        Ok(lease)
    }

    pub(super) fn resize(&mut self, bytes: usize) -> Result<()> {
        let mut total = self.budget.lock().unwrap_or_else(|p| p.into_inner());
        let next = (*total - self.bytes)
            .checked_add(bytes)
            .filter(|next| *next <= MAX_ITEM_OUTPUT_BYTES)
            .ok_or_else(|| {
                CoreError::invalid("IterationResourceLimit: live item output stores exceed 50 MiB")
            })?;
        *total = next;
        self.bytes = bytes;
        Ok(())
    }
}

impl Drop for OutputLease {
    fn drop(&mut self) {
        *self.budget.lock().unwrap_or_else(|p| p.into_inner()) -= self.bytes;
    }
}

struct ItemExecution {
    index: usize,
    slot: usize,
    agent: WorkflowAgent,
}

pub(super) struct IterationExecution {
    plan: IterationPlan,
    items: Vec<Value>,
    next: usize,
    active: Vec<ItemExecution>,
    results: Vec<Option<Value>>,
    finished: usize,
    result_bytes: usize,
    started: Instant,
}

impl WorkflowAgent {
    pub(super) fn begin_iteration(&mut self, node_id: &str) -> Result<()> {
        let plan = self
            .compiled
            .iteration_plans
            .get(node_id)
            .cloned()
            .ok_or_else(|| CoreError::invalid("iteration has no admitted plan"))?;
        let items = plan.items(&self.outputs)?;
        // Bound the inherited snapshot before making up to 50 independent
        // contexts. No item gets another item's private output map.
        values::check_value(&serde_json::to_value(&self.outputs)?, "iteration snapshot")?;
        self.pending_iteration = Some(IterationExecution {
            plan,
            results: vec![None; items.len()],
            items,
            next: 0,
            active: Vec::new(),
            finished: 0,
            result_bytes: 0,
            started: Instant::now(),
        });
        Ok(())
    }

    fn item_agent(
        &self,
        plan: &IterationPlan,
        index: usize,
        item: Value,
        lanes: usize,
    ) -> Result<Self> {
        let mut expected = scope_of(&self.compiled.iteration_plans, &plan.node_id);
        expected.push(plan.node_id.clone());
        let snapshot_bytes = serde_json::to_vec(&self.outputs)?.len();
        let lease = OutputLease::reserve(self.item_output_budget.clone(), snapshot_bytes)?;
        let mut child = Self::from_shared(
            self.compiled.clone(),
            self.inputs.clone(),
            None,
            self.publisher.scoped(&plan.node_id, index),
        );
        child.walk = self
            .compiled
            .order
            .iter()
            .copied()
            .filter(|index| {
                scope_of(
                    &self.compiled.iteration_plans,
                    &self.compiled.definition.nodes[*index].id,
                ) == expected
            })
            .collect();
        child.outputs = self.outputs.clone();
        child.item_output_budget = self.item_output_budget.clone();
        child.item_output_lease = Some(lease);
        child.item = Some(item);
        child.item_roots = plan.roots.clone();
        child.item_lanes = lanes;
        child.cancel = Some(Arc::new(AtomicBool::new(false)));
        child.model = self.model.clone();
        child.model_name = self.model_name.clone();
        child.knowledge_retriever = self.knowledge_retriever.clone();
        child.tool_executor = self.tool_executor.clone();
        child.tool_executor_lock = self.tool_executor_lock.clone();
        child.canvas_bridge = self.canvas_bridge.clone();
        child.artifact_validator = self.artifact_validator.clone();
        child.code_bridge = self.code_bridge.clone();
        child.usage = self.usage.clone();
        Ok(child)
    }

    pub(super) fn admit_item_output(&mut self, node_id: &str, value: &Value) -> Result<()> {
        let Some(lease) = &mut self.item_output_lease else {
            return Ok(());
        };
        let mut snapshot = serde_json::to_value(&self.outputs)?;
        snapshot
            .as_object_mut()
            .expect("output map")
            .insert(node_id.to_owned(), value.clone());
        values::check_value(&snapshot, "iteration item output store")?;
        lease.resize(serde_json::to_vec(&snapshot)?.len())
    }

    fn item_event(
        &self,
        owner: &str,
        index: usize,
        status: NodeExecutionStatus,
        error: Option<String>,
    ) {
        // The owner remains Running until every result is aggregated. Existing
        // UI consumers must not confuse one item's terminal with its owner.
        self.publisher.emit(NodeExecutionEvent {
            status: NodeExecutionStatus::Running,
            updates: BTreeMap::from([(
                "iterationItem".into(),
                json!({
                    "index":index, "status":status, "error":error
                }),
            )]),
            ..self.base_event(owner)
        });
    }

    fn fill_items(&mut self) -> Result<()> {
        let state = self.pending_iteration.as_ref().expect("active iteration");
        let concurrency = state
            .plan
            .concurrency
            .min(self.item_lanes)
            .min(state.items.len().max(1));
        let slots =
            (0..concurrency).filter(|slot| !state.active.iter().any(|child| child.slot == *slot));
        let children: Vec<_> = slots
            .zip(state.next..state.items.len())
            .map(|(slot, index)| {
                let lanes = self.item_lanes / concurrency
                    + usize::from(slot < self.item_lanes % concurrency);
                Ok(ItemExecution {
                    index,
                    slot,
                    agent: self.item_agent(
                        &state.plan,
                        index,
                        state.items[index].clone(),
                        lanes,
                    )?,
                })
            })
            .collect::<Result<_>>()?;
        for child in &children {
            self.item_event(
                &state.plan.node_id,
                child.index,
                NodeExecutionStatus::Running,
                None,
            );
        }
        let state = self.pending_iteration.as_mut().expect("active iteration");
        state.next += children.len();
        state.active.extend(children);
        Ok(())
    }

    pub(super) async fn advance_iteration(
        &mut self,
        last: &[Observation],
    ) -> Result<AgentDecision> {
        if self.is_cancelled() {
            return Err(CoreError::other("workflow cancelled"));
        }
        if let Err(error) = self.fill_items() {
            return self.fail_iteration(error).await;
        }
        // Keep agents in the parent while polling borrowed futures: dropping
        // think on timeout/cancellation never loses their cleanup ownership.
        let decisions = {
            let state = self.pending_iteration.as_mut().expect("active iteration");
            let mut pending = FuturesUnordered::new();
            for item in &mut state.active {
                pending.push(async move { (item.index, item.agent.think(0, last).await) });
            }
            let mut decisions = BTreeMap::new();
            while let Some((index, result)) = pending.next().await {
                let stop = self.usage.check_control().is_err()
                    || (result.is_err() && state.plan.error_handling == ErrorHandling::Terminate);
                decisions.insert(index, result);
                if stop {
                    break;
                }
            }
            decisions
        };
        if let Err(error) = self.usage.check_control() {
            return self.fail_iteration(error).await;
        }
        let mut tools = Vec::new();
        for (index, decision) in decisions {
            let result = match decision {
                Ok(AgentDecision::Continue) => continue,
                Ok(AgentDecision::CallTool(tool)) => {
                    tools.push(tool);
                    continue;
                }
                Ok(AgentDecision::CallTools(batch)) => {
                    tools.extend(batch);
                    continue;
                }
                Ok(AgentDecision::Complete(_)) => {
                    let state = self.pending_iteration.as_ref().expect("active iteration");
                    let child = state
                        .active
                        .iter()
                        .find(|child| child.index == index)
                        .expect("owned item");
                    state.plan.select(&state.items[index], &child.agent.outputs)
                }
                Ok(_) => Err(CoreError::invalid("unexpected decision from workflow item")),
                Err(error) => Err(error),
            };
            if let Err(error) = self.finish_item(index, result).await {
                // Includes a terminate policy or bounded aggregate violation.
                return self.fail_iteration(error).await;
            }
        }
        let state = self.pending_iteration.as_ref().expect("active iteration");
        if state.finished == state.items.len() {
            let state = self.pending_iteration.take().expect("active iteration");
            let mut output = Vec::new();
            for value in state.results {
                let value = match value {
                    Some(value) => value,
                    None if state.plan.error_handling == ErrorHandling::Remove => continue,
                    None => Value::Null,
                };
                match value {
                    Value::Array(items) if state.plan.flatten => output.extend(items),
                    value => output.push(value),
                }
            }
            let result = json!({"output":output});
            let result = values::check_value(&result, "iteration output").map(|()| result);
            return self.finish_node(
                state.plan.node_id,
                "iteration".into(),
                result,
                state.started.elapsed().as_millis() as u64,
            );
        }
        // Kernel preserves stable call order, permission/approval and exclusive
        // side-effect barriers. This scheduler never executes a tool itself.
        Ok(if tools.is_empty() {
            AgentDecision::Continue
        } else {
            AgentDecision::CallTools(tools)
        })
    }

    async fn finish_item(&mut self, index: usize, result: Result<Value>) -> Result<()> {
        let state = self.pending_iteration.as_mut().expect("active iteration");
        let position = state
            .active
            .iter()
            .position(|child| child.index == index)
            .expect("owned item");
        let mut child = state.active.remove(position);
        let owner = state.plan.node_id.clone();
        let (status, error) = match &result {
            Ok(_) => (NodeExecutionStatus::Completed, None),
            Err(error) => (NodeExecutionStatus::Failed, Some(error.to_string())),
        };
        child.agent.finish_background_tasks().await;
        self.item_event(&owner, index, status, error);
        let state = self.pending_iteration.as_mut().expect("active iteration");
        state.finished += 1;
        match result {
            Ok(value) => {
                values::check_value(&value, "iteration item output")?;
                state.result_bytes += serde_json::to_vec(&value)?.len() + 1;
                state.results[index] = Some(value);
            }
            Err(error) if state.plan.error_handling == ErrorHandling::Terminate => {
                return Err(error);
            }
            Err(_) if state.plan.error_handling == ErrorHandling::Continue => {
                state.result_bytes += 5
            }
            Err(_) => {}
        }
        if state.result_bytes > values::MAX_BYTES {
            return Err(CoreError::invalid("iteration aggregate exceeds 1 MiB"));
        }
        Ok(())
    }

    async fn cancel_items(&mut self) {
        let Some(state) = self.pending_iteration.as_mut() else {
            return;
        };
        for child in &mut state.active {
            if let Some(cancel) = &child.agent.cancel {
                cancel.store(true, Ordering::Release);
            }
        }
        let owner = state.plan.node_id.clone();
        let mut indices = Vec::new();
        for mut child in state.active.drain(..) {
            child.agent.finish_background_tasks().await;
            indices.push(child.index);
        }
        for index in indices {
            self.item_event(
                &owner,
                index,
                NodeExecutionStatus::Cancelled,
                Some("iteration stopped; unobserved side effects are not retried".into()),
            );
        }
    }

    async fn fail_iteration(&mut self, error: CoreError) -> Result<AgentDecision> {
        self.cancel_items().await;
        let state = self.pending_iteration.take().expect("active iteration");
        self.finish_node(
            state.plan.node_id,
            "iteration".into(),
            Err(error),
            state.started.elapsed().as_millis() as u64,
        )
    }

    pub(super) async fn stop_iteration(&mut self) {
        if self.pending_iteration.is_none() {
            return;
        }
        let _ = self
            .fail_iteration(CoreError::other(
                "iteration stopped before aggregation; reconcile durable actions before retrying",
            ))
            .await;
    }
}
