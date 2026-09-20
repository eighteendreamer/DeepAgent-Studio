//! Workflow execution agent that drives a compiled graph through the kernel.
//!
//! [`WorkflowAgent`] implements [`Agent`] so the existing `AgentKernel` loop can
//! execute professional canvas workflows without a second run center. Each
//! `think()` call advances one node in topological order, resolving variable
//! references, executing the node operation, and publishing status events.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use deepagent_core::message::Message;
use deepagent_models::chat_completions::ChatCompletionRequest;

use super::canvas::{
    CanvasAudioRequest, CanvasCompletionRequest, CanvasEmbeddingRequest, CanvasImageRequest,
    CanvasModelBridge, CanvasRouteRequest, CanvasVideoRequest,
};
use deepagent_models::client::ModelClient;
use serde_json::{Map, Value};

use super::graph::CompiledWorkflow;
use super::knowledge::KnowledgeRetriever;
use super::node_events::{NodeEventPublisher, NodeExecutionEvent, NodeExecutionStatus};
use super::tools::ToolExecutor;
use super::values;
use crate::agent::{Agent, AgentDecision, Observation, RunUsage};
use deepagent_core::error::{CoreError, Result};

/// Executes a compiled workflow graph as an [`Agent`].
///
/// The agent walks nodes in topological order. Each `think()` call executes one
/// node, stores its outputs, and emits lifecycle events. When all nodes complete
/// (or an `end` node is reached), the agent returns `Complete` with the final
/// output summary.
pub struct WorkflowAgent {
    compiled: CompiledWorkflow,
    inputs: Map<String, Value>,
    target_node_id: Option<String>,
    publisher: NodeEventPublisher,
    outputs: BTreeMap<String, Value>,
    step: usize,
    #[allow(dead_code)]
    started_at: Instant,
    cancel: Option<Arc<AtomicBool>>,
    model: Option<Arc<ModelClient>>,
    model_name: Option<String>,
    knowledge_retriever: Option<Arc<dyn KnowledgeRetriever>>,
    tool_executor: Option<Arc<dyn ToolExecutor>>,
    canvas_bridge: Option<Arc<dyn CanvasModelBridge>>,
    usage: RunUsage,
}

impl WorkflowAgent {
    /// Create a new workflow agent from a compiled graph and user inputs.
    pub fn new(
        compiled: CompiledWorkflow,
        inputs: Map<String, Value>,
        target_node_id: Option<String>,
        publisher: NodeEventPublisher,
    ) -> Self {
        Self {
            compiled,
            inputs,
            target_node_id,
            publisher,
            outputs: BTreeMap::new(),
            step: 0,
            started_at: Instant::now(),
            cancel: None,
            model: None,
            model_name: None,
            knowledge_retriever: None,
            tool_executor: None,
            canvas_bridge: None,
            usage: RunUsage::default(),
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

    /// Attach a tool executor so tool nodes can execute registered tools.
    pub fn with_tool_executor(mut self, executor: Arc<dyn ToolExecutor>) -> Self {
        self.tool_executor = Some(executor);
        self
    }

    fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|flag| flag.load(Ordering::Relaxed))
            .unwrap_or(false)
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
            revision: self.compiled.revision.clone(),
            node_id: node_id.to_string(),
            status,
            scope: Vec::new(),
            attempt,
            elapsed_ms,
            outputs,
            updates: BTreeMap::new(),
            error,
        });
    }

    async fn execute_node_inline(
        &mut self,
        kind: &str,
        config: &Map<String, Value>,
    ) -> Result<Value> {
        let mut resolved_config = Map::new();
        for (key, value) in config {
            resolved_config.insert(key.clone(), values::resolve(value, &self.outputs)?);
        }

        match kind {
            "start" => self.execute_start(&resolved_config),
            "end" => self.execute_end(&resolved_config),
            "answer" => self.execute_answer(&resolved_config),
            "variable-assigner" => self.execute_variable_assigner(&resolved_config),
            "variable-aggregator" => self.execute_variable_aggregator(&resolved_config),
            "if-else" => self.execute_if_else(&resolved_config),
            "code" => self.execute_code(&resolved_config),
            "template-transform" => self.execute_template_transform(&resolved_config),
            "list-operator" => self.execute_list_operator(&resolved_config),
            "llm" | "agent" | "agent-v2" | "text-gen" | "script-gen" | "director"
            | "creative-template" | "storyboard-grid" | "category-picker" | "character-face"
            | "character-body" | "character-style" => {
                self.execute_llm(kind, &resolved_config).await
            }
            "image-gen" | "image-edit" => self.execute_image_gen(kind, &resolved_config).await,
            "audio" => self.execute_audio(&resolved_config).await,
            "video-gen" => self.execute_video(&resolved_config).await,
            "embeddings" => self.execute_embedding(&resolved_config).await,
            // Media kinds without a backend job runner fail loudly instead of
            // reporting a fabricated completion through the passthrough arm.
            "video-stitch" | "image-compare" => Err(deepagent_core::error::CoreError::invalid(
                format!("UnsupportedOperation: canvas node `{kind}` has no executor yet"),
            )),
            "http-request" => self.execute_http_request(&resolved_config).await,
            "tool" => self.execute_tool(&resolved_config).await,
            "knowledge-retrieval" => self.execute_knowledge_retrieval(&resolved_config).await,
            "iteration" | "loop" => self.execute_iteration(&resolved_config).await,
            // These nodes have nothing to compute: an input node's own config
            // *is* its output, the loop markers only carry their context, and
            // the ContractOnly constraint nodes exist to hold their settings.
            "image-input" | "iteration-start" | "loop-start" | "loop-end" | "camera" | "lens"
            | "focal-length" | "aperture" => Ok(Value::Object(resolved_config)),
            // Anything else must fail loudly. Forwarding its config would make
            // an unimplemented node look like it had produced real results.
            _ => Err(deepagent_core::error::CoreError::invalid(format!(
                "UnsupportedNode: kernel has no executor for node kind `{kind}`"
            ))),
        }
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

    fn execute_start(&mut self, config: &Map<String, Value>) -> Result<Value> {
        let mut result = Map::new();
        if let Some(variables) = config.get("inputVariables").and_then(Value::as_array) {
            for var in variables {
                let name = var
                    .get("variable")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !name.is_empty() {
                    let value = self.inputs.get(name).cloned().unwrap_or(Value::Null);
                    result.insert(name.to_string(), value);
                }
            }
        }
        for (key, value) in &self.inputs {
            result.entry(key.clone()).or_insert_with(|| value.clone());
        }
        Ok(Value::Object(result))
    }

    fn execute_end(&mut self, config: &Map<String, Value>) -> Result<Value> {
        let mut result = Map::new();
        if let Some(variables) = config.get("outputVariables").and_then(Value::as_array) {
            for var in variables {
                let name = var
                    .get("variable")
                    .and_then(Value::as_str)
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
        let text = config
            .get("text")
            .cloned()
            .unwrap_or(Value::String(String::new()));
        let resolved = values::resolve(&text, &self.outputs)?;
        let answer = match resolved {
            Value::String(s) => s,
            other => other.to_string(),
        };
        Ok(serde_json::json!({ "answer": answer }))
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

    fn execute_variable_aggregator(&self, config: &Map<String, Value>) -> Result<Value> {
        let variables = config
            .get("variables")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut aggregated = Vec::new();
        for var in variables {
            let resolved = values::resolve(&var, &self.outputs)?;
            aggregated.push(resolved);
        }
        Ok(serde_json::json!({ "output": aggregated }))
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
                .cloned()
                .unwrap_or_default();
            let group_op = group
                .get("logicalOperator")
                .and_then(Value::as_str)
                .unwrap_or("and");

            let mut group_result = true;
            for item in items {
                let left = item.get("left").cloned().unwrap_or(Value::Null);
                let operator = item.get("operator").and_then(Value::as_str).unwrap_or("is");
                let right = item.get("right").cloned().unwrap_or(Value::Null);

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

    fn execute_code(&self, config: &Map<String, Value>) -> Result<Value> {
        let _code = config.get("code").and_then(Value::as_str).unwrap_or("");
        let _language = config
            .get("codeLanguage")
            .and_then(Value::as_str)
            .unwrap_or("javascript");
        let variables = config
            .get("codeVariables")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let outputs = config
            .get("codeOutputVariables")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut result = Map::new();
        for var in variables {
            let name = var
                .get("variable")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let value = var.get("value").cloned().unwrap_or(Value::Null);
            if !name.is_empty() {
                let resolved = values::resolve(&value, &self.outputs)?;
                result.insert(name.to_string(), resolved);
            }
        }
        for out in outputs {
            let name = out
                .get("variable")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !name.is_empty() && !result.contains_key(name) {
                let default = match out.get("type").and_then(Value::as_str) {
                    Some("number") => Value::Number(serde_json::Number::from(0)),
                    Some("array") => Value::Array(Vec::new()),
                    Some("object") => Value::Object(Map::new()),
                    _ => Value::String(String::new()),
                };
                result.insert(name.to_string(), default);
            }
        }
        Ok(Value::Object(result))
    }

    fn execute_template_transform(&self, config: &Map<String, Value>) -> Result<Value> {
        let template = config.get("template").and_then(Value::as_str).unwrap_or("");
        let variables = config
            .get("variables")
            .and_then(Value::as_object)
            .cloned()
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

    async fn execute_llm(&mut self, kind: &str, config: &Map<String, Value>) -> Result<Value> {
        let model_ref = config
            .get("llmModel")
            .or_else(|| config.get("model"))
            .or_else(|| config.get("classifierModel"))
            .or_else(|| config.get("extractorModel"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let user_prompt = config
            .get("llmPrompt")
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
            if !model_ref.is_empty() || self.model.is_none() {
                let response = bridge
                    .complete(CanvasCompletionRequest {
                        model_ref: model_ref.clone(),
                        node_kind: kind.to_string(),
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
                    })
                    .await?;
                let mut outcome = serde_json::json!({
                    "text": response.text,
                    "providerId": response.provider_id,
                    "modelId": response.model_id,
                    "model": if model_ref.is_empty() { response.model_id.clone() } else { model_ref },
                });
                if let Some(reasoning) = response.reasoning {
                    outcome["reasoning"] = serde_json::Value::String(reasoning);
                }
                return Ok(outcome);
            }
        }

        let client = match &self.model {
            Some(c) => c.clone(),
            None => {
                return Ok(serde_json::json!({
                    "text": format!("[{}] no model client configured for workflow run", kind),
                    "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 }
                }));
            }
        };

        let model_name = self.model_name.as_deref().unwrap_or("deepseek-chat");

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

        let mut request = ChatCompletionRequest::new(model_name, messages);
        if let Some(t) = temperature {
            request.temperature = Some(t);
        }
        if let Some(m) = max_tokens {
            request.max_tokens = Some(m);
        }

        let response = client.stream_chat_completion(request).await?;
        let text = response.output_text_projection();

        let usage_json = response
            .usage
            .as_ref()
            .map(|u| {
                serde_json::json!({
                    "prompt_tokens": u.prompt_tokens,
                    "completion_tokens": u.completion_tokens,
                    "total_tokens": u.total_tokens,
                })
            })
            .unwrap_or_else(|| {
                serde_json::json!({ "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 })
            });

        if let Some(u) = &response.usage {
            self.usage.prompt_tokens += u.prompt_tokens;
            self.usage.completion_tokens += u.completion_tokens;
            self.usage.total_tokens += u.total_tokens;
        }

        Ok(serde_json::json!({
            "text": text,
            "usage": usage_json
        }))
    }

    /// Execute a canvas image node. Whether the call generates a new image or
    /// edits a reference image is decided by the resolved inputs, exactly like
    /// the deterministic router prescribes.
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
        let response = bridge
            .generate_image(CanvasImageRequest {
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
            .await?;
        Ok(serde_json::json!({
            // The node keeps its existing `imageUrl` field, but the value is an
            // `artifact://` reference: no base64 enters events or the graph.
            "imageUrl": response.artifact_uri,
            "mime": response.mime,
            "operation": response.operation,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        }))
    }

    /// Video node: one asynchronous provider job, resumable by task id.
    async fn execute_video(&self, config: &Map<String, Value>) -> Result<Value> {
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
        let response = bridge
            .generate_video(CanvasVideoRequest {
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
                // 上次中断留下的任务 id：继续轮询，不再提交一次生成。
                resume_task_id: config
                    .get("videoTaskId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .filter(|value| !value.trim().is_empty()),
                timeout_ms: 600_000,
                cancel: self.cancel.clone(),
            })
            .await?;
        Ok(serde_json::json!({
            "videoUrl": response.artifact_uri,
            "videoTaskId": response.task_id,
            "mime": response.mime,
            "operation": routed.operation,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        }))
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
        let audio_reference = match references.len() {
            0 => None,
            1 => Some(references.remove(0)),
            count => {
                return Err(deepagent_core::error::CoreError::invalid(format!(
                "OperationInputConflict: `audio` node takes exactly one audio input, got {count}"
            )))
            }
        };
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
            .embed(CanvasEmbeddingRequest {
                model_ref: config
                    .get("embeddingModel")
                    .or_else(|| config.get("model"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                texts,
            })
            .await?;
        Ok(serde_json::json!({
            "count": response.count,
            "dimensions": response.dimensions,
            "providerId": response.provider_id,
            "modelId": response.model_id,
        }))
    }

    async fn execute_http_request(&self, config: &Map<String, Value>) -> Result<Value> {
        let method = config
            .get("httpMethod")
            .and_then(Value::as_str)
            .unwrap_or("GET");
        let url = config.get("httpUrl").and_then(Value::as_str).unwrap_or("");
        let body = config.get("httpBody").and_then(Value::as_str).unwrap_or("");
        let timeout_secs = config
            .get("httpTimeout")
            .and_then(Value::as_u64)
            .unwrap_or(30);
        let retry_count = config
            .get("httpRetryCount")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;

        let headers: Vec<(String, String)> = config
            .get("httpHeaders")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        if url.is_empty() {
            return Err(CoreError::invalid("http-request: url is required"));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| CoreError::other(format!("http-request: client build failed: {e}")))?;

        let req_method = match method {
            "POST" => reqwest::Method::POST,
            "PUT" => reqwest::Method::PUT,
            "DELETE" => reqwest::Method::DELETE,
            "PATCH" => reqwest::Method::PATCH,
            _ => reqwest::Method::GET,
        };

        let mut last_error = String::new();
        let attempts = retry_count + 1;

        for attempt in 0..attempts {
            let mut req = client.request(req_method.clone(), url);
            for (k, v) in &headers {
                req = req.header(k.as_str(), v.as_str());
            }
            if !body.is_empty()
                && matches!(
                    req_method,
                    reqwest::Method::POST | reqwest::Method::PUT | reqwest::Method::PATCH
                )
            {
                req = req.body(body.to_string());
            }

            match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let resp_headers: serde_json::Map<String, Value> = resp
                        .headers()
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.as_str().to_string(),
                                Value::String(v.to_str().unwrap_or("").to_string()),
                            )
                        })
                        .collect();
                    let resp_body = resp.text().await.unwrap_or_default();

                    let body_json: Value =
                        serde_json::from_str(&resp_body).unwrap_or(Value::String(resp_body));

                    return Ok(serde_json::json!({
                        "body": body_json,
                        "status_code": status,
                        "headers": resp_headers,
                    }));
                }
                Err(e) => {
                    last_error = e.to_string();
                    if attempt < attempts - 1 {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }

        Err(CoreError::other(format!(
            "http-request: failed after {} attempts: {}",
            attempts, last_error
        )))
    }

    async fn execute_tool(&self, config: &Map<String, Value>) -> Result<Value> {
        let executor = match &self.tool_executor {
            Some(e) => e,
            None => {
                return Ok(serde_json::json!({
                    "success": false,
                    "output": null,
                    "error": "no tool executor configured"
                }));
            }
        };

        let tool_name = config.get("toolName").and_then(Value::as_str).unwrap_or("");
        let arguments = config
            .get("toolArguments")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));

        if tool_name.is_empty() {
            return Ok(serde_json::json!({
                "success": false,
                "output": null,
                "error": "toolName is required"
            }));
        }

        let result = executor.execute(tool_name, arguments).await?;
        Ok(serde_json::json!({
            "success": result.success,
            "output": result.output,
            "error": result.error
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
    async fn think(&mut self, _step: usize, _last: &[Observation]) -> Result<AgentDecision> {
        if self.is_cancelled() {
            return Err(CoreError::other("workflow cancelled"));
        }

        if self.step >= self.compiled.order.len() {
            let summary = self
                .outputs
                .get("end")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "workflow completed".to_string());
            return Ok(AgentDecision::Complete(summary));
        }

        let node_index = self.compiled.order[self.step];
        let node = &self.compiled.definition.nodes[node_index];
        let node_id = node.id.clone();
        let node_kind = node.kind.clone();
        let node_config = node.config.clone();

        if let Some(target) = &self.target_node_id {
            if &node_id != target && node_kind != "start" {
                let ancestors = self
                    .compiled
                    .ancestors
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_default();
                if !ancestors.contains(target) {
                    self.emit_status(&node_id, NodeExecutionStatus::Skipped, 1, 0, None, None);
                    self.step += 1;
                    return Ok(AgentDecision::Continue);
                }
            }
        }

        self.emit_status(&node_id, NodeExecutionStatus::Running, 1, 0, None, None);
        let node_start = Instant::now();

        let result = self.execute_node_inline(&node_kind, &node_config).await;
        let elapsed_ms = node_start.elapsed().as_millis() as u64;

        match result {
            Ok(output) => {
                let output_value = Self::node_output_value_inline(&node_kind, &output);
                self.outputs.insert(node_id.clone(), output_value.clone());
                self.emit_status(
                    &node_id,
                    NodeExecutionStatus::Completed,
                    1,
                    elapsed_ms,
                    Some(output_value),
                    None,
                );
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
                self.emit_status(
                    &node_id,
                    NodeExecutionStatus::Failed,
                    1,
                    elapsed_ms,
                    None,
                    Some(error.to_string()),
                );
                Err(error)
            }
        }
    }

    fn cumulative_usage(&self) -> Option<RunUsage> {
        Some(self.usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::graph::{compile, WorkflowDefinition, WorkflowEdgeSpec, WorkflowNodeSpec};
    use crate::workflow::node_events::NodeEventPublisher;

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
        CanvasImageResponse, CanvasVideoRequest, CanvasVideoResponse,
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
                text: "from canvas provider".to_string(),
                reasoning: Some("thinking about the tagline".to_string()),
                provider_id: "cvp-1".to_string(),
                model_id: "gpt-5.6-sol".to_string(),
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
            self.videos.lock().unwrap().push(request);
            Ok(CanvasVideoResponse {
                artifact_uri: "artifact://art_video".to_string(),
                mime: "video/mp4".to_string(),
                provider_id: "cvp-1".to_string(),
                model_id: "Wan2.2-T2V-A14B".to_string(),
                task_id: "job-77".to_string(),
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

    fn agent_with_bridge(bridge: RecordingBridge) -> WorkflowAgent {
        let definition = WorkflowDefinition {
            version: 1,
            nodes: vec![make_start("start-1"), make_end("end-1")],
            edges: vec![make_edge("e1", "start-1", "end-1")],
        };
        let compiled = compile(definition).unwrap();
        WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default())
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
            "parameter-extractor",
            "question-classifier",
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
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default());

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
                .with_cancel(cancel);

        let result = agent.think(0, &[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn workflow_agent_handles_tool_node_without_executor() {
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
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default());

        // Execute start
        agent.think(0, &[]).await.unwrap();

        // Execute tool (should handle missing executor gracefully)
        let decision = agent.think(1, &[]).await.unwrap();
        assert!(matches!(decision, AgentDecision::Continue));

        // Verify tool output indicates no executor
        let output = agent.outputs.get("tool-1").unwrap();
        assert!(!output.get("success").unwrap().as_bool().unwrap());
        assert!(output
            .get("error")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("no tool executor"));
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
            WorkflowAgent::new(compiled, Map::new(), None, NodeEventPublisher::default());

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
