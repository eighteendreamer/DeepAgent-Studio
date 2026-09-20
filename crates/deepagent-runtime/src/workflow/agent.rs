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
    CanvasCompletionRequest, CanvasEmbeddingRequest, CanvasImageRequest, CanvasModelBridge,
    CanvasRouteRequest,
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
            "embeddings" => self.execute_embedding(&resolved_config).await,
            // Media kinds without a backend job runner fail loudly instead of
            // reporting a fabricated completion through the passthrough arm.
            "video-gen" | "video-stitch" | "image-compare" | "audio" => {
                Err(deepagent_core::error::CoreError::invalid(format!(
                    "UnsupportedOperation: canvas node `{kind}` has no executor yet"
                )))
            }
            "http-request" => self.execute_http_request(&resolved_config).await,
            "tool" => self.execute_tool(&resolved_config).await,
            "knowledge-retrieval" => self.execute_knowledge_retrieval(&resolved_config).await,
            "iteration" | "loop" => self.execute_iteration(&resolved_config).await,
            _ => self.execute_passthrough(kind, &resolved_config),
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
        // The node contract plus the resolved input facts decide generate vs
        // edit; the model is never asked to infer it and an explicit but
        // conflicting operation fails instead of being rerouted.
        let mut input_kinds = Vec::new();
        if !prompt.trim().is_empty() {
            input_kinds.push("Text".to_string());
        }
        if !reference_images.is_empty() {
            input_kinds.push("Image".to_string());
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
            "imageUrl": response.data_url,
            "imageDataUrl": response.data_url,
            "mime": response.mime,
            "operation": response.operation,
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

    fn execute_passthrough(&self, kind: &str, config: &Map<String, Value>) -> Result<Value> {
        let mut result = Map::new();
        for (key, value) in config {
            result.insert(key.clone(), value.clone());
        }
        if result.is_empty() {
            result.insert(
                "__note".to_string(),
                Value::String(format!("{} node executed as passthrough", kind)),
            );
        }
        Ok(Value::Object(result))
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
        CanvasCompletionResponse, CanvasEmbeddingResponse, CanvasImageResponse,
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct RecordingBridge {
        completions: Arc<Mutex<Vec<CanvasCompletionRequest>>>,
        images: Arc<Mutex<Vec<CanvasImageRequest>>>,
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
                data_url: "data:image/png;base64,AA".to_string(),
                mime: "image/png".to_string(),
                provider_id: "cvp-1".to_string(),
                model_id: "gpt-image-2".to_string(),
                operation: if edited { "edit" } else { "generate" }.to_string(),
            })
        }

        fn route_operation(
            &self,
            request: CanvasRouteRequest,
        ) -> deepagent_core::error::Result<crate::workflow::CanvasRouteOutcome> {
            use crate::workflow::CanvasRouteOutcome;
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
        let mut agent = agent_with_bridge(bridge);
        let outcome = agent
            .execute_node_inline(
                "image-gen",
                &config(&[
                    ("imageModel", json!("cvp-1::gpt-image-2")),
                    ("prompt", json!("make it night")),
                    ("referenceImages", json!(["data:image/png;base64,BB"])),
                ]),
            )
            .await
            .expect("image edit");
        assert_eq!(outcome["operation"], "edit");
        assert_eq!(outcome["imageUrl"], "data:image/png;base64,AA");
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
    async fn creative_media_nodes_fail_instead_of_faking_completion() {
        let mut agent = agent_with_bridge(RecordingBridge::default());
        for kind in ["video-gen", "video-stitch", "image-compare", "audio"] {
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
