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
use deepagent_models::client::ModelClient;
use serde_json::{Map, Value};

use super::graph::CompiledWorkflow;
use super::knowledge::KnowledgeRetriever;
use super::node_events::{NodeEventPublisher, NodeExecutionEvent, NodeExecutionStatus};
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

    async fn execute_node_inline(&mut self, kind: &str, config: &Map<String, Value>) -> Result<Value> {
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
            "llm" | "agent" | "agent-v2" => {
                self.execute_llm(kind, &resolved_config).await
            }
            "http-request" => self.execute_http_request(&resolved_config).await,
            "tool" => self.execute_tool_stub(kind),
            "knowledge-retrieval" => self.execute_knowledge_retrieval(&resolved_config).await,
            "iteration" | "loop" => self.execute_iteration_stub(kind),
            _ => self.execute_passthrough(kind, &resolved_config),
        }
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
        let client = match &self.model {
            Some(c) => c.clone(),
            None => {
                return Ok(serde_json::json!({
                    "text": format!("[{}] no model client configured for workflow run", kind),
                    "usage": { "prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0 }
                }));
            }
        };

        let model_name = self
            .model_name
            .as_deref()
            .unwrap_or("deepseek-chat");

        let system_prompt = config
            .get("llmSystemPrompt")
            .or_else(|| config.get("agentSystemPrompt"))
            .or_else(|| config.get("agentV2SystemPrompt"))
            .and_then(Value::as_str)
            .unwrap_or("");

        let user_prompt = config
            .get("llmPrompt")
            .or_else(|| config.get("agentTask"))
            .and_then(Value::as_str)
            .unwrap_or("");

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

    async fn execute_http_request(&self, config: &Map<String, Value>) -> Result<Value> {
        let method = config
            .get("httpMethod")
            .and_then(Value::as_str)
            .unwrap_or("GET");
        let url = config
            .get("httpUrl")
            .and_then(Value::as_str)
            .unwrap_or("");
        let body = config
            .get("httpBody")
            .and_then(Value::as_str)
            .unwrap_or("");
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
            if !body.is_empty() && matches!(req_method, reqwest::Method::POST | reqwest::Method::PUT | reqwest::Method::PATCH) {
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

                    let body_json: Value = serde_json::from_str(&resp_body)
                        .unwrap_or(Value::String(resp_body));

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

    fn execute_tool_stub(&self, kind: &str) -> Result<Value> {
        Ok(serde_json::json!({
            "text": format!("[tool] {} is not yet connected", kind),
            "json": null
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

    fn execute_iteration_stub(&self, kind: &str) -> Result<Value> {
        Ok(serde_json::json!({
            "output": [],
            "__note": format!("{} node requires sub-graph execution", kind)
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
}
