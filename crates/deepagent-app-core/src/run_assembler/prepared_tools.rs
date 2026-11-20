//! One real main-run registry, prepared without selecting a model or starting a
//! worker. Live execution services are bound once before that registry runs.
use super::*;
use async_trait::async_trait;
use deepagent_builtins::{
    BackgroundSubagent, PlanExecutionResult, PlanExecutor, SubagentOutcome, SubagentRequest,
    SubagentRunner, SubagentStatus,
};
use deepagent_tools::{ToolExecutionContext, ToolRegistry};
use std::collections::{BTreeMap, HashSet};
use std::sync::OnceLock;

use crate::subagent_runner::{collect_runtime_agent_definitions, RuntimeAgentDefinition};
use crate::tool_runtime::{build_main_run_toolset, MainRunToolset, MainRunToolsetRequest};

struct BoundRunServices {
    runner: Arc<dyn SubagentRunner>,
    plan: Arc<dyn PlanExecutor>,
}

/// Only a late-bound reference to the existing production executors. This owns
/// no queue, run state, approval policy or alternate implementation of a tool.
#[derive(Clone, Default)]
pub(super) struct RunToolServices(Arc<OnceLock<BoundRunServices>>);

impl RunToolServices {
    pub(super) fn bind(&self, runner: ChatSubagentRunner) -> Result<()> {
        self.0
            .set(BoundRunServices {
                plan: Arc::new(crate::dag_orchestration::ChatPlanExecutor::new(
                    runner.clone(),
                )),
                runner: Arc::new(runner),
            })
            .map_err(|_| CoreError::invalid("prepared run tool services are already bound"))
    }

    fn bound(&self) -> Result<&BoundRunServices> {
        self.0
            .get()
            .ok_or_else(|| CoreError::invalid("prepared run tool services are not bound"))
    }
}

#[async_trait]
impl SubagentRunner for RunToolServices {
    async fn run(&self, request: SubagentRequest) -> Result<String> {
        self.bound()?.runner.run(request).await
    }

    async fn run_controlled(
        &self,
        request: SubagentRequest,
        context: ToolExecutionContext,
    ) -> Result<SubagentOutcome> {
        self.bound()?.runner.run_controlled(request, context).await
    }

    async fn start_background(
        &self,
        request: SubagentRequest,
        context: ToolExecutionContext,
    ) -> Result<BackgroundSubagent> {
        self.bound()?
            .runner
            .start_background(request, context)
            .await
    }

    async fn status(&self, id: &str) -> Result<SubagentStatus> {
        self.bound()?.runner.status(id).await
    }

    async fn cancel(&self, id: &str) -> Result<bool> {
        self.bound()?.runner.cancel(id).await
    }

    async fn resume(
        &self,
        id: &str,
        prompt: &str,
        context: ToolExecutionContext,
    ) -> Result<String> {
        self.bound()?.runner.resume(id, prompt, context).await
    }

    async fn cleanup(&self, id: &str) -> Result<bool> {
        self.bound()?.runner.cleanup(id).await
    }
}

#[async_trait]
impl PlanExecutor for RunToolServices {
    async fn execute_plan(&self, goal: String) -> Result<PlanExecutionResult> {
        self.bound()?.plan.execute_plan(goal).await
    }

    async fn execute_plan_controlled(
        &self,
        goal: String,
        context: ToolExecutionContext,
    ) -> Result<PlanExecutionResult> {
        self.bound()?
            .plan
            .execute_plan_controlled(goal, context)
            .await
    }
}

pub(super) struct MainRunPreparationRequest<'a> {
    pub(super) base: ToolRegistryBuildRequest<'a>,
    pub(super) sub_base: ToolRegistryBuildRequest<'a>,
    #[cfg(all(
        feature = "desktop-screen",
        any(windows, target_os = "macos", target_os = "linux")
    ))]
    pub(super) screen_capture: bool,
    pub(super) plugin_projection: Option<&'a crate::plugin_runtime::PluginRuntimeProjection>,
    pub(super) plugin_registry: Option<Arc<deepagent_mcp::McpRegistry>>,
    pub(super) plan: deepagent_builtins::PlanMode,
    pub(super) discovered: DiscoveredToolSet,
    pub(super) search_mode: deepagent_builtins::ToolSearchMode,
    pub(super) search_threshold: usize,
}

pub(super) struct PreparedMainRunTools {
    pub(super) toolset: MainRunToolset,
    pub(super) services: RunToolServices,
    pub(super) sub_registry: Arc<ToolRegistry>,
    pub(super) agent_definitions: BTreeMap<String, RuntimeAgentDefinition>,
    pub(super) parent_discovered_snapshot: HashSet<String>,
}

impl RunAssembler<'_> {
    /// Prepare the unique registry consumed by the normal run. No ModelClient,
    /// credentials, event sink or active run is needed to build its tool face.
    pub(super) fn prepare_main_run_tools(
        &self,
        request: MainRunPreparationRequest<'_>,
    ) -> Result<PreparedMainRunTools> {
        let root = request.base.root.to_owned();
        let sub_registry = Arc::new(build_base_tool_registry(request.sub_base)?.0);
        let agents = collect_runtime_agent_definitions(
            [root, self.workspace.to_owned()],
            request.plugin_projection,
        );
        let task_agent_types = agents
            .iter()
            .map(RuntimeAgentDefinition::task_agent_type)
            .collect();
        let agent_definitions = agents
            .into_iter()
            .map(|agent| (agent.type_name.clone(), agent))
            .collect();
        let parent_discovered_snapshot = request
            .discovered
            .lock()
            .map_err(|_| CoreError::other("discovered tool set mutex poisoned"))?
            .clone();
        let services = RunToolServices::default();
        let memory_backend = self.knowledge.as_ref().map(|knowledge| {
            Arc::new(crate::memory_service::MemoryService::new(knowledge.clone()))
                as Arc<dyn deepagent_builtins::MemoryBackend>
        });
        if memory_backend.is_none() {
            tracing::warn!(
                "no knowledge vault attached; memory_write / memory_recall are off this run"
            );
        }
        let toolset = build_main_run_toolset(MainRunToolsetRequest {
            base: request.base,
            mcp: self.mcp.as_deref(),
            plugin_projection: request.plugin_projection,
            plugin_registry: request.plugin_registry,
            task_runner: services.clone(),
            task_agent_types,
            plan_executor: Some(Arc::new(services.clone())),
            memory_backend,
            #[cfg(all(
                feature = "desktop-screen",
                any(windows, target_os = "macos", target_os = "linux")
            ))]
            screen_capture: request.screen_capture,
            plan: request.plan,
            skills: self.skills.as_ref(),
            tool_search_mode: request.search_mode,
            tool_search_discovered: request.discovered,
            tool_search_threshold: request.search_threshold,
        })?;
        Ok(PreparedMainRunTools {
            toolset,
            services,
            sub_registry,
            agent_definitions,
            parent_discovered_snapshot,
        })
    }
}

#[cfg(test)]
mod tests;
