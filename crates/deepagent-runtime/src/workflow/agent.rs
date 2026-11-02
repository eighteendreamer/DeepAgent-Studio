//! Workflow execution agent that drives a compiled graph through the kernel.
//!
//! [`WorkflowAgent`] implements [`Agent`] so the existing `AgentKernel` loop can
//! execute professional canvas workflows without a second run center. Each
//! `think()` call advances one node in topological order, resolving variable
//! references, executing the node operation, and publishing status events.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use deepagent_core::message::Message;
use deepagent_models::chat_completions::ChatCompletionRequest;

use super::canvas::{
    CanvasArtifactResponse, CanvasArtifactValidator, CanvasAudioInputPreparation,
    CanvasAudioRequest, CanvasCompletionRequest, CanvasEmbeddingRequest, CanvasImagePreparation,
    CanvasImageRequest, CanvasImageResponse, CanvasJobProgressSink, CanvasModelBridge,
    CanvasPromptPolicy, CanvasRouteRequest, CanvasVideoPreparation, CanvasVideoRequest,
    CanvasVideoResponse,
};
use deepagent_models::client::ModelClient;
use serde_json::{json, Map, Value};

use super::code::{
    CodeBridge, CodeInvocation, CodeOutput, CodeRequest, CodeResponse, CODE_EXECUTE_TOOL,
};
use super::graph::CompiledWorkflow;
use super::knowledge::KnowledgeRetriever;
use super::node_events::{NodeEventPublisher, NodeExecutionEvent, NodeExecutionStatus};
use super::tools::ToolExecutor;
use super::usage::{WorkflowUsage, WorkflowUsageObserver};
use super::values;
use crate::agent::{Agent, AgentDecision, Observation, RunUsage};
use deepagent_core::error::{CoreError, Result};

mod item_scheduler;
#[cfg(test)]
mod iteration_tests;
use item_scheduler::{IterationExecution, OutputLease};

struct PendingNodeTool {
    node_id: String,
    call_id: String,
    started: Instant,
    kind: PendingNodeToolKind,
}

enum PendingNodeToolKind {
    Http,
    Code {
        invocation: CodeInvocation,
        outputs: Vec<CodeOutput>,
    },
}

impl PendingNodeToolKind {
    fn node_kind(&self) -> &'static str {
        match self {
            Self::Http => "http-request",
            Self::Code { .. } => "code",
        }
    }

    fn tool_name(&self) -> &'static str {
        match self {
            Self::Http => "http_request",
            Self::Code { .. } => CODE_EXECUTE_TOOL,
        }
    }
}

struct PendingMediaNode {
    node_id: String,
    node_kind: String,
    call_id: String,
    started: Instant,
    output: PendingMediaOutput,
}

enum PendingMediaOutput {
    Image,
    AudioInput(Map<String, Value>),
    Video,
}

impl PendingMediaOutput {
    fn tool_name(&self) -> &'static str {
        match self {
            Self::Image => "canvas_image_fetch",
            Self::AudioInput(_) => "canvas_audio_fetch",
            Self::Video => "canvas_video_fetch",
        }
    }
}

/// Executes a compiled workflow graph as an [`Agent`].
///
/// The agent walks nodes in topological order. Each `think()` call executes one
/// node, stores its outputs, and emits lifecycle events. When all nodes complete
/// (or an `end` node is reached), the agent returns `Complete` with the final
/// output summary.
pub struct WorkflowAgent {
    compiled: Arc<CompiledWorkflow>,
    walk: Vec<usize>,
    item: Option<Value>,
    item_roots: BTreeSet<String>,
    /// A fixed lane quota is divided among nested schedulers, never multiplied.
    item_lanes: usize,
    pending_iteration: Option<IterationExecution>,
    item_output_budget: Arc<std::sync::Mutex<usize>>,
    item_output_lease: Option<OutputLease>,
    inputs: Arc<super::inputs::AdmittedInputs>,
    target_node_id: Option<String>,
    publisher: NodeEventPublisher,
    outputs: BTreeMap<String, Value>,
    /// Terminal status of every node that has been visited.  Branch edges are
    /// activated from this state rather than from the static topological order
    /// alone; a skipped predecessor must not make its exclusive successors run.
    node_statuses: BTreeMap<String, NodeExecutionStatus>,
    step: usize,
    #[allow(dead_code)]
    started_at: Instant,
    cancel: Option<Arc<AtomicBool>>,
    /// Node `think` is executing right now. A media job reports progress from
    /// inside the provider poll loop, which cannot see the graph walk.
    active_node_id: Option<String>,
    pending_tool: Option<PendingNodeTool>,
    pending_media: Option<PendingMediaNode>,
    model: Option<Arc<ModelClient>>,
    model_name: Option<String>,
    knowledge_retriever: Option<Arc<dyn KnowledgeRetriever>>,
    tool_executor: Option<Arc<dyn ToolExecutor>>,
    // The legacy registry adapter has no Kernel batch classification. Keep
    // its existing exclusive semantics when item contexts share it.
    tool_executor_lock: Arc<tokio::sync::Mutex<()>>,
    canvas_bridge: Option<Arc<dyn CanvasModelBridge>>,
    artifact_validator: Option<Arc<dyn CanvasArtifactValidator>>,
    code_bridge: Option<Arc<dyn CodeBridge>>,
    usage: Arc<WorkflowUsage>,
}

impl WorkflowAgent {
    /// Admit inputs against the compiled contract before creating any executor.
    /// Invalid/missing inputs fail here, including for non-start graph roots.
    pub fn new(
        compiled: CompiledWorkflow,
        inputs: Map<String, Value>,
        target_node_id: Option<String>,
        publisher: NodeEventPublisher,
    ) -> Result<Self> {
        let inputs = compiled.input_plan.admit(inputs, &compiled.revision)?;
        Ok(Self::from_shared(
            Arc::new(compiled),
            Arc::new(inputs),
            target_node_id,
            publisher,
        ))
    }

    fn from_shared(
        compiled: Arc<CompiledWorkflow>,
        inputs: Arc<super::inputs::AdmittedInputs>,
        target_node_id: Option<String>,
        publisher: NodeEventPublisher,
    ) -> Self {
        let walk = compiled
            .order
            .iter()
            .copied()
            .filter(|index| {
                super::iteration::scope_of(
                    &compiled.iteration_plans,
                    &compiled.definition.nodes[*index].id,
                )
                .is_empty()
            })
            .collect();
        Self {
            compiled,
            walk,
            item: None,
            item_roots: BTreeSet::new(),
            item_lanes: super::iteration::MAX_CONCURRENCY,
            pending_iteration: None,
            item_output_budget: Arc::new(std::sync::Mutex::new(0)),
            item_output_lease: None,
            inputs,
            target_node_id,
            usage: Arc::new(WorkflowUsage::new(publisher.clone())),
            publisher,
            outputs: BTreeMap::new(),
            node_statuses: BTreeMap::new(),
            step: 0,
            started_at: Instant::now(),
            cancel: None,
            active_node_id: None,
            pending_tool: None,
            pending_media: None,
            model: None,
            model_name: None,
            knowledge_retriever: None,
            tool_executor: None,
            tool_executor_lock: Arc::new(tokio::sync::Mutex::new(())),
            canvas_bridge: None,
            artifact_validator: None,
            code_bridge: None,
        }
    }

    /// Attach a cancellation flag so the agent can abort mid-execution.
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Attach a model client so LLM/Agent nodes can make real provider calls.
    pub fn with_model(mut self, client: Arc<ModelClient>, model_name: String) -> Self {
        self.model = Some(client);
        self.model_name = Some(model_name);
        self
    }

    /// Measured calls grouped by actual route, for the existing cost finalizer.
    pub fn model_usage(&self) -> Vec<super::WorkflowModelUsage> {
        self.usage.models()
    }

    /// Bind physical calls to the host's existing admission and cost owner.
    pub fn with_call_usage_sink(self, sink: Arc<dyn crate::usage::CallUsageSink>) -> Self {
        self.usage.bind_sink(sink);
        self
    }

    /// Attach a knowledge retriever so knowledge-retrieval nodes can search.
    pub fn with_knowledge_retriever(mut self, retriever: Arc<dyn KnowledgeRetriever>) -> Self {
        self.knowledge_retriever = Some(retriever);
        self
    }

    /// Attach the canvas model bridge so nodes can call the provider model the
    /// user configured in the canvas settings (database-backed). Without it,
    /// model nodes keep using the run-level chat model.
    pub fn with_canvas_bridge(mut self, bridge: Arc<dyn CanvasModelBridge>) -> Self {
        self.canvas_bridge = Some(bridge);
        self
    }

    /// Attach the app-owned artifact validator used by media authoring nodes.
    pub fn with_artifact_validator(mut self, validator: Arc<dyn CanvasArtifactValidator>) -> Self {
        self.artifact_validator = Some(validator);
        self
    }

    /// Attach a tool executor so tool nodes can execute registered tools.
    pub fn with_tool_executor(mut self, executor: Arc<dyn ToolExecutor>) -> Self {
        self.tool_executor = Some(executor);
        self
    }

    pub fn with_code_bridge(mut self, bridge: Arc<dyn CodeBridge>) -> Self {
        self.code_bridge = Some(bridge);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    fn cancel_active_node(&mut self) {
        let Some(node_id) = self.active_node_id.take() else {
            return;
        };
        if self.node_statuses.get(&node_id) != Some(&NodeExecutionStatus::Running) {
            return;
        }
        self.node_statuses
            .insert(node_id.clone(), NodeExecutionStatus::Cancelled);
        self.emit_status(
            &node_id,
            NodeExecutionStatus::Cancelled,
            1,
            0,
            None,
            Some("workflow cancelled while the node was in flight".to_string()),
        );
    }

    fn base_event(&self, node_id: &str) -> NodeExecutionEvent {
        NodeExecutionEvent::new(self.compiled.revision.clone(), node_id.to_string())
    }

    fn emit_status(
        &self,
        node_id: &str,
        status: NodeExecutionStatus,
        attempt: u32,
        elapsed_ms: u64,
        outputs: Option<Value>,
        error: Option<String>,
    ) {
        self.publisher.emit(NodeExecutionEvent {
            status,
            attempt,
            elapsed_ms,
            outputs,
            error,
            ..self.base_event(node_id)
        });
    }

    /// Whether at least one incoming edge has been activated by the results
    /// already produced in this walk.  A node with no incoming edges retains
    /// the historical root-node behavior; connected nodes require a completed
    /// predecessor and, for an `if-else` or question-classifier predecessor,
    /// the matching source handle.
    fn has_active_predecessor(&self, node_id: &str) -> bool {
        if self.item_roots.contains(node_id) {
            return true;
        }
        let incoming = self
            .compiled
            .definition
            .edges
            .iter()
            .filter(|edge| edge.target == node_id)
            .collect::<Vec<_>>();
        if incoming.is_empty() {
            return true;
        }

        incoming.into_iter().any(|edge| {
            if self.node_statuses.get(&edge.source) != Some(&NodeExecutionStatus::Completed) {
                return false;
            }
            let source_kind = self
                .compiled
                .definition
                .nodes
                .iter()
                .find(|node| node.id == edge.source)
                .map(|node| node.kind.as_str());
            if source_kind == Some("iteration") {
                return edge.source_handle.as_deref() == Some("done");
            }
            if !matches!(source_kind, Some("if-else") | Some("question-classifier")) {
                return true;
            }
            let Some(handle) = edge.source_handle.as_deref() else {
                return false;
            };
            let output_key = if source_kind == Some("question-classifier") {
                "__branch"
            } else {
                "branch"
            };
            self.outputs
                .get(&edge.source)
                .and_then(|output| output.get(output_key))
                .and_then(Value::as_str)
                == Some(handle)
        })
    }

    /// Sink handed to a media job so the provider poll loop can report the
    /// node's job id and phase while it is still running.
    fn progress_sink(&self) -> Option<CanvasJobProgressSink> {
        let node_id = self.active_node_id.clone()?;
        let publisher = self.publisher.clone();
        let revision = self.compiled.revision.clone();
        Some(CanvasJobProgressSink::new(move |progress| {
            publisher.emit_job_progress(
                &revision,
                &node_id,
                &progress.job_id,
                &progress.resume_fingerprint,
                progress.phase.as_str(),
            );
        }))
    }

    fn resolve_config(&self, config: &Map<String, Value>) -> Result<Map<String, Value>> {
        config
            .iter()
            .map(|(key, value)| Ok((key.clone(), values::resolve(value, &self.outputs)?)))
            .collect()
    }

    async fn execute_node_inline(
        &mut self,
        kind: &str,
        config: &Map<String, Value>,
    ) -> Result<Value> {
        // These executors own their single resolution/rendering pass. A generic
        // pre-pass would reinterpret inserted upstream text as executable syntax.
        match kind {
            "start" => return Ok(self.inputs.output.clone()),
            "answer" => return self.execute_answer(config),
            "end" => return self.execute_end(config),
            _ => {}
        }
        let resolved_config = self.resolve_config(config)?;

        match kind {
            "variable-assigner" => self.execute_variable_assigner(&resolved_config),
            "if-else" => self.execute_if_else(&resolved_config),
            "template-transform" => self.execute_template_transform(&resolved_config),
            "list-operator" => self.execute_list_operator(&resolved_config),
            "question-classifier" => self
                .execute_question_classifier(&resolved_config)
                .await,
            "parameter-extractor" => self
                .execute_parameter_extractor(&resolved_config)
                .await,
            // Agent V2 advertises tools, memory and declared outputs, but the
            // Canvas bridge currently exposes a single completion only. Do
            // not silently execute that request as a one-shot LLM: keeping
            // the node explicitly reserved is safer than claiming semantics
            // the runtime cannot provide (tool loop, approval, cancellation,
            // memory and structured output mapping).
            "agent-v2" => Err(CoreError::invalid(
                "UnsupportedOperation: canvas node `agent-v2` is reserved until its tool, approval, cancellation, memory, and structured-output executor is available",
            )),
            "llm" | "agent" | "text-gen" | "script-gen" | "director"
            | "creative-template" | "storyboard-grid" | "category-picker" | "character-face"
            | "character-body" | "character-style" => {
                self.execute_llm(kind, &resolved_config).await
            }
            "image-gen" | "image-edit" => self.execute_image_gen(kind, &resolved_config).await,
            "audio" => self.execute_audio(&resolved_config).await,
            "video-gen" => self.execute_video(&resolved_config).await,
            "video-edit" => self.execute_video_edit(&resolved_config),
            "embeddings" => self.execute_embedding(&resolved_config).await,
            // Media kinds without a backend job runner fail loudly instead of
            // reporting a fabricated completion through the passthrough arm.
            "video-stitch" | "image-compare" => Err(deepagent_core::error::CoreError::invalid(
                format!("UnsupportedOperation: canvas node `{kind}` has no executor yet"),
            )),
            "tool" => self.execute_tool(&resolved_config).await,
            "knowledge-retrieval" => self.execute_knowledge_retrieval(&resolved_config).await,
            "loop" => self.execute_iteration(&resolved_config).await,
            // These nodes have nothing to compute: an input node's own config
            // *is* its output, the loop markers only carry their context, and
            // the ContractOnly constraint nodes exist to hold their settings.
            "iteration-start" => self.item.as_ref().map(|item| json!({"item":item}))
                .ok_or_else(|| CoreError::invalid("iteration-start has no active item scope")),
            "image-input" | "loop-start" | "loop-end" | "camera" | "lens"
            | "focal-length" | "aperture" => Ok(Value::Object(resolved_config)),
            // Anything else must fail loudly. Forwarding its config would make
            // an unimplemented node look like it had produced real results.
            _ => Err(deepagent_core::error::CoreError::invalid(format!(
                "UnsupportedNode: kernel has no executor for node kind `{kind}`"
            ))),
        }
    }

    fn execute_video_edit(&self, config: &Map<String, Value>) -> Result<Value> {
        let output = config
            .get("videoEditOutputRef")
            .and_then(Value::as_str)
            .filter(|reference| reference.starts_with("artifact://"))
            .ok_or_else(|| {
                CoreError::invalid(
                    "UnsupportedOperation: video-edit has no exported artifact reference",
                )
            })?;
        if config.get("videoEditStatus").and_then(Value::as_str) != Some("exported") {
            return Err(CoreError::invalid(
                "UnsupportedOperation: video-edit output is not marked as exported",
            ));
        }
        let inputs = config
            .get("videoEditInputRefs")
            .and_then(Value::as_array)
            .filter(|refs| !refs.is_empty())
            .ok_or_else(|| {
                CoreError::invalid("MissingReferenceInput: video-edit requires a source artifact")
            })?;
        let validator = self.artifact_validator.as_ref().ok_or_else(|| {
            CoreError::invalid(
                "ArtifactValidationUnavailable: video-edit cannot publish without artifact validation",
            )
        })?;
        let mut input_refs = Vec::with_capacity(inputs.len());
        for input in inputs {
            let reference = input
                .as_str()
                .filter(|value| value.starts_with("artifact://"))
                .ok_or_else(|| {
                    CoreError::invalid(
                        "InvalidReferenceInput: video-edit source must be an artifact",
                    )
                })?;
            validator.validate_video_artifact(reference)?;
            input_refs.push(reference);
        }
        validator.validate_video_artifact(output)?;
        let start = config
            .get("videoEditTrimStart")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                CoreError::invalid("InvalidConfiguration: video-edit trim start is missing")
            })?;
        let end = config
            .get("videoEditTrimEnd")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > start)
            .ok_or_else(|| {
                CoreError::invalid("InvalidConfiguration: video-edit trim end is missing")
            })?;
        let expected_fingerprint = format!("{}:{start:.3}:{end:.3}", input_refs.join("|"));
        if config
            .get("videoEditInputFingerprint")
            .and_then(Value::as_str)
            != Some(expected_fingerprint.as_str())
        {
            return Err(CoreError::invalid(
                "StaleArtifact: video-edit output no longer matches its source and trim range",
            ));
        }
        Ok(serde_json::json!({
            "videoUrl": output,
            "videoEditOutputRef": output,
            "videoEditInputFingerprint": expected_fingerprint,
        }))
    }

    /// Merge upstream node text into a node's own prompt.
    ///
    /// Creative nodes carry their inputs on edges, which the frontend serializes
    /// as `upstreamTexts` references; the generic value resolver has already
    /// turned those into upstream values by the time this runs. The text and
    /// image executors share this one implementation.
    fn upstream_texts(config: &Map<String, Value>) -> Vec<String> {
        config
            .get("upstreamTexts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match item {
                        Value::String(text) => Some(text.clone()),
                        Value::Number(number) => Some(number.to_string()),
                        Value::Bool(flag) => Some(flag.to_string()),
                        other => other
                            .get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| serde_json::to_string_pretty(other).ok()),
                    })
                    .filter(|text| !text.trim().is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn compose_prompt(config: &Map<String, Value>, own_prompt: &str) -> String {
        let upstream = Self::upstream_texts(config);
        if upstream.is_empty() {
            return own_prompt.to_string();
        }
        if own_prompt.trim().is_empty() {
            return format!("上游内容：\n{}", upstream.join("\n---\n"));
        }
        format!(
            "上游内容：\n{}\n\n当前要求：\n{}",
            upstream.join("\n---\n"),
            own_prompt
        )
    }

    fn execute_end(&mut self, config: &Map<String, Value>) -> Result<Value> {
        let mut result = Map::new();
        if let Some(variables) = config.get("outputVariables").and_then(Value::as_array) {
            for var in variables {
                let name = var
                    .get("name")
                    .and_then(Value::as_str)
                    .or_else(|| var.get("variable").and_then(Value::as_str))
                    .unwrap_or_default();
                let value = var.get("value").cloned().unwrap_or(Value::Null);
                if !name.is_empty() {
                    let resolved = values::resolve(&value, &self.outputs)?;
                    result.insert(name.to_string(), resolved);
                }
            }
        }
        Ok(Value::Object(result))
    }

    fn execute_answer(&self, config: &Map<String, Value>) -> Result<Value> {
        super::answer::AnswerConfig::parse(config)?.render(&self.outputs)
    }

    fn execute_variable_assigner(&self, config: &Map<String, Value>) -> Result<Value> {
        let assignments = config
            .get("assignments")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut result = Map::new();
        for assignment in assignments {
            let variable = assignment
                .get("variable")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input_type = assignment
                .get("inputType")
                .and_then(Value::as_str)
                .unwrap_or("constant");
            let value = match input_type {
                "variable" => {
                    let var_ref = assignment.get("value").cloned().unwrap_or(Value::Null);
                    values::resolve(&var_ref, &self.outputs)?
                }
                _ => assignment.get("value").cloned().unwrap_or(Value::Null),
            };
            if !variable.is_empty() {
                result.insert(variable.to_string(), value);
            }
        }
        Ok(Value::Object(result))
    }

    fn execute_if_else(&self, config: &Map<String, Value>) -> Result<Value> {
        let conditions = config
            .get("conditions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let logical_operator = config
            .get("logicalOperator")
            .and_then(Value::as_str)
            .unwrap_or("and");

        for group in conditions {
            let group_id = group.get("id").and_then(Value::as_str).unwrap_or("default");
            let items = group
                .get("conditions")
                .and_then(Value::as_array)
                .or_else(|| group.get("items").and_then(Value::as_array))
                .cloned()
                .unwrap_or_default();
            let group_op = group
                .get("logicalOperator")
                .and_then(Value::as_str)
                .or_else(|| group.get("logic").and_then(Value::as_str))
                .unwrap_or("and");

            let mut group_result = true;
            for item in items {
                let left = item
                    .get("left")
                    .cloned()
                    .or_else(|| item.get("variable").cloned())
                    .unwrap_or(Value::Null);
                let operator = item.get("operator").and_then(Value::as_str).unwrap_or("is");
                let right = item
                    .get("right")
                    .cloned()
                    .or_else(|| item.get("value").cloned())
                    .unwrap_or(Value::Null);

                let left_resolved = values::resolve(&left, &self.outputs)?;
                let right_resolved = values::resolve(&right, &self.outputs)?;
                let cmp = values::compare(&left_resolved, operator, &right_resolved)?;

                group_result = match group_op {
                    "or" => group_result || cmp,
                    _ => group_result && cmp,
                };
            }

            if group_result {
                return Ok(serde_json::json!({ "__branch": group_id }));
            }
        }

        let _ = logical_operator;
        Ok(serde_json::json!({ "__branch": "false" }))
    }

    fn execute_template_transform(&self, config: &Map<String, Value>) -> Result<Value> {
        let template = config
            .get("templateScript")
            .or_else(|| config.get("template"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let variables = config
            .get("variables")
            .and_then(Value::as_object)
            .cloned()
            .or_else(|| {
                config
                    .get("templateInputVariables")
                    .and_then(Value::as_array)
                    .map(|bindings| {
                        bindings.iter().fold(Map::new(), |mut variables, binding| {
                            if let Some(name) = binding.get("name").and_then(Value::as_str) {
                                if let Some(value) = binding.get("value") {
                                    variables.insert(name.to_owned(), value.clone());
                                }
                            }
                            variables
                        })
                    })
            })
            .unwrap_or_default();
        let rendered = values::render_template(template, &variables, &self.outputs)?;
        Ok(serde_json::json!({ "output": rendered }))
    }

    fn execute_list_operator(&self, config: &Map<String, Value>) -> Result<Value> {
        let input = config
            .get("input")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        let action = config
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("passthrough");
        let condition = config
            .get("condition")
            .and_then(Value::as_str)
            .unwrap_or("");
        let field = config.get("field").and_then(Value::as_str).unwrap_or("");
        let order_by = config
            .get("orderBy")
            .and_then(Value::as_str)
            .unwrap_or("asc");
        let limit = config.get("limit").and_then(Value::as_u64).unwrap_or(0) as usize;
        let resolved_input = values::resolve(&input, &self.outputs)?;
        let result =
            values::operate_list(&resolved_input, action, condition, field, order_by, limit)?;
        Ok(result)
    }

    fn is_professional_model_node(kind: &str) -> bool {
        matches!(
            kind,
            "llm" | "agent" | "agent-v2" | "question-classifier" | "parameter-extractor"
        )
    }

    fn value_to_prompt(value: Option<&Value>) -> String {
        match value {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }

    fn parse_json_object(raw: &str, context: &str) -> Result<Map<String, Value>> {
        let trimmed = raw.trim();
        let unfenced = if let Some(rest) = trimmed.strip_prefix("```") {
            // Models commonly emit ```json\n{...}\n```; the language tag is
            // metadata, not part of the JSON payload. Only remove a fence when
            // the response actually starts with one, so ordinary prose keeps
            // the bounded object-extraction fallback below.
            let body = rest
                .strip_prefix("json")
                .or_else(|| rest.strip_prefix("JSON"))
                .unwrap_or(rest);
            let body = body.strip_prefix('\n').unwrap_or(body);
            body.strip_suffix("```").unwrap_or(body).trim()
        } else {
            trimmed
        };
        let candidate = serde_json::from_str::<Value>(unfenced)
            .ok()
            .or_else(|| {
                let start = unfenced.find('{')?;
                let end = unfenced.rfind('}')?;
                (start < end).then(|| serde_json::from_str::<Value>(&unfenced[start..=end]).ok())?
            })
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "InvalidModelOutput: {context} must return one JSON object"
                ))
            })?;
        candidate.as_object().cloned().ok_or_else(|| {
            CoreError::invalid(format!(
                "InvalidModelOutput: {context} must return a JSON object"
            ))
        })
    }

    fn model_completion_config(
        config: &Map<String, Value>,
        model_ref: Option<&Value>,
        prompt: String,
        system_prompt: String,
    ) -> Map<String, Value> {
        let mut completion = config.clone();
        if let Some(model_ref) = model_ref {
            completion.insert("llmModel".to_string(), model_ref.clone());
        }
        completion.insert("llmPrompt".to_string(), Value::String(prompt));
        completion.insert("llmSystemPrompt".to_string(), Value::String(system_prompt));
        completion
    }

    async fn execute_question_classifier(&mut self, config: &Map<String, Value>) -> Result<Value> {
        let classes = config
            .get("classifierClasses")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .ok_or_else(|| {
                CoreError::invalid(
                    "InvalidConfiguration: question-classifier requires at least one class",
                )
            })?;
        let class_lines = classes
            .iter()
            .enumerate()
            .map(|(index, class)| {
                let name = class
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| {
                        CoreError::invalid(format!(
                            "InvalidConfiguration: question-classifier class {index} has no name"
                        ))
                    })?;
                let description = class
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Ok(if description.trim().is_empty() {
                    format!("{}. {}", index + 1, name)
                } else {
                    format!("{}. {} — {}", index + 1, name, description)
                })
            })
            .collect::<Result<Vec<_>>>()?
            .join("\n");
        let input = Self::value_to_prompt(config.get("classifierInput"));
        let instruction = Self::value_to_prompt(config.get("classifierInstruction"));
        let prompt = format!(
            "{}\n\n将输入归入一个类别，只输出类别编号（1-{}），不要输出解释。\n类别：\n{}\n\n输入：\n{}",
            if instruction.trim().is_empty() {
                "请对输入进行分类。"
            } else {
                instruction.as_str()
            },
            classes.len(),
            class_lines,
            input
        );
        let completion = Self::model_completion_config(
            config,
            config.get("classifierModel"),
            prompt,
            "你是严格的工作流分类器，只能返回一个有效类别编号。".to_string(),
        );
        let response = self.execute_llm("question-classifier", &completion).await?;
        let raw = response
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let index = raw
            .parse::<usize>()
            .ok()
            .or_else(|| {
                Self::parse_json_object(raw, "question-classifier")
                    .ok()
                    .and_then(|object| {
                        object
                            .get("index")
                            .or_else(|| object.get("class_index"))
                            .and_then(Value::as_u64)
                            .map(|value| value as usize)
                    })
            })
            .filter(|index| (1..=classes.len()).contains(index))
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "InvalidModelOutput: question-classifier returned invalid class index `{raw}`"
                ))
            })?
            - 1;
        let class = &classes[index];
        let class_name = class
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let class_label = class
            .get("description")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
            .unwrap_or(&class_name)
            .to_string();
        Ok(serde_json::json!({
            "class_name": class_name,
            "class_label": class_label,
            "usage": response.get("usage").cloned().unwrap_or(Value::Null),
            "providerId": response.get("providerId").cloned().unwrap_or(Value::Null),
            "modelId": response.get("modelId").cloned().unwrap_or(Value::Null),
            "__branch": format!("class-{index}"),
        }))
    }

    fn coerce_parameter_value(
        value: Option<&Value>,
        type_name: &str,
        required: bool,
        name: &str,
    ) -> Result<Value> {
        let value = value.cloned().unwrap_or(Value::Null);
        if value.is_null() {
            if required {
                return Err(CoreError::invalid(format!(
                    "InvalidModelOutput: parameter-extractor missing required field `{name}`"
                )));
            }
            return Ok(Value::Null);
        }
        match type_name {
            "number" => value
                .as_f64()
                .filter(|number| number.is_finite())
                .map(|number| json!(number))
                .or_else(|| {
                    value
                        .as_str()?
                        .parse::<f64>()
                        .ok()
                        .map(|number| json!(number))
                })
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "InvalidModelOutput: parameter `{name}` is not a number"
                    ))
                }),
            "boolean" => value
                .as_bool()
                .map(Value::Bool)
                .or_else(|| {
                    value.as_str().and_then(|text| {
                        match text.trim().to_ascii_lowercase().as_str() {
                            "true" | "1" => Some(Value::Bool(true)),
                            "false" | "0" => Some(Value::Bool(false)),
                            _ => None,
                        }
                    })
                })
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "InvalidModelOutput: parameter `{name}` is not boolean"
                    ))
                }),
            "object" if !value.is_object() => Err(CoreError::invalid(format!(
                "InvalidModelOutput: parameter `{name}` is not an object"
            ))),
            "array" if !value.is_array() => Err(CoreError::invalid(format!(
                "InvalidModelOutput: parameter `{name}` is not an array"
            ))),
            "string" => Ok(Value::String(match value {
                Value::String(text) => text,
                other => other.to_string(),
            })),
            _ => Ok(value),
        }
    }

    async fn execute_parameter_extractor(&mut self, config: &Map<String, Value>) -> Result<Value> {
        let params = config
            .get("extractorParams")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .ok_or_else(|| {
                CoreError::invalid(
                    "InvalidConfiguration: parameter-extractor requires at least one parameter",
                )
            })?;
        let mut names = BTreeSet::new();
        let declarations = params
            .iter()
            .map(|parameter| {
                let name = parameter
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| {
                        CoreError::invalid(
                            "InvalidConfiguration: parameter-extractor parameter has no name",
                        )
                    })?;
                if !names.insert(name.to_string()) {
                    return Err(CoreError::invalid(format!(
                        "InvalidConfiguration: parameter-extractor has duplicate parameter `{name}`"
                    )));
                }
                let type_name = parameter
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("string");
                if !matches!(type_name, "string" | "number" | "boolean" | "object" | "array" | "file")
                {
                    return Err(CoreError::invalid(format!(
                        "InvalidConfiguration: parameter-extractor parameter `{name}` has unsupported type `{type_name}`"
                    )));
                }
                let required = parameter
                    .get("required")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let description = parameter
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                Ok(format!(
                    "- {name}{}: {type_name}{}",
                    if required { "（必填）" } else { "" },
                    if description.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" — {description}")
                    }
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join("\n");
        let input = Self::value_to_prompt(config.get("extractorInput"));
        let instruction = Self::value_to_prompt(config.get("extractorInstruction"));
        let prompt = format!(
            "{}\n\n从输入中提取字段，只输出一个 JSON 对象，键必须严格使用下列声明，不要输出 Markdown 或解释。\n字段：\n{}\n\n输入：\n{}",
            if instruction.trim().is_empty() {
                "请提取所需参数。"
            } else {
                instruction.as_str()
            },
            declarations,
            input
        );
        let completion = Self::model_completion_config(
            config,
            config.get("extractorModel"),
            prompt,
            "你是严格的工作流参数提取器，只能返回符合字段声明的 JSON 对象。".to_string(),
        );
        let response = self.execute_llm("parameter-extractor", &completion).await?;
        let parsed = Self::parse_json_object(
            response
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            "parameter-extractor",
        )?;
        if let Some(unknown) = parsed.keys().find(|name| {
            !params.iter().any(|parameter| {
                parameter.get("name").and_then(Value::as_str) == Some(name.as_str())
            })
        }) {
            return Err(CoreError::invalid(format!(
                "InvalidModelOutput: parameter-extractor returned undeclared field `{unknown}`"
            )));
        }
        let mut output = Map::new();
        for parameter in params {
            let name = parameter
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let type_name = parameter
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("string");
            let required = parameter
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            output.insert(
                name.to_string(),
                Self::coerce_parameter_value(parsed.get(name), type_name, required, name)?,
            );
        }
        output.insert("__is_success".to_string(), json!(1));
        output.insert("__reason".to_string(), Value::String(String::new()));
        output.insert(
            "__usage".to_string(),
            response.get("usage").cloned().unwrap_or(Value::Null),
        );
        output.insert(
            "providerId".to_string(),
            response.get("providerId").cloned().unwrap_or(Value::Null),
        );
        output.insert(
            "modelId".to_string(),
            response.get("modelId").cloned().unwrap_or(Value::Null),
        );
        Ok(Value::Object(output))
    }

    async fn execute_llm(&mut self, kind: &str, config: &Map<String, Value>) -> Result<Value> {
        let model_ref = config
            .get("llmModel")
            .or_else(|| config.get("model"))
            .or_else(|| config.get("agentV2Model"))
            .or_else(|| config.get("classifierModel"))
            .or_else(|| config.get("extractorModel"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let user_prompt = config
            .get("llmPrompt")
            .or_else(|| config.get("agentV2Task"))
            .or_else(|| config.get("agentTask"))
            .or_else(|| config.get("prompt"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let system_prompt = config
            .get("llmSystemPrompt")
            .or_else(|| config.get("agentSystemPrompt"))
            .or_else(|| config.get("agentV2SystemPrompt"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let composed_prompt = Self::compose_prompt(config, &user_prompt);

        if let Some(bridge) = self.canvas_bridge.clone() {
            // The canvas bridge owns both explicit providerId::modelId routes
            // and the empty automatic route. Do not fall back to the generic
            // client merely because the model reference is empty: that would
            // make a new canvas node silently leave its configured scenario.
            let response = bridge
                .complete_with_usage(
                    CanvasCompletionRequest {
                        model_ref: model_ref.clone(),
                        node_kind: kind.to_string(),
                        prompt_policy: if Self::is_professional_model_node(kind) {
                            CanvasPromptPolicy::Professional
                        } else {
                            CanvasPromptPolicy::CreativeProfile
                        },
                        skill_ids: config
                            .get("skillIds")
                            .or_else(|| config.get("skill_ids"))
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        system_prompt: (!system_prompt.is_empty()).then(|| system_prompt.clone()),
                        prompt: composed_prompt.clone(),
                        temperature: config
                            .get("llmTemperature")
                            .or_else(|| config.get("temperature"))
                            .and_then(Value::as_f64)
                            .map(|value| value as f32),
                        max_tokens: config
                            .get("llmMaxTokens")
                            .or_else(|| config.get("maxTokens"))
                            .and_then(Value::as_u64)
                            .map(|value| value as u32),
                        images: Vec::new(),
                    },
                    self.usage.clone(),
                )
                .await?;
            let mut outcome = serde_json::json!({
                "text": response.text,
                "usage": response.usage,
                "providerId": response.provider_id,
                "modelId": response.model_id,
                "model": if model_ref.is_empty() { response.model_id.clone() } else { model_ref },
            });
            if let Some(reasoning) = response.reasoning {
                outcome["reasoning"] = serde_json::Value::String(reasoning);
            }
            return Ok(outcome);
        }

        let client = match &self.model {
            Some(c) => c.clone(),
            None => {
                return Ok(serde_json::json!({
                    "text": format!("[{}] no model client configured for workflow run", kind),
                    "usage": null
                }));
            }
        };

        let model_name = self
            .model_name
            .clone()
            .unwrap_or_else(|| "deepseek-chat".into());

        let temperature = config
            .get("llmTemperature")
            .and_then(Value::as_f64)
            .map(|v| v as f32);

        let max_tokens = config
            .get("llmMaxTokens")
            .and_then(Value::as_u64)
            .map(|v| v as u32);

        let mut messages = Vec::new();
        if !system_prompt.is_empty() {
            messages.push(Message::system(system_prompt));
        }
        messages.push(Message::user(user_prompt));

        let mut request = ChatCompletionRequest::new(&model_name, messages);
        if let Some(t) = temperature {
            request.temperature = Some(t);
        }
        if let Some(m) = max_tokens {
            request.max_tokens = Some(m);
        }

        crate::usage::CallUsageSink::before_call(self.usage.as_ref())?;
        let mut observer = WorkflowUsageObserver::new(self.usage.clone(), model_name, true);
        let response = client
            .stream_chat_completion_observed(request, &mut observer)
            .await;
        drop(observer);
        crate::usage::CallUsageSink::checkpoint(self.usage.as_ref())?;
        let response = response?;
        let text = response.output_text_projection();

        Ok(serde_json::json!({
            "text": text,
            "usage": response.usage
        }))
    }

    /// Build the typed request for a canvas image node. Whether the call
    /// generates a new image or edits a reference image is decided by the
    /// resolved inputs, exactly like the deterministic router prescribes.
    fn image_request(
        &self,
        node_kind: &str,
        config: &Map<String, Value>,
    ) -> Result<CanvasImageRequest> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other(
                "image nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        let prompt = config
            .get("prompt")
            .or_else(|| config.get("imagePrompt"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let reference_images: Vec<String> = config
            .get("referenceImages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.as_str()
                            .map(str::to_string)
                            .or_else(|| item.get("url").and_then(Value::as_str).map(str::to_string))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let prompt = Self::compose_prompt(config, &prompt);
        if prompt.trim().is_empty() {
            return Err(deepagent_core::error::CoreError::invalid(
                "MissingReferenceInput: image node requires a prompt",
            ));
        }
        // Route on what the referenced values actually are, not on which config
        // array they happened to arrive in.
        let reference_kinds = bridge.inspect_input_kinds(&reference_images)?;
        let mut input_kinds = vec!["Text".to_string()];
        for kind in reference_kinds {
            if kind == "Unknown" {
                return Err(deepagent_core::error::CoreError::invalid(format!(
                    "MissingReferenceInput: `{node_kind}` received a reference the store cannot classify"
                )));
            }
            if !input_kinds.contains(&kind) {
                input_kinds.push(kind);
            }
        }
        let routed = bridge.route_operation(CanvasRouteRequest {
            node_kind: node_kind.to_string(),
            explicit_operation: config
                .get("operation")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty() && value.trim() != "auto"),
            input_kinds,
            has_prompt: true,
        })?;
        let _ = bridge;
        Ok(CanvasImageRequest {
            model_ref: config
                .get("imageModel")
                .or_else(|| config.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            prompt,
            reference_images,
            size: config
                .get("size")
                .and_then(Value::as_str)
                .map(str::to_string),
            operation: routed.operation.clone(),
        })
    }

    fn image_response_value(response: CanvasImageResponse) -> Value {
        serde_json::json!({
            // The node keeps its existing `imageUrl` field, but the value is an
            // `artifact://` reference: no base64 enters events or the graph.
            "imageUrl": response.artifact_uri,
            "mime": response.mime,
            "operation": response.operation,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        })
    }

    async fn execute_image_gen(
        &self,
        node_kind: &str,
        config: &Map<String, Value>,
    ) -> Result<Value> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other(
                "image nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        let response = bridge
            .generate_image(self.image_request(node_kind, config)?)
            .await?;
        Ok(Self::image_response_value(response))
    }

    async fn prepare_image_gen(
        &self,
        node_kind: &str,
        config: &Map<String, Value>,
    ) -> Result<CanvasImagePreparation> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other(
                "image nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        bridge
            .prepare_image(self.image_request(node_kind, config)?)
            .await
    }

    fn build_video_request(
        &self,
        config: &Map<String, Value>,
    ) -> Result<(CanvasVideoRequest, String)> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other(
                "video nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        let prompt = Self::compose_prompt(
            config,
            config
                .get("prompt")
                .or_else(|| config.get("videoPrompt"))
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let image_url = config
            .get("videoInputUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let mut input_kinds = Vec::new();
        if !prompt.trim().is_empty() {
            input_kinds.push("Text".to_string());
        }
        if let Some(reference) = image_url.clone() {
            match bridge
                .inspect_input_kinds(&[reference])?
                .first()
                .map(String::as_str)
            {
                Some("Image") => input_kinds.push("Image".to_string()),
                Some(other) => {
                    return Err(deepagent_core::error::CoreError::invalid(format!(
                        "OperationInputConflict: `video-gen` first frame must be an image, got `{other}`"
                    )))
                }
                None => {
                    return Err(deepagent_core::error::CoreError::invalid(
                        "MissingReferenceInput: `video-gen` received a reference the store cannot classify",
                    ))
                }
            }
        }
        if input_kinds.is_empty() {
            return Err(deepagent_core::error::CoreError::invalid(
                "MissingReferenceInput: video node requires a prompt or an input frame",
            ));
        }
        let routed = bridge.route_operation(CanvasRouteRequest {
            node_kind: "video-gen".to_string(),
            explicit_operation: config
                .get("operation")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty() && value.trim() != "auto"),
            input_kinds,
            has_prompt: !prompt.trim().is_empty(),
        })?;
        if routed.operation != "video_generate" {
            return Err(deepagent_core::error::CoreError::invalid(format!(
                "UnsupportedOperation: no provider job is wired for video operation `{}`",
                routed.operation
            )));
        }
        Ok((
            CanvasVideoRequest {
                model_ref: config
                    .get("videoModel")
                    .or_else(|| config.get("model"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                prompt,
                image_url,
                size: config
                    .get("videoResolution")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty()),
                resume_task_id: config
                    .get("videoTaskId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty()),
                resume_fingerprint: config
                    .get("videoTaskFingerprint")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty()),
                timeout_ms: 600_000,
                cancel: self.cancel.clone(),
                on_progress: self.progress_sink(),
            },
            routed.operation,
        ))
    }

    /// Video node: one asynchronous provider job, resumable by task id.
    async fn execute_video(&self, config: &Map<String, Value>) -> Result<Value> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            CoreError::other(
                "video nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        let (request, operation) = self.build_video_request(config)?;
        let response = bridge.generate_video(request).await?;
        Ok(serde_json::json!({
            "videoUrl": response.artifact_uri,
            "videoTaskId": response.task_id,
            "videoTaskFingerprint": response.resume_fingerprint,
            "mime": response.mime,
            "operation": operation,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        }))
    }

    async fn prepare_video_gen(
        &self,
        config: &Map<String, Value>,
    ) -> Result<CanvasVideoPreparation> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            CoreError::other(
                "video nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        let (request, _operation) = self.build_video_request(config)?;
        bridge.prepare_video(request).await
    }
    fn audio_reference(config: &Map<String, Value>) -> Result<Option<String>> {
        // 节点自选的文件与上游连线送入的都是同一路音频；契约要求恰好一路。
        let edge_audios: Vec<String> = config
            .get("audioInputs")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut references = config
            .get("audioReference")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .into_iter()
            .chain(edge_audios)
            .collect::<Vec<_>>();
        match references.len() {
            0 => Ok(None),
            1 => Ok(Some(references.remove(0))),
            count => Err(deepagent_core::error::CoreError::invalid(format!(
                "OperationInputConflict: `audio` node takes exactly one audio input, got {count}"
            ))),
        }
    }

    /// Audio node: either direction of speech, never guessed by the node.
    ///
    /// The contract requires an explicit operation, so the only fact this
    /// executor adds to the route is what the referenced value really is.
    async fn execute_audio(&self, config: &Map<String, Value>) -> Result<Value> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other(
                "audio nodes need the canvas model bridge; configure the canvas providers first",
            )
        })?;
        let text = Self::compose_prompt(
            config,
            config
                .get("prompt")
                .or_else(|| config.get("text"))
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let audio_reference = Self::audio_reference(config)?;
        let mut input_kinds = Vec::new();
        if !text.trim().is_empty() {
            input_kinds.push("Text".to_string());
        }
        if let Some(reference) = audio_reference.clone() {
            match bridge
                .inspect_input_kinds(&[reference])?
                .first()
                .map(String::as_str)
            {
                Some("Audio") => input_kinds.push("Audio".to_string()),
                Some(other) => {
                    return Err(deepagent_core::error::CoreError::invalid(format!(
                        "OperationInputConflict: `audio` node expects an audio reference, got `{other}`"
                    )))
                }
                None => {
                    return Err(deepagent_core::error::CoreError::invalid(
                        "MissingReferenceInput: `audio` received a reference the store cannot classify",
                    ))
                }
            }
        }
        if input_kinds.is_empty() {
            input_kinds.push("Empty".to_string());
        }
        let routed = bridge.route_operation(CanvasRouteRequest {
            node_kind: "audio".to_string(),
            explicit_operation: config
                .get("operation")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.trim().is_empty() && value.trim() != "auto"),
            input_kinds,
            has_prompt: !text.trim().is_empty(),
        })?;
        let response = bridge
            .run_audio(CanvasAudioRequest {
                model_ref: config
                    .get("audioModel")
                    .or_else(|| config.get("model"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                operation: routed.operation.clone(),
                text,
                audio_reference,
                voice: config
                    .get("audioVoice")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty()),
                format: config
                    .get("audioFormat")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty()),
                timeout_ms: 300_000,
            })
            .await?;
        Ok(serde_json::json!({
            "operation": response.operation,
            "audioUrl": response.audio_url,
            "text": response.text,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        }))
    }

    async fn execute_embedding(&self, config: &Map<String, Value>) -> Result<Value> {
        let bridge = self.canvas_bridge.clone().ok_or_else(|| {
            deepagent_core::error::CoreError::other(
                "embedding nodes need the canvas model bridge; configure a vector model first",
            )
        })?;
        let texts: Vec<String> = config
            .get("texts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .or_else(|| {
                config
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|one| vec![one.to_string()])
            })
            .unwrap_or_default();
        let response = bridge
            .embed_with_usage(
                CanvasEmbeddingRequest {
                    model_ref: config
                        .get("embeddingModel")
                        .or_else(|| config.get("model"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    texts,
                },
                self.usage.clone(),
            )
            .await?;
        Ok(serde_json::json!({
            "count": response.count,
            "dimensions": response.dimensions,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        }))
    }

    fn http_tool_arguments(&self, config: &Map<String, Value>) -> Result<Value> {
        let mut arguments = Map::new();
        for (field, name, default) in [
            ("httpUrl", "url", Value::Null),
            ("httpMethod", "method", Value::String("GET".into())),
            ("httpBody", "body", Value::String(String::new())),
            ("httpHeaders", "headers", serde_json::json!({})),
            ("httpTimeout", "timeout_secs", Value::from(30)),
            ("httpRetryCount", "retry_count", Value::from(0)),
        ] {
            arguments.insert(
                name.into(),
                values::resolve(config.get(field).unwrap_or(&default), &self.outputs)?,
            );
        }
        Ok(Value::Object(arguments))
    }
    async fn execute_tool(&self, config: &Map<String, Value>) -> Result<Value> {
        let _exclusive = self.tool_executor_lock.lock().await;
        let executor = match &self.tool_executor {
            Some(e) => e,
            None => {
                return Err(CoreError::invalid("tool executor is not configured"));
            }
        };

        let tool_name = config.get("toolName").and_then(Value::as_str).unwrap_or("");
        let arguments = config
            .get("toolArguments")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));

        if tool_name.is_empty() {
            return Err(CoreError::invalid("toolName is required"));
        }

        let result = executor.execute(tool_name, arguments).await?;
        if !result.success {
            let detail = result
                .error
                .or_else(|| {
                    result
                        .output
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| format!("workflow tool `{tool_name}` failed"));
            return Err(CoreError::other(detail));
        }
        Ok(serde_json::json!({
            "success": true,
            "output": result.output,
            "error": Value::Null
        }))
    }

    async fn execute_knowledge_retrieval(&self, config: &Map<String, Value>) -> Result<Value> {
        let retriever = match &self.knowledge_retriever {
            Some(r) => r,
            None => {
                return Ok(serde_json::json!({
                    "documents": [],
                    "content": "",
                    "__note": "no knowledge retriever configured"
                }));
            }
        };

        let query = config
            .get("queryVariable")
            .and_then(Value::as_str)
            .unwrap_or("");
        let top_k = config
            .get("knowledgeTopK")
            .and_then(Value::as_u64)
            .unwrap_or(3) as usize;
        let score_threshold = config
            .get("scoreThreshold")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;

        if query.is_empty() {
            return Ok(serde_json::json!({
                "documents": [],
                "content": ""
            }));
        }

        let top_k = top_k.clamp(1, 20);
        let docs = retriever.search(query, top_k).await?;

        let filtered: Vec<_> = docs
            .into_iter()
            .filter(|d| d.score >= score_threshold)
            .collect();

        let content = filtered
            .iter()
            .map(|d| {
                if d.title.is_empty() {
                    d.content.clone()
                } else {
                    format!("## {}\n{}", d.title, d.content)
                }
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");

        let documents: Vec<Value> = filtered
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "title": d.title,
                    "content": d.content,
                    "score": d.score,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "documents": documents,
            "content": content,
        }))
    }

    async fn execute_iteration(&self, config: &Map<String, Value>) -> Result<Value> {
        // Get the collection to iterate over
        let collection = config
            .get("iterationCollection")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if collection.is_empty() {
            return Ok(serde_json::json!({
                "output": [],
                "__note": "empty collection"
            }));
        }

        // Get the iterator variable name
        let iterator_var = config
            .get("iteratorVariable")
            .and_then(Value::as_str)
            .unwrap_or("item");

        // Get the operation to perform on each item
        // For now, support simple pass-through or template transform
        let operation = config
            .get("iterationOperation")
            .and_then(Value::as_str)
            .unwrap_or("passthrough");

        let mut results = Vec::new();
        for item in collection {
            let result = match operation {
                "template" => {
                    // Apply template transform if specified
                    let template = config
                        .get("iterationTemplate")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let transformed =
                        template.replace(&format!("{{{{{}}}}}", iterator_var), &item.to_string());
                    Value::String(transformed)
                }
                _ => {
                    // Pass through the item as-is
                    item.clone()
                }
            };
            results.push(result);
        }

        Ok(serde_json::json!({
            "output": results
        }))
    }

    fn finish_node(
        &mut self,
        node_id: String,
        node_kind: String,
        result: Result<Value>,
        elapsed_ms: u64,
    ) -> Result<AgentDecision> {
        self.finish_node_with_updates(node_id, node_kind, result, elapsed_ms, BTreeMap::new())
    }

    fn finish_node_with_updates(
        &mut self,
        node_id: String,
        node_kind: String,
        result: Result<Value>,
        elapsed_ms: u64,
        updates: BTreeMap<String, Value>,
    ) -> Result<AgentDecision> {
        match result {
            Ok(output) => {
                if self.is_cancelled() {
                    let error = CoreError::other("workflow cancelled before node completion");
                    self.node_statuses
                        .insert(node_id.clone(), NodeExecutionStatus::Cancelled);
                    self.emit_status(
                        &node_id,
                        NodeExecutionStatus::Cancelled,
                        1,
                        elapsed_ms,
                        None,
                        Some(error.to_string()),
                    );
                    self.active_node_id = None;
                    return Err(error);
                }
                let output_value = Self::node_output_value_inline(&node_kind, &output);
                if let Err(error) = self.admit_item_output(&node_id, &output_value) {
                    return self.finish_node_with_updates(
                        node_id,
                        node_kind,
                        Err(error),
                        elapsed_ms,
                        updates,
                    );
                }
                self.outputs.insert(node_id.clone(), output_value.clone());
                self.node_statuses
                    .insert(node_id.clone(), NodeExecutionStatus::Completed);
                self.publisher.emit(NodeExecutionEvent {
                    status: NodeExecutionStatus::Completed,
                    elapsed_ms,
                    outputs: Some(output_value),
                    updates,
                    ..self.base_event(&node_id)
                });
                self.active_node_id = None;
                self.step += 1;

                if node_kind == "end" {
                    let summary = output
                        .get("output")
                        .or_else(|| output.as_object().and_then(|m| m.values().next()))
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "workflow completed".to_string());
                    return Ok(AgentDecision::Complete(summary));
                }

                Ok(AgentDecision::Continue)
            }
            Err(error) => {
                // 用户取消不是失败：视频这类长作业在轮询中途被打断时走到这里，
                // 终态必须区分开，否则一次停止会被记成一次执行失败。
                let status = if self.is_cancelled() {
                    NodeExecutionStatus::Cancelled
                } else {
                    NodeExecutionStatus::Failed
                };
                self.node_statuses.insert(node_id.clone(), status);
                self.emit_status(
                    &node_id,
                    status,
                    1,
                    elapsed_ms,
                    None,
                    Some(error.to_string()),
                );
                self.active_node_id = None;
                Err(error)
            }
        }
    }

    fn correlated_tool_output(
        last: &[Observation],
        call_id: &str,
        tool: &str,
        label: &str,
    ) -> Result<Value> {
        let observation = last
            .iter()
            .find(|observation| {
                observation.call_id.as_deref() == Some(call_id) && observation.tool == tool
            })
            .ok_or_else(|| CoreError::other(format!("{label}: missing correlated tool result")))?;
        if !observation.ok {
            return Err(CoreError::other(format!("{label}: {}", observation.output)));
        }
        Ok(observation
            .raw_output
            .as_ref()
            .unwrap_or(&observation.output)
            .clone())
    }

    fn node_output_value_inline(kind: &str, result: &Value) -> Value {
        match kind {
            "if-else" => {
                let branch = result
                    .get("__branch")
                    .cloned()
                    .unwrap_or(Value::String("false".into()));
                serde_json::json!({ "branch": branch })
            }
            _ => result.clone(),
        }
    }
}

#[async_trait]
impl Agent for WorkflowAgent {
    fn set_event_sink(&mut self, events: Arc<dyn crate::events::RuntimeEventSink>) {
        self.publisher.set_sink(events);
    }

    async fn think_cancelled(
        &mut self,
        step: usize,
        last: &[Observation],
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<AgentDecision> {
        let Some(cancel) = cancel else {
            return self.think(step, last).await;
        };
        let result =
            crate::cancellation::await_cancelled(self.think(step, last), Some(cancel)).await;
        if result.is_err() && self.is_cancelled() && self.pending_iteration.is_none() {
            self.cancel_active_node();
        }
        result
    }

    async fn think(&mut self, _step: usize, last: &[Observation]) -> Result<AgentDecision> {
        if self.pending_iteration.is_some() {
            return self.advance_iteration(last).await;
        }
        if let Some(pending) = self.pending_media.take() {
            let tool = pending.output.tool_name();
            let output = Self::correlated_tool_output(last, &pending.call_id, tool, tool);
            let result = match pending.output {
                PendingMediaOutput::Image => output.and_then(|value| {
                    serde_json::from_value::<CanvasImageResponse>(value)
                        .map(Self::image_response_value)
                        .map_err(|error| {
                            CoreError::other(format!(
                                "{tool} returned an invalid image response: {error}"
                            ))
                        })
                }),
                PendingMediaOutput::AudioInput(mut config) => {
                    async {
                        let receipt: CanvasArtifactResponse = serde_json::from_value(output?)
                            .map_err(|error| {
                                CoreError::other(format!(
                                    "{tool} returned an invalid artifact response: {error}"
                                ))
                            })?;
                        if self.is_cancelled() {
                            return Err(CoreError::other("workflow cancelled"));
                        }
                        config.insert("audioReference".into(), Value::String(receipt.artifact_uri));
                        self.execute_audio(&config).await
                    }
                    .await
                }
                PendingMediaOutput::Video => output.and_then(|value| {
                    serde_json::from_value::<CanvasVideoResponse>(value)
                        .map(|response| {
                            serde_json::json!({
                                "videoUrl": response.artifact_uri,
                                "videoTaskId": response.task_id,
                                "videoTaskFingerprint": response.resume_fingerprint,
                                "mime": response.mime,
                                "operation": "video_generate",
                                "providerId": response.provider_id,
                                "modelId": response.model_id,
                            })
                        })
                        .map_err(|error| {
                            CoreError::other(format!(
                                "{tool} returned an invalid video response: {error}"
                            ))
                        })
                }),
            };
            return self.finish_node(
                pending.node_id,
                pending.node_kind,
                result,
                pending.started.elapsed().as_millis() as u64,
            );
        }
        if let Some(pending) = self.pending_tool.take() {
            let node_kind = pending.kind.node_kind();
            let result = Self::correlated_tool_output(
                last,
                &pending.call_id,
                pending.kind.tool_name(),
                node_kind,
            );
            let mut updates = BTreeMap::new();
            let result = match pending.kind {
                PendingNodeToolKind::Http => result,
                PendingNodeToolKind::Code {
                    invocation,
                    outputs,
                } => {
                    if let Some(bridge) = self.code_bridge.as_ref() {
                        bridge.release(&invocation.request_id);
                    }
                    result.and_then(|value| {
                        let response: CodeResponse =
                            serde_json::from_value(value).map_err(|_| {
                                CoreError::invalid(
                                    "CodeResultInvalid: malformed execution response",
                                )
                            })?;
                        response.validate(&invocation, &outputs)?;
                        updates.insert(
                            "codeExecution".into(),
                            serde_json::to_value(response.receipt)?,
                        );
                        Ok(Value::Object(response.outputs))
                    })
                }
            };
            return self.finish_node_with_updates(
                pending.node_id,
                node_kind.into(),
                result,
                pending.started.elapsed().as_millis() as u64,
                updates,
            );
        }
        if self.is_cancelled() {
            return Err(CoreError::other("workflow cancelled"));
        }

        if self.step >= self.walk.len() {
            let summary = self
                .outputs
                .get("end")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "workflow completed".to_string());
            return Ok(AgentDecision::Complete(summary));
        }

        let node_index = self.walk[self.step];
        let node = &self.compiled.definition.nodes[node_index];
        let node_id = node.id.clone();
        let node_kind = node.kind.clone();
        let node_config = node.config.clone();

        if !self.has_active_predecessor(&node_id) {
            self.node_statuses
                .insert(node_id.clone(), NodeExecutionStatus::Skipped);
            self.emit_status(
                &node_id,
                NodeExecutionStatus::Skipped,
                1,
                0,
                None,
                Some("conditional branch not selected".to_string()),
            );
            self.step += 1;
            return Ok(AgentDecision::Continue);
        }

        if let Some(target) = &self.target_node_id {
            if &node_id != target && node_kind != "start" {
                let ancestors = self
                    .compiled
                    .ancestors
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_default();
                if !ancestors.contains(target) {
                    self.node_statuses
                        .insert(node_id.clone(), NodeExecutionStatus::Skipped);
                    self.emit_status(&node_id, NodeExecutionStatus::Skipped, 1, 0, None, None);
                    self.step += 1;
                    return Ok(AgentDecision::Continue);
                }
            }
        }

        self.active_node_id = Some(node_id.clone());
        self.node_statuses
            .insert(node_id.clone(), NodeExecutionStatus::Running);
        self.emit_status(&node_id, NodeExecutionStatus::Running, 1, 0, None, None);
        let node_start = Instant::now();

        if node_kind == "variable-aggregator" {
            let mut updates = BTreeMap::new();
            let result = self
                .compiled
                .aggregator_plans
                .get(&node_id)
                .ok_or_else(|| {
                    CoreError::invalid("AggregatorConfigInvalid: compiled plan is missing")
                })
                .and_then(|plan| plan.evaluate(&self.outputs, &self.node_statuses))
                .map(|(output, metadata)| {
                    updates = metadata;
                    output
                });
            return self.finish_node_with_updates(
                node_id,
                node_kind,
                result,
                node_start.elapsed().as_millis() as u64,
                updates,
            );
        }

        if node_kind == "iteration" {
            let preparation = self.begin_iteration(&node_id);
            if let Err(error) = preparation {
                return self.finish_node(node_id, node_kind, Err(error), 0);
            }
            return self.advance_iteration(last).await;
        }

        if node_kind == "http-request" {
            let arguments = match self.http_tool_arguments(&node_config) {
                Ok(arguments) => arguments,
                Err(error) => return self.finish_node(node_id, node_kind, Err(error), 0),
            };
            let call_id = format!("workflow_http_{}", deepagent_core::id::EventId::new());
            self.pending_tool = Some(PendingNodeTool {
                node_id,
                call_id: call_id.clone(),
                started: node_start,
                kind: PendingNodeToolKind::Http,
            });
            return Ok(AgentDecision::CallTool(
                deepagent_tools::ToolInvocation::new("http_request", arguments).with_id(call_id),
            ));
        }

        if node_kind == "code" {
            let preparation = (|| -> Result<_> {
                let request = CodeRequest::from_config(&node_config, &self.outputs)?;
                let outputs = request.outputs.clone();
                let bridge = self.code_bridge.as_ref().ok_or_else(|| {
                    CoreError::invalid("CodeExecutorUnavailable: no host code bridge was attached")
                })?;
                Ok((bridge.prepare(request)?, outputs))
            })();
            let (invocation, outputs) = match preparation {
                Ok(prepared) => prepared,
                Err(error) => return self.finish_node(node_id, node_kind, Err(error), 0),
            };
            let arguments = serde_json::to_value(&invocation)?;
            let call_id = format!("workflow_code_{}", deepagent_core::id::EventId::new());
            self.pending_tool = Some(PendingNodeTool {
                node_id,
                call_id: call_id.clone(),
                started: node_start,
                kind: PendingNodeToolKind::Code {
                    invocation,
                    outputs,
                },
            });
            return Ok(AgentDecision::CallTool(
                deepagent_tools::ToolInvocation::new(CODE_EXECUTE_TOOL, arguments).with_id(call_id),
            ));
        }

        if node_kind == "audio" {
            let preparation = (|| -> Result<_> {
                let config = self.resolve_config(&node_config)?;
                let reference = Self::audio_reference(&config)?;
                let preparation = reference
                    .map(|reference| {
                        self.canvas_bridge
                            .as_ref()
                            .ok_or_else(|| {
                                CoreError::other("audio nodes need the canvas model bridge")
                            })?
                            .prepare_audio_input(&reference)
                    })
                    .transpose()?;
                Ok((config, preparation))
            })();
            let (mut config, preparation) = match preparation {
                Ok(prepared) => prepared,
                Err(error) => return self.finish_node(node_id, node_kind, Err(error), 0),
            };
            match preparation {
                Some(CanvasAudioInputPreparation::Deferred { request_id, origin }) => {
                    // The signed source reference remains solely in the gateway.
                    config.remove("audioReference");
                    config.remove("audioInputs");
                    let call_id = format!("workflow_audio_{}", deepagent_core::id::EventId::new());
                    self.pending_media = Some(PendingMediaNode {
                        node_id,
                        node_kind,
                        call_id: call_id.clone(),
                        started: node_start,
                        output: PendingMediaOutput::AudioInput(config),
                    });
                    return Ok(AgentDecision::CallTool(
                        deepagent_tools::ToolInvocation::new(
                            "canvas_audio_fetch",
                            serde_json::json!({"request_id":request_id,"origin":origin}),
                        )
                        .with_id(call_id),
                    ));
                }
                Some(CanvasAudioInputPreparation::Ready(reference)) => {
                    config.remove("audioInputs");
                    config.insert("audioReference".into(), Value::String(reference));
                }
                None => {}
            }
            let result = self.execute_audio(&config).await;
            return self.finish_node(
                node_id,
                node_kind,
                result,
                node_start.elapsed().as_millis() as u64,
            );
        }

        if node_kind == "video-gen" {
            match self.prepare_video_gen(&node_config).await {
                Ok(CanvasVideoPreparation::Ready(response)) => {
                    return self.finish_node(
                        node_id,
                        node_kind,
                        Ok(serde_json::json!({
                            "videoUrl": response.artifact_uri,
                            "videoTaskId": response.task_id,
                            "videoTaskFingerprint": response.resume_fingerprint,
                            "mime": response.mime,
                            "operation": "video_generate",
                            "providerId": response.provider_id,
                            "modelId": response.model_id,
                        })),
                        node_start.elapsed().as_millis() as u64,
                    );
                }
                Ok(CanvasVideoPreparation::Deferred { request_id, origin }) => {
                    let call_id = format!("workflow_video_{}", deepagent_core::id::EventId::new());
                    self.pending_media = Some(PendingMediaNode {
                        node_id,
                        node_kind,
                        output: PendingMediaOutput::Video,
                        call_id: call_id.clone(),
                        started: node_start,
                    });
                    return Ok(AgentDecision::CallTool(
                        deepagent_tools::ToolInvocation::new(
                            "canvas_video_fetch",
                            serde_json::json!({
                                "request_id": request_id,
                                "origin": origin,
                            }),
                        )
                        .with_id(call_id),
                    ));
                }
                Err(error) => {
                    return self.finish_node(node_id, node_kind, Err(error), 0);
                }
            }
        }

        if matches!(node_kind.as_str(), "image-gen" | "image-edit") {
            let preparation = self.prepare_image_gen(&node_kind, &node_config).await;
            match preparation {
                Ok(CanvasImagePreparation::Ready(response)) => {
                    return self.finish_node(
                        node_id,
                        node_kind,
                        Ok(Self::image_response_value(response)),
                        node_start.elapsed().as_millis() as u64,
                    );
                }
                Ok(CanvasImagePreparation::Deferred { request_id, origin }) => {
                    let call_id = format!("workflow_image_{}", deepagent_core::id::EventId::new());
                    self.pending_media = Some(PendingMediaNode {
                        node_id,
                        node_kind,
                        output: PendingMediaOutput::Image,
                        call_id: call_id.clone(),
                        started: node_start,
                    });
                    return Ok(AgentDecision::CallTool(
                        deepagent_tools::ToolInvocation::new(
                            "canvas_image_fetch",
                            serde_json::json!({
                                "request_id": request_id,
                                "origin": origin,
                            }),
                        )
                        .with_id(call_id),
                    ));
                }
                Err(error) => {
                    return self.finish_node(node_id, node_kind, Err(error), 0);
                }
            }
        }

        let result = self.execute_node_inline(&node_kind, &node_config).await;
        let elapsed_ms = node_start.elapsed().as_millis() as u64;

        let updates = if node_kind == "start" {
            self.inputs.updates.clone()
        } else {
            BTreeMap::new()
        };
        self.finish_node_with_updates(node_id, node_kind, result, elapsed_ms, updates)
    }

    async fn finish_background_tasks(&mut self) {
        self.stop_iteration().await;
        // The Kernel can stop before the next think (cancellation, persistence
        // error or deadline). Close the pending node without inventing success.
        if let Some(pending) = self.pending_tool.take() {
            if let PendingNodeToolKind::Code { invocation, .. } = &pending.kind {
                if let Some(bridge) = self.code_bridge.as_ref() {
                    bridge.release(&invocation.request_id);
                }
            }
            let _ = self.finish_node(
                pending.node_id, pending.kind.node_kind().into(),
                Err(CoreError::other(format!("{}: run stopped before node result delivery; reconcile the durable action before retrying", pending.kind.tool_name()))),
                pending.started.elapsed().as_millis() as u64);
        }
        if let Some(pending) = self.pending_media.take() {
            let tool = pending.output.tool_name();
            let _ = self.finish_node(
                pending.node_id,
                pending.node_kind,
                Err(CoreError::other(format!(
                    "{tool}: run stopped before node result delivery; reconcile the durable action before retrying",
                ))),
                pending.started.elapsed().as_millis() as u64,
            );
        }
        self.cancel_active_node();
    }

    fn cumulative_usage(&self) -> Option<RunUsage> {
        Some(self.usage.total())
    }

    fn take_pending_raw_usage(&mut self) -> Vec<Value> {
        self.usage.take_raw()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{RuntimeEvent, RuntimeEventSink};
    use crate::workflow::graph::{compile, WorkflowDefinition, WorkflowEdgeSpec, WorkflowNodeSpec};
    use crate::workflow::node_events::{
        NodeEventPublisher, NodeExecutionEvent, NodeExecutionStatus,
    };

    fn make_start(id: &str) -> WorkflowNodeSpec {
        WorkflowNodeSpec {
            id: id.to_string(),
            kind: "start".to_string(),
            config: Map::new(),
        }
    }

    fn make_end(id: &str) -> WorkflowNodeSpec {
        WorkflowNodeSpec {
            id: id.to_string(),
            kind: "end".to_string(),
            config: Map::new(),
        }
    }

    fn make_edge(id: &str, source: &str, target: &str) -> WorkflowEdgeSpec {
        WorkflowEdgeSpec {
            id: id.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            source_handle: None,
            target_handle: None,
        }
    }

    use crate::workflow::canvas::{
        CanvasAudioRequest, CanvasAudioResponse, CanvasCompletionResponse, CanvasEmbeddingResponse,
        CanvasImageResponse, CanvasJobPhase, CanvasJobProgress, CanvasPromptPolicy,
        CanvasVideoRequest, CanvasVideoResponse,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct RecordingBridge {
        completions: Arc<Mutex<Vec<CanvasCompletionRequest>>>,
        images: Arc<Mutex<Vec<CanvasImageRequest>>>,
        audios: Arc<Mutex<Vec<CanvasAudioRequest>>>,
        videos: Arc<Mutex<Vec<CanvasVideoRequest>>>,
        completion_text: Option<String>,
        fail: bool,
    }

    #[async_trait]
    impl CanvasModelBridge for RecordingBridge {
        async fn complete(
            &self,
            request: CanvasCompletionRequest,
        ) -> deepagent_core::error::Result<CanvasCompletionResponse> {
            self.completions.lock().unwrap().push(request);
            if self.fail {
                return Err(deepagent_core::error::CoreError::other(
                    "SecretMissing: provider has no api key",
                ));
            }
            Ok(CanvasCompletionResponse {
                text: self
                    .completion_text
                    .clone()
                    .unwrap_or_else(|| "from canvas provider".to_string()),
                reasoning: Some("thinking about the tagline".to_string()),
                provider_id: "cvp-1".to_string(),
                model_id: "gpt-5.6-sol".to_string(),
                usage: None,
                raw_usage: None,
            })
        }

        async fn generate_image(
            &self,
            request: CanvasImageRequest,
        ) -> deepagent_core::error::Result<CanvasImageResponse> {
            let edited = !request.reference_images.is_empty();
            self.images.lock().unwrap().push(request);
            Ok(CanvasImageResponse {
                artifact_uri: "artifact://art_generated".to_string(),
                mime: "image/png".to_string(),
                provider_id: "cvp-1".to_string(),
                model_id: "gpt-image-2".to_string(),
                operation: if edited { "edit" } else { "generate" }.to_string(),
            })
        }

        async fn run_audio(
            &self,
            request: CanvasAudioRequest,
        ) -> deepagent_core::error::Result<CanvasAudioResponse> {
            let transcribe = request.audio_reference.is_some();
            self.audios.lock().unwrap().push(request);
            Ok(CanvasAudioResponse {
                operation: if transcribe {
                    "speech_transcribe".to_string()
                } else {
                    "speech_synthesize".to_string()
                },
                audio_url: if transcribe {
                    None
                } else {
                    Some("artifact://art_voice".to_string())
                },
                text: if transcribe {
                    Some("转写出来的文本".to_string())
                } else {
                    None
                },
                provider_id: "cvp-1".to_string(),
                model_id: "CosyVoice2-0.5B".to_string(),
            })
        }

        async fn generate_video(
            &self,
            request: CanvasVideoRequest,
        ) -> deepagent_core::error::Result<CanvasVideoResponse> {
            // 真实网关会在轮询途中报回作业 id，这里同样先报再返回结果。
            if let Some(sink) = request.on_progress.as_ref() {
                sink.emit(CanvasJobProgress {
                    job_id: "job-77".to_string(),
                    phase: CanvasJobPhase::Queued,
                    resume_fingerprint: "audit-fingerprint".to_string(),
                });
            }
            // 模型名带标记即模拟“轮询途中用户点了停止”：翻起真实取消标志再报错。
            if request.model_ref.contains("cancel-mid-job") {
                if let Some(flag) = request.cancel.as_ref() {
                    flag.store(true, Ordering::SeqCst);
                }
                return Err(deepagent_core::error::CoreError::other(
                    "Cancelled: video job `job-77` interrupted; rerun the node with the same task id to keep polling",
                ));
            }
            self.videos.lock().unwrap().push(request);
            Ok(CanvasVideoResponse {
                artifact_uri: "artifact://art_video".to_string(),
                mime: "video/mp4".to_string(),
                provider_id: "cvp-1".to_string(),
                model_id: "Wan2.2-T2V-A14B".to_string(),
                task_id: "job-77".to_string(),
                resume_fingerprint: "audit-fingerprint".to_string(),
            })
        }

        fn route_operation(
            &self,
            request: CanvasRouteRequest,
        ) -> deepagent_core::error::Result<crate::workflow::CanvasRouteOutcome> {
            use crate::workflow::CanvasRouteOutcome;
            if request.node_kind == "video-gen" {
                return Ok(CanvasRouteOutcome {
                    operation: "video_generate".to_string(),
                    reason: "artifact_facts".to_string(),
                });
            }
            let has_image = request.input_kinds.iter().any(|kind| kind == "Image");
            let operation = match request.explicit_operation.as_deref() {
                Some("image_generate") if has_image => {
                    return Err(deepagent_core::error::CoreError::invalid(
                        "OperationInputConflict: image_generate conflicts with image input",
                    ));
                }
                Some("image_generate") => "image_generate",
                Some("image_edit") if !has_image => {
                    return Err(deepagent_core::error::CoreError::invalid(
                        "MissingReferenceInput: image_edit requires an image artifact",
                    ));
                }
                Some("image_edit") => "image_edit",
                // 音频节点必须显式给 operation；真实校验在网关契约里，这里透传。
                Some(op @ ("speech_synthesize" | "speech_transcribe")) => op,
                Some(other) => {
                    return Err(deepagent_core::error::CoreError::invalid(format!(
                        "OperationInputConflict: `{other}` is not allowed for `{}`",
                        request.node_kind
                    )));
                }
                None if has_image => "image_edit",
                None => "image_generate",
            };
            Ok(CanvasRouteOutcome {
                operation: operation.to_string(),
                reason: if request.explicit_operation.is_some() {
                    "explicit_operation".to_string()
                } else {
                    "artifact_facts".to_string()
                },
            })
        }

        fn inspect_input_kinds(
            &self,
            references: &[String],
        ) -> deepagent_core::error::Result<Vec<String>> {
            Ok(references
                .iter()
                .map(|reference| {
                    let lowered = reference.to_ascii_lowercase();
                    if lowered.starts_with("data:audio")
                        || lowered.ends_with(".mp3")
                        || lowered.ends_with(".wav")
                        // 相当于制品索引里 kind=audio 的记录。
                        || lowered.starts_with("artifact://audio")
                    {
                        "Audio".to_string()
                    } else if lowered.starts_with("data:video") || lowered.ends_with(".mp4") {
                        "Video".to_string()
                    } else if lowered.starts_with("artifact://")
                        || lowered.contains("image")
                        || lowered.ends_with(".png")
                        || lowered.ends_with(".jpg")
                    {
                        "Image".to_string()
                    } else {
                        "Text".to_string()
                    }
                })
                .collect())
        }

        async fn embed(
            &self,
            request: CanvasEmbeddingRequest,
        ) -> deepagent_core::error::Result<CanvasEmbeddingResponse> {
            Ok(CanvasEmbeddingResponse {
                dimensions: 3,
                count: request.texts.len(),
                provider_id: "cvp-2".to_string(),
                model_id: "Qwen/Qwen3-VL-Embedding-8B".to_string(),
                vectors: Vec::new(),
                usage: None,
                raw_usage: None,
            })
        }
    }

    fn config(pairs: &[(&str, Value)]) -> Map<String, Value> {
        let mut config = Map::new();
        for (key, value) in pairs {
            config.insert((*key).to_string(), value.clone());
        }
        config
    }

    #[derive(Clone, Default)]
    struct CollectSink {
        events: Arc<Mutex<Vec<NodeExecutionEvent>>>,
    }

    impl RuntimeEventSink for CollectSink {
        fn emit(&self, event: RuntimeEvent) {
            if let RuntimeEvent::WorkflowNode { event } = event {
                self.events.lock().unwrap().push(event);
            }
        }
    }

    fn agent_with_bridge(bridge: RecordingBridge) -> WorkflowAgent {
        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![make_start("start-1"), make_end("end-1")],
            edges: vec![make_edge("e1", "start-1", "end-1")],
        };
        let compiled = compile(definition).unwrap();
        WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
            .expect("valid workflow inputs")
            .with_canvas_bridge(Arc::new(bridge))
    }

    #[tokio::test]
    async fn llm_node_with_explicit_canvas_model_uses_the_bridge() {
        let bridge = RecordingBridge::default();
        let records = bridge.completions.clone();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "llm",
                &config(&[
                    ("llmModel", json!("cvp-1::gpt-5.6-sol")),
                    ("llmPrompt", json!("write a tagline")),
                    ("llmSystemPrompt", json!("be brief")),
                    ("llmTemperature", json!(0.2)),
                    ("llmMaxTokens", json!(17)),
                ]),
            )
            .await
            .expect("bridge call succeeds");
        assert_eq!(outcome["text"], "from canvas provider");
        assert_eq!(outcome["providerId"], "cvp-1");
        assert_eq!(outcome["modelId"], "gpt-5.6-sol");
        assert_eq!(outcome["reasoning"], "thinking about the tagline");
        let recorded = records.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].model_ref, "cvp-1::gpt-5.6-sol");
        assert_eq!(recorded[0].prompt, "write a tagline");
        assert_eq!(recorded[0].system_prompt.as_deref(), Some("be brief"));
        assert_eq!(
            recorded[0].prompt_policy,
            CanvasPromptPolicy::Professional,
            "professional nodes must not use the creative prompt catalog"
        );
        assert_eq!(recorded[0].temperature, Some(0.2));
        assert_eq!(recorded[0].max_tokens, Some(17));
    }

    #[tokio::test]
    async fn professional_classifier_and_extractor_use_structured_canvas_outputs() {
        let classifier_bridge = RecordingBridge {
            completion_text: Some("1".to_string()),
            ..Default::default()
        };
        let classifier_records = classifier_bridge.completions.clone();
        let mut classifier = agent_with_bridge(classifier_bridge);
        let classified = classifier
            .execute_node_inline(
                "question-classifier",
                &config(&[
                    ("classifierModel", json!("cvp-1::gpt-5.6-sol")),
                    ("classifierInput", json!("需要退款")),
                    (
                        "classifierClasses",
                        json!([
                            {"name": "售后", "description": "售后问题"},
                            {"name": "技术", "description": "技术问题"}
                        ]),
                    ),
                ]),
            )
            .await
            .expect("classifier completion");
        assert_eq!(classified["class_name"], "售后");
        assert_eq!(classified["class_label"], "售后问题");
        assert_eq!(classified["__branch"], "class-0");
        assert_eq!(classifier_records.lock().unwrap().len(), 1);

        let extractor_bridge = RecordingBridge {
            completion_text: Some(r#"{"name":"Eighteen","age":"18"}"#.to_string()),
            ..Default::default()
        };
        let extractor_records = extractor_bridge.completions.clone();
        let mut extractor = agent_with_bridge(extractor_bridge);
        let extracted = extractor
            .execute_node_inline(
                "parameter-extractor",
                &config(&[
                    ("extractorModel", json!("cvp-1::gpt-5.6-sol")),
                    ("extractorInput", json!("name=Eighteen, age=18")),
                    (
                        "extractorParams",
                        json!([
                            {"name": "name", "type": "string", "required": true},
                            {"name": "age", "type": "number", "required": true}
                        ]),
                    ),
                ]),
            )
            .await
            .expect("extractor completion");
        assert_eq!(extracted["name"], "Eighteen");
        assert_eq!(extracted["age"], 18.0);
        assert_eq!(extracted["__is_success"], 1);
        assert_eq!(extractor_records.lock().unwrap().len(), 1);
    }

    #[test]
    fn classifier_branch_output_activates_only_the_selected_source_handle() {
        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![
                make_start("start"),
                WorkflowNodeSpec {
                    id: "classifier".to_string(),
                    kind: "question-classifier".to_string(),
                    config: config(&[(
                        "classifierClasses",
                        json!([
                            {"name": "one", "description": "first"},
                            {"name": "two", "description": "second"}
                        ]),
                    )]),
                },
                WorkflowNodeSpec {
                    id: "left".to_string(),
                    kind: "answer".to_string(),
                    config: config(&[("answerTemplate", json!("left"))]),
                },
                WorkflowNodeSpec {
                    id: "right".to_string(),
                    kind: "answer".to_string(),
                    config: config(&[("answerTemplate", json!("right"))]),
                },
            ],
            edges: vec![
                make_edge("start-classifier", "start", "classifier"),
                WorkflowEdgeSpec {
                    id: "classifier-left".to_string(),
                    source: "classifier".to_string(),
                    target: "left".to_string(),
                    source_handle: Some("class-0".to_string()),
                    target_handle: None,
                },
                WorkflowEdgeSpec {
                    id: "classifier-right".to_string(),
                    source: "classifier".to_string(),
                    target: "right".to_string(),
                    source_handle: Some("class-1".to_string()),
                    target_handle: None,
                },
            ],
        };
        let compiled = compile(definition).expect("classifier graph compiles");
        let mut agent =
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
                .expect("valid workflow inputs");
        agent
            .node_statuses
            .insert("classifier".to_string(), NodeExecutionStatus::Completed);
        agent
            .outputs
            .insert("classifier".to_string(), json!({"__branch": "class-0"}));

        assert!(agent.has_active_predecessor("left"));
        assert!(!agent.has_active_predecessor("right"));
    }

    #[tokio::test]
    async fn bridge_failure_surfaces_instead_of_falling_back() {
        let bridge = RecordingBridge {
            fail: true,
            ..Default::default()
        };
        let mut agent = agent_with_bridge(bridge);
        let error = agent
            .execute_node_inline(
                "llm",
                &config(&[
                    ("llmModel", json!("cvp-1::gpt-5.6-sol")),
                    ("llmPrompt", json!("hi")),
                ]),
            )
            .await
            .expect_err("no silent fallback");
        assert!(error.to_string().contains("SecretMissing"));
    }

    #[tokio::test]
    async fn agent_v2_is_explicitly_reserved_instead_of_running_one_shot_completion() {
        let bridge = RecordingBridge::default();
        let records = bridge.completions.clone();
        let mut agent = agent_with_bridge(bridge);
        let error = agent
            .execute_node_inline(
                "agent-v2",
                &config(&[
                    ("agentV2Model", json!("cvp-1::gpt-5.6-sol")),
                    ("agentV2Task", json!("use a tool")),
                    ("agentV2Tools", json!([{"name": "read_file"}])),
                    ("agentV2Memory", json!(true)),
                ]),
            )
            .await
            .expect_err("Agent V2 must not masquerade as a one-shot completion");
        assert!(error.to_string().contains("agent-v2` is reserved"));
        assert!(records.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn creative_text_gen_node_reads_plain_prompt_field() {
        let bridge = RecordingBridge::default();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "text-gen",
                &config(&[
                    ("model", json!("cvp-1::gpt-5.6-sol")),
                    ("prompt", json!("hello")),
                ]),
            )
            .await
            .expect("creative text node");
        assert_eq!(outcome["text"], "from canvas provider");
    }

    #[tokio::test]
    async fn image_node_with_reference_reports_edit_operation() {
        let bridge = RecordingBridge::default();
        let records = bridge.images.clone();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "image-gen",
                &config(&[
                    ("imageModel", json!("cvp-1::gpt-image-2")),
                    ("prompt", json!("make it night")),
                    ("referenceImages", json!(["artifact://art_input"])),
                ]),
            )
            .await
            .expect("image edit");
        assert_eq!(outcome["operation"], "edit");
        assert_eq!(outcome["imageUrl"], "artifact://art_generated");
        let serialized = outcome.to_string();
        assert!(
            !serialized.contains("base64"),
            "node output carried inline bytes: {serialized}"
        );
        // Resolving the artifact to bytes is the gateway's job; the node only
        // has to forward the reference it was given.
        let sent = records.lock().unwrap().first().cloned().expect("one call");
        assert_eq!(
            sent.reference_images,
            vec!["artifact://art_input".to_string()]
        );
    }

    #[tokio::test]
    async fn image_node_without_prompt_fails_with_missing_reference_input() {
        let bridge = RecordingBridge::default();
        let mut agent = agent_with_bridge(bridge);
        let error = agent
            .execute_node_inline("image-gen", &config(&[("prompt", json!("  "))]))
            .await
            .expect_err("empty prompt must fail");
        assert!(error.to_string().contains("MissingReferenceInput"));
    }

    #[tokio::test]
    async fn image_node_operation_comes_from_the_contract_route() {
        let with_image = RecordingBridge::default();
        let seen = with_image.images.clone();
        let mut agent = agent_with_bridge(with_image);
        agent
            .execute_node_inline(
                "image-gen",
                &config(&[
                    ("prompt", json!("change the sky")),
                    ("referenceImages", json!(["data:image/png;base64,BB"])),
                ]),
            )
            .await
            .expect("routed edit");
        assert_eq!(
            seen.lock().unwrap()[0].operation,
            "image_edit",
            "image input must route to the edit operation"
        );

        let without_image = RecordingBridge::default();
        let seen = without_image.images.clone();
        let mut agent = agent_with_bridge(without_image);
        agent
            .execute_node_inline("image-gen", &config(&[("prompt", json!("a cube"))]))
            .await
            .expect("routed generation");
        assert_eq!(seen.lock().unwrap()[0].operation, "image_generate");

        let mut agent = agent_with_bridge(RecordingBridge::default());
        let conflict = agent
            .execute_node_inline(
                "image-gen",
                &config(&[
                    ("prompt", json!("a cube")),
                    ("operation", json!("image_generate")),
                    ("referenceImages", json!(["data:image/png;base64,BB"])),
                ]),
            )
            .await
            .expect_err("explicit generate with an image input must conflict");
        assert!(conflict.to_string().contains("OperationInputConflict"));
    }

    #[tokio::test]
    async fn image_node_prompt_carries_the_upstream_text() {
        let bridge = RecordingBridge::default();
        let records = bridge.images.clone();
        let mut agent = agent_with_bridge(bridge);
        agent
            .execute_node_inline(
                "image-gen",
                &config(&[
                    ("prompt", json!("画成海报")),
                    ("upstreamTexts", json!(["一只戴帽子的橘猫"])),
                ]),
            )
            .await
            .expect("image generation");
        let sent = records
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("one image call");
        assert!(
            sent.prompt.contains("一只戴帽子的橘猫") && sent.prompt.contains("画成海报"),
            "prompt sent to the image endpoint was {:?}",
            sent.prompt
        );
    }

    #[tokio::test]
    async fn embedding_node_returns_dimensions_without_vectors() {
        let bridge = RecordingBridge::default();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline("embeddings", &config(&[("texts", json!(["a", "b"]))]))
            .await
            .expect("embedding node");
        assert_eq!(outcome["dimensions"], 3);
        assert_eq!(outcome["count"], 2);
        assert!(outcome.get("vectors").is_none());
    }

    #[tokio::test]
    async fn video_node_runs_one_job_and_keeps_the_task_id() {
        let bridge = RecordingBridge::default();
        let records = bridge.videos.clone();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "video-gen",
                &config(&[
                    ("videoModel", json!("cvp-1::Wan2.2-T2V-A14B")),
                    ("prompt", json!("一只橘猫在雨里走路")),
                    ("videoResolution", json!("720p")),
                ]),
            )
            .await
            .expect("video job");
        assert_eq!(outcome["videoUrl"], "artifact://art_video");
        assert_eq!(outcome["videoTaskId"], "job-77");
        assert_eq!(outcome["operation"], "video_generate");
        assert!(
            !outcome.to_string().contains("base64"),
            "video output must not carry inline bytes"
        );
        let sent = records
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("one video call");
        assert_eq!(sent.model_ref, "cvp-1::Wan2.2-T2V-A14B");
        assert_eq!(sent.size.as_deref(), Some("720p"));
        assert_eq!(sent.resume_task_id, None);
        assert_eq!(sent.resume_fingerprint, None);
    }

    #[tokio::test]
    async fn video_node_resumes_the_task_id_left_on_the_node() {
        let bridge = RecordingBridge::default();
        let records = bridge.videos.clone();
        let mut agent = agent_with_bridge(bridge);
        agent
            .execute_node_inline(
                "video-gen",
                &config(&[
                    ("videoModel", json!("cvp-1::Wan2.2-T2V-A14B")),
                    ("prompt", json!("继续上次那条")),
                    ("videoTaskId", json!("job-42")),
                    ("videoTaskFingerprint", json!("saved-binding")),
                ]),
            )
            .await
            .expect("resumed video job");
        let sent = records
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("one video call");
        // 中断后重跑要接着轮询同一个作业，不能再花一次额度提交新任务。
        assert_eq!(sent.resume_task_id.as_deref(), Some("job-42"));
        assert_eq!(sent.resume_fingerprint.as_deref(), Some("saved-binding"));
    }

    #[tokio::test]
    async fn video_node_forwards_the_run_cancellation_flag() {
        let bridge = RecordingBridge::default();
        let records = bridge.videos.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut agent = agent_with_bridge(bridge).with_cancel(cancel.clone());
        agent
            .execute_node_inline(
                "video-gen",
                &config(&[
                    ("videoModel", json!("cvp-1::Wan2.2-T2V-A14B")),
                    ("prompt", json!("一段短片")),
                ]),
            )
            .await
            .expect("video job");
        let sent = records.lock().unwrap().first().cloned().expect("call");
        // 长作业必须能被打断：桥接拿到的是同一次运行的标志。
        let flag = sent
            .cancel
            .expect("video jobs must receive the cancel flag");
        assert!(Arc::ptr_eq(&flag, &cancel));
    }

    fn video_agent(
        bridge: RecordingBridge,
        sink: CollectSink,
        model_ref: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> WorkflowAgent {
        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![
                make_start("start-1"),
                WorkflowNodeSpec {
                    id: "video-1".to_string(),
                    kind: "video-gen".to_string(),
                    config: config(&[
                        ("videoModel", json!(model_ref)),
                        ("prompt", json!("一段短片")),
                    ]),
                },
                make_end("end-1"),
            ],
            edges: vec![
                make_edge("e1", "start-1", "video-1"),
                make_edge("e2", "video-1", "end-1"),
            ],
        };
        let compiled = compile(definition).unwrap();
        let agent = WorkflowAgent::new(
            compiled,
            Map::new(),
            None,
            NodeEventPublisher::new(Arc::new(sink)),
        )
        .expect("valid workflow inputs")
        .with_canvas_bridge(Arc::new(bridge));
        match cancel {
            Some(flag) => agent.with_cancel(flag),
            None => agent,
        }
    }

    #[tokio::test]
    async fn completed_video_event_reports_provenance_and_artifact_refs() {
        let sink = CollectSink::default();
        let recorded = sink.events.clone();
        let mut agent = video_agent(
            RecordingBridge::default(),
            sink,
            "cvp-1::Wan2.2-T2V-A14B",
            None,
        );
        agent.think(0, &[]).await.expect("start runs");
        agent.think(1, &[]).await.expect("video job runs");

        let events = recorded.lock().unwrap();
        let done = events
            .iter()
            .find(|event| {
                event.node_id == "video-1" && event.status == NodeExecutionStatus::Completed
            })
            .expect("completed event");
        assert_eq!(done.operation.as_deref(), Some("video_generate"));
        assert_eq!(done.provider_id.as_deref(), Some("cvp-1"));
        assert_eq!(done.model_id.as_deref(), Some("Wan2.2-T2V-A14B"));
        assert_eq!(done.job_id.as_deref(), Some("job-77"));
        assert_eq!(done.artifacts, vec!["artifact://art_video"]);
        let progress = events
            .iter()
            .find(|event| {
                event.node_id == "video-1" && event.outputs.is_none() && event.job_id.is_some()
            })
            .expect("mid-flight job progress");
        assert_eq!(progress.status, NodeExecutionStatus::Running);
        assert_eq!(
            progress.updates.get("jobPhase").and_then(Value::as_str),
            Some("queued")
        );
    }

    #[tokio::test]
    async fn cancelled_video_job_reports_exactly_one_terminal_state() {
        let sink = CollectSink::default();
        let recorded = sink.events.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut agent = video_agent(
            RecordingBridge::default(),
            sink,
            "cvp-1::cancel-mid-job",
            Some(cancel.clone()),
        );

        agent.think(0, &[]).await.expect("start runs");
        agent
            .think(1, &[])
            .await
            .expect_err("the stop must reach the driver");

        let events = recorded.lock().unwrap();
        let terminals: Vec<&NodeExecutionEvent> = events
            .iter()
            .filter(|event| event.node_id == "video-1" && event.status.is_terminal())
            .collect();
        assert_eq!(terminals.len(), 1, "video-1 terminals: {terminals:?}");
        assert_eq!(terminals[0].status, NodeExecutionStatus::Cancelled);
        let error = terminals[0].error.clone().unwrap_or_default();
        assert!(
            error.contains("job-77"),
            "the provider task id must survive into the event: {error}"
        );
        assert!(cancel.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn creative_media_nodes_fail_instead_of_faking_completion() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        for kind in ["video-stitch", "image-compare"] {
            let error = agent
                .execute_node_inline(kind, &config(&[("prompt", json!("make a clip"))]))
                .await
                .expect_err("no executor yet");
            assert!(
                error.to_string().contains("UnsupportedOperation"),
                "{kind} reported {error}"
            );
        }
    }

    #[tokio::test]
    async fn video_edit_does_not_publish_unverified_outputs() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        for fields in [
            config(&[]),
            config(&[("videoUrl", json!("artifact://source"))]),
            config(&[
                ("videoEditOutputRef", json!("artifact://copied-source")),
                ("videoEditStatus", json!("exported")),
            ]),
        ] {
            let error = agent
                .execute_node_inline("video-edit", &fields)
                .await
                .expect_err("an unvalidated source copy is not a rendered edit");
            assert!(
                error.to_string().contains("UnsupportedOperation")
                    || error.to_string().contains("MissingReferenceInput")
                    || error.to_string().contains("InvalidConfiguration")
            );
        }
    }

    #[tokio::test]
    async fn video_edit_publishes_only_an_export_matching_its_input_and_trim() {
        struct ExistingVideoArtifacts;
        impl CanvasArtifactValidator for ExistingVideoArtifacts {
            fn validate_video_artifact(&self, reference: &str) -> Result<()> {
                if reference.starts_with("artifact://") {
                    Ok(())
                } else {
                    Err(CoreError::invalid("not an artifact reference"))
                }
            }
        }

        let mut agent = agent_with_bridge(RecordingBridge::default())
            .with_artifact_validator(Arc::new(ExistingVideoArtifacts));
        let valid = config(&[
            ("videoEditOutputRef", json!("artifact://rendered")),
            ("videoEditStatus", json!("exported")),
            ("videoEditInputRefs", json!(["artifact://source"])),
            ("videoEditTrimStart", json!(1.25)),
            ("videoEditTrimEnd", json!(4.5)),
            (
                "videoEditInputFingerprint",
                json!("artifact://source:1.250:4.500"),
            ),
        ]);
        let output = agent
            .execute_node_inline("video-edit", &valid)
            .await
            .expect("validated export passes through to downstream nodes");
        assert_eq!(output["videoUrl"], "artifact://rendered");

        let stale = config(&[
            ("videoEditOutputRef", json!("artifact://rendered")),
            ("videoEditStatus", json!("exported")),
            ("videoEditInputRefs", json!(["artifact://other-source"])),
            ("videoEditTrimStart", json!(1.25)),
            ("videoEditTrimEnd", json!(4.5)),
            (
                "videoEditInputFingerprint",
                json!("artifact://source:1.250:4.500"),
            ),
        ]);
        let error = agent
            .execute_node_inline("video-edit", &stale)
            .await
            .expect_err("source changes invalidate the export");
        assert!(error.to_string().contains("StaleArtifact"));
    }

    #[tokio::test]
    async fn audio_node_synthesizes_into_an_artifact_reference() {
        let bridge = RecordingBridge::default();
        let records = bridge.audios.clone();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "audio",
                &config(&[
                    ("operation", json!("speech_synthesize")),
                    ("audioModel", json!("cvp-1::CosyVoice2-0.5B")),
                    ("prompt", json!("把这句话念出来")),
                    ("audioVoice", json!("voice-42")),
                    ("audioFormat", json!("mp3")),
                ]),
            )
            .await
            .expect("synthesis");
        assert_eq!(outcome["audioUrl"], "artifact://art_voice");
        assert_eq!(outcome["operation"], "speech_synthesize");
        let sent = records
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("one audio call");
        assert_eq!(sent.text, "把这句话念出来");
        assert_eq!(sent.model_ref, "cvp-1::CosyVoice2-0.5B");
        // 音色与容器由节点配置原样透传，内核不补默认值。
        assert_eq!(sent.voice.as_deref(), Some("voice-42"));
        assert_eq!(sent.format.as_deref(), Some("mp3"));
        assert_eq!(sent.audio_reference, None);
        assert!(
            !outcome.to_string().contains("base64"),
            "audio output must not carry inline bytes"
        );
    }

    #[tokio::test]
    async fn audio_node_transcribes_the_referenced_audio() {
        let bridge = RecordingBridge::default();
        let records = bridge.audios.clone();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "audio",
                &config(&[
                    ("operation", json!("speech_transcribe")),
                    ("audioModel", json!("cvp-1::SenseVoiceSmall")),
                    ("audioReference", json!("data:audio/wav;base64,AA")),
                ]),
            )
            .await
            .expect("transcription");
        assert_eq!(outcome["text"], "转写出来的文本");
        assert_eq!(outcome["operation"], "speech_transcribe");
        assert!(outcome["audioUrl"].is_null());
        let sent = records
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("one audio call");
        assert_eq!(
            sent.audio_reference.as_deref(),
            Some("data:audio/wav;base64,AA")
        );
    }

    #[tokio::test]
    async fn audio_node_rejects_a_non_audio_reference() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        let error = agent
            .execute_node_inline(
                "audio",
                &config(&[
                    ("operation", json!("speech_transcribe")),
                    ("audioReference", json!("artifact://img-1")),
                ]),
            )
            .await
            .expect_err("an image cannot be transcribed");
        assert!(
            error.to_string().contains("OperationInputConflict"),
            "got {error}"
        );
    }

    #[tokio::test]
    async fn audio_node_takes_one_reference_from_edges() {
        let bridge = RecordingBridge::default();
        let records = bridge.audios.clone();
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "audio",
                &config(&[
                    ("operation", json!("speech_transcribe")),
                    ("audioInputs", json!(["artifact://audio-1"])),
                ]),
            )
            .await
            .expect("one audio input from an edge");
        assert_eq!(outcome["text"], "转写出来的文本");
        let sent = records
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("one audio call");
        assert_eq!(sent.audio_reference.as_deref(), Some("artifact://audio-1"));
    }

    #[tokio::test]
    async fn audio_node_rejects_two_audio_inputs() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        let conflict = agent
            .execute_node_inline(
                "audio",
                &config(&[
                    ("operation", json!("speech_transcribe")),
                    ("audioReference", json!("data:audio/wav;base64,AA")),
                    ("audioInputs", json!(["data:audio/mp3;base64,BB"])),
                ]),
            )
            .await
            .expect_err("the contract allows exactly one audio input");
        assert!(
            conflict.to_string().contains("exactly one audio input"),
            "got {conflict}"
        );
    }

    /// The graph registry accepts these kinds, so without a real executor the
    /// run must say so rather than echo the node config back as a result.
    #[tokio::test]
    async fn registered_kinds_without_an_executor_report_unsupported_node() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        for kind in [
            "document-extractor",
            "human-input",
            "datasource",
            "knowledge-index",
            "trigger-schedule",
        ] {
            let error = agent
                .execute_node_inline(kind, &config(&[("prompt", json!("run me"))]))
                .await
                .expect_err("no executor for this kind");
            assert!(
                error.to_string().contains("UnsupportedNode"),
                "{kind} reported {error}"
            );
        }
    }

    #[tokio::test]
    async fn carrier_nodes_publish_their_own_config() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        let imported = agent
            .execute_node_inline(
                "image-input",
                &config(&[("imageUrl", json!("artifact://img-1"))]),
            )
            .await
            .expect("input node carries its image");
        assert_eq!(imported["imageUrl"], json!("artifact://img-1"));

        let settings = agent
            .execute_node_inline("camera", &config(&[("cameraModel", json!("a7iv"))]))
            .await
            .expect("constraint node carries its settings");
        assert_eq!(settings["cameraModel"], json!("a7iv"));
    }

    #[tokio::test]
    async fn creative_responses_nodes_run_on_the_shared_llm_executor() {
        for kind in [
            "director",
            "storyboard-grid",
            "character-face",
            "creative-template",
        ] {
            let bridge = RecordingBridge::default();
            let records = bridge.completions.clone();
            let mut agent = agent_with_bridge(bridge);
            let outcome = agent
                .execute_node_inline(
                    kind,
                    &config(&[
                        ("model", json!("cvp-1::gpt-5.6-sol")),
                        ("prompt", json!("story about a cube")),
                    ]),
                )
                .await
                .unwrap_or_else(|error| panic!("{kind} failed: {error}"));
            assert_eq!(outcome["text"], "from canvas provider");
            assert_eq!(records.lock().unwrap().len(), 1, "{kind} must call once");
        }
    }

    #[tokio::test]
    async fn workflow_agent_executes_start_to_end() {
        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![make_start("start-1"), make_end("end-1")],
            edges: vec![make_edge("e1", "start-1", "end-1")],
        };
        let compiled = compile(definition).unwrap();
        let mut agent =
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
                .expect("valid workflow inputs");

        let decision = agent.think(0, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::Continue));

        let decision = agent.think(1, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::Complete(_)));
    }

    #[tokio::test]
    async fn workflow_agent_respects_cancellation() {
        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![make_start("start-1"), make_end("end-1")],
            edges: vec![make_edge("e1", "start-1", "end-1")],
        };
        let compiled = compile(definition).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let mut agent =
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
                .expect("valid workflow inputs")
                .with_cancel(cancel);

        let result = agent.think(0, &[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn workflow_agent_fails_tool_node_without_executor() {
        let mut tool_config = Map::new();
        tool_config.insert(
            "toolName".to_string(),
            Value::String("test_tool".to_string()),
        );
        tool_config.insert("toolArguments".to_string(), Value::Object(Map::new()));

        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![
                make_start("start-1"),
                WorkflowNodeSpec {
                    id: "tool-1".to_string(),
                    kind: "tool".to_string(),
                    config: tool_config,
                },
                make_end("end-1"),
            ],
            edges: vec![
                make_edge("e1", "start-1", "tool-1"),
                make_edge("e2", "tool-1", "end-1"),
            ],
        };
        let compiled = compile(definition).unwrap();
        let mut agent =
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
                .expect("valid workflow inputs");

        // Execute start
        agent.think(0, &[]).await.unwrap();

        // A missing executor is an execution failure, not a successful node
        // carrying an error-shaped output.
        let error = agent.think(1, &[]).await.unwrap_err();
        assert!(error.to_string().contains("tool executor"));
        assert!(!agent.outputs.contains_key("tool-1"));
    }

    #[tokio::test]
    async fn workflow_agent_handles_knowledge_node_without_retriever() {
        let mut knowledge_config = Map::new();
        knowledge_config.insert(
            "queryVariable".to_string(),
            Value::String("test query".to_string()),
        );
        knowledge_config.insert("knowledgeTopK".to_string(), Value::Number(3.into()));

        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![
                make_start("start-1"),
                WorkflowNodeSpec {
                    id: "knowledge-1".to_string(),
                    kind: "knowledge-retrieval".to_string(),
                    config: knowledge_config,
                },
                make_end("end-1"),
            ],
            edges: vec![
                make_edge("e1", "start-1", "knowledge-1"),
                make_edge("e2", "knowledge-1", "end-1"),
            ],
        };
        let compiled = compile(definition).unwrap();
        let mut agent =
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
                .expect("valid workflow inputs");

        // Execute start
        agent.think(0, &[]).await.unwrap();

        // Execute knowledge retrieval (should handle missing retriever gracefully)
        let decision = agent.think(1, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::Continue));

        // Verify knowledge output is empty but valid
        let output = agent.outputs.get("knowledge-1").unwrap();
        assert!(output
            .get("documents")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());
    }
}
