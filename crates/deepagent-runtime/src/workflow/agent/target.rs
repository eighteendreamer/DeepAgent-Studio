//! v1 target requests are fresh recomputation, not cached-output resume.
use super::*;

pub(super) fn admit(compiled: &CompiledWorkflow, target: Option<&str>) -> Result<()> {
    let Some(target) = target else {
        return Ok(());
    };
    if !compiled.ancestors.contains_key(target) {
        return Err(CoreError::invalid(
            "InvalidWorkflowTarget: target node does not exist",
        ));
    }
    if !super::super::iteration::scope_of(&compiled.iteration_plans, target).is_empty() {
        return Err(CoreError::invalid(
            "InvalidWorkflowTarget: item-scoped target requires an explicit item context",
        ));
    }
    Ok(())
}

impl WorkflowAgent {
    /// The compiler's transitive control ancestors are the single scope
    /// authority. Branch activation still decides which admitted nodes run.
    pub(super) fn includes_target_node(&self, node_id: &str) -> bool {
        match &self.target_node_id {
            None => true,
            Some(target) => {
                node_id == target
                    || self
                        .compiled
                        .ancestors
                        .get(target)
                        .is_some_and(|ancestors| ancestors.contains(node_id))
            }
        }
    }
}
