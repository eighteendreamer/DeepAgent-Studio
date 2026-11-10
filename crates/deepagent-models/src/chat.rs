//! Provider-facing DeepSeek Responses request/response types.
//!
//! The internal `Message` projection remains provider-neutral for the runtime
//! and UI, while request serialization emits Responses input items.

use serde::{Deserialize, Serialize, Serializer};

use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::Message;
use deepagent_core::response_item::{ResponseInputItem, ResponseOutputItem};
use deepagent_tools::{validate_tool_name, validate_tool_schema};

/// Validate DeepSeek Responses' optional end-user identifier.
///
/// The provider accepts 1-512 ASCII letters, digits, hyphens, or underscores.
/// The error deliberately omits the value so identifiers cannot leak to logs.
pub fn validate_responses_user_identifier(value: &str) -> Result<()> {
    let valid = (1..=512).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if !valid {
        return Err(CoreError::invalid(
            "Responses user must be 1-512 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(())
}

/// DeepSeek Responses reasoning effort exposed to users.
///
/// The four variants serialize directly to the official Responses values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Thinking disabled.
    None,
    /// Thinking enabled at low effort.
    Low,
    /// Thinking enabled at high effort.
    #[default]
    High,
    /// Thinking enabled at maximum effort.
    Max,
}

impl ReasoningEffort {
    /// Stable canonical label used in settings/UI and Responses requests.
    pub const fn label(self) -> &'static str {
        match self {
            ReasoningEffort::None => "none",
            ReasoningEffort::Low => "low",
            ReasoningEffort::High => "high",
            ReasoningEffort::Max => "max",
        }
    }

    /// Parse one of the canonical provider values without accepting aliases.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "low" => Some(Self::Low),
            "high" => Some(Self::High),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// DeepSeek OpenAI-format thinking toggle: `{"thinking":{"type":"enabled"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThinkingToggle {
    #[serde(rename = "type")]
    pub kind: String,
}

/// A tool/function the model may call, in JSON-schema form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    /// Registered function or provider projection (`custom` / `web_search`).
    #[serde(rename = "type")]
    pub kind: String,
    /// The function definition.
    pub function: FunctionSchema,
}

impl ToolSchema {
    /// Build a function tool schema.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            kind: "function".to_string(),
            function: FunctionSchema {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

/// Validate every function tool that will be projected to a provider wire.
///
/// The registry normally validates descriptors before they become visible,
/// but requests can also be assembled directly by SDK and integration callers.
/// Keeping this final guard on the provider-neutral request type prevents an
/// invalid schema from reaching either Responses or Chat serialization.
pub(crate) fn validate_function_tool_schemas(tools: &[ToolSchema]) -> Result<()> {
    let mut names = std::collections::BTreeSet::new();
    for (index, tool) in tools
        .iter()
        .enumerate()
        .filter(|(_, tool)| tool.kind == "function")
    {
        validate_tool_name(&tool.function.name)
            .map_err(|error| CoreError::invalid(format!("tools[{index}]: {error}")))?;
        if !names.insert(&tool.function.name) {
            return Err(CoreError::invalid(format!(
                "tools[{index}]: duplicate function tool name"
            )));
        }
        validate_tool_schema(&tool.function.name, &tool.function.parameters)?;
    }
    Ok(())
}

/// A function definition advertised to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSchema {
    /// Function name.
    pub name: String,
    /// Natural-language description.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
}

/// A DeepSeek Responses API request.
#[derive(Debug, Clone, PartialEq)]
pub struct ResponseRequest {
    /// Target model id (e.g. `"deepseek-v4-flash"` / `"deepseek-v4-pro"`).
    pub model: String,
    /// System/developer instructions. Serialized as Responses `instructions`.
    pub instructions: Option<String>,
    /// Ordered Responses input items (`message`, `reasoning`, function/custom
    /// tool calls and paired outputs, web search calls).
    pub input: Vec<ResponseInputItem>,
    /// Whether to stream the response as SSE.
    pub stream: bool,
    /// Sampling temperature.
    pub temperature: Option<f32>,
    /// Max output tokens.
    pub max_output_tokens: Option<u32>,
    /// Responses API reasoning effort (`none` / `low` / `high` / `max`).
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Advertised tools.
    pub tools: Vec<ToolSchema>,
    /// Responses API tool choice. Omitted means provider default (`auto`).
    pub tool_choice: Option<serde_json::Value>,
    /// Responses API nucleus sampling.
    pub top_p: Option<f32>,
    /// Responses API top logprobs.
    pub top_logprobs: Option<u8>,
    /// Responses API text configuration, e.g. json_schema.
    pub text: Option<serde_json::Value>,
    /// Optional end-user identifier.
    pub user: Option<String>,
    /// Attachment id → base64 data URL, resolved by the caller that owns
    /// attachment storage. Used to materialize `input_image` content parts;
    /// empty when the turn carries no image attachments.
    pub attachment_data_urls: std::collections::BTreeMap<String, String>,
    /// Original item order used by the compatibility Chat projection.
    ///
    /// `input` is repaired at the Responses provider boundary so every tool
    /// call has an adjacent output. That repair must not be used to rebuild a
    /// legacy Chat assistant turn: native history may contain narration
    /// between a call and its output while still belonging to one turn.
    chat_projection_input: Vec<ResponseInputItem>,
}

impl Serialize for ResponseRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        self.validate_tool_image_resolution()
            .map_err(serde::ser::Error::custom)?;
        let input = response_input_items_to_wire(&self.input, &self.attachment_data_urls)
            .map_err(serde::ser::Error::custom)?;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("model", &self.model)?;
        if let Some(value) = &self.instructions {
            map.serialize_entry("instructions", &value)?;
        }
        map.serialize_entry("input", &input)?;
        map.serialize_entry("stream", &self.stream)?;
        if let Some(value) = self.temperature {
            map.serialize_entry("temperature", &value)?;
        }
        if let Some(value) = self.top_p {
            map.serialize_entry("top_p", &value)?;
        }
        if let Some(value) = self.max_output_tokens {
            map.serialize_entry("max_output_tokens", &value)?;
        }
        if let Some(value) = self.top_logprobs {
            map.serialize_entry("top_logprobs", &value)?;
        }
        if let Some(value) = self.reasoning_effort {
            map.serialize_entry("reasoning", &serde_json::json!({"effort": value.label()}))?;
        }
        if !self.tools.is_empty() {
            map.serialize_entry("tools", &response_tools_to_wire(&self.tools))?;
        }
        if let Some(value) = &self.tool_choice {
            map.serialize_entry("tool_choice", value)?;
        }
        if let Some(value) = &self.text {
            map.serialize_entry("text", value)?;
        }
        if let Some(value) = &self.user {
            map.serialize_entry("user", value)?;
        }
        map.end()
    }
}

/// The one Responses tool serializer, also used by read-only wire evidence.
pub(crate) fn response_tools_to_wire(tools: &[ToolSchema]) -> Vec<serde_json::Value> {
    tools.iter().map(|tool| match tool.kind.as_str() {
        "function" => serde_json::json!({"type":"function","name":tool.function.name,"description":tool.function.description,"parameters":tool.function.parameters}),
        "custom" => serde_json::json!({"type":"custom","name":tool.function.name,"description":tool.function.description,"format":{"type":"text"}}),
        "web_search" => serde_json::json!({"type":"web_search"}),
        _ => serde_json::json!({"type": tool.kind}),
    }).collect()
}

fn response_input_items_to_wire(
    items: &[ResponseInputItem],
    data_urls: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<serde_json::Value>> {
    items
        .iter()
        .map(|item| {
            Ok(match item {
                ResponseInputItem::Message { role, content } => {
                    serde_json::json!({
                        "type": "message",
                        "role": role,
                        "content": content,
                    })
                }
                ResponseInputItem::InputMessage {
                    role,
                    content,
                    attachments,
                } => response_input_message_to_wire(role, content, attachments, data_urls)?,
                ResponseInputItem::Reasoning { id, content } => {
                    let mut value = serde_json::json!({
                        "type": "reasoning",
                        "content": [{
                            "type": "reasoning_text",
                            "text": content,
                        }],
                    });
                    if let Some(id) = id {
                        value["id"] = serde_json::Value::String(id.clone());
                    }
                    value
                }
                ResponseInputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    serde_json::json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": arguments,
                    })
                }
                ResponseInputItem::FunctionCallOutput { call_id, output } => {
                    serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": tool_output_to_wire(output, data_urls),
                    })
                }
                ResponseInputItem::CustomToolCall {
                    call_id,
                    name,
                    input,
                } => {
                    serde_json::json!({
                        "type": "custom_tool_call",
                        "call_id": call_id,
                        "name": name,
                        "input": input,
                    })
                }
                ResponseInputItem::CustomToolCallOutput { call_id, output } => {
                    serde_json::json!({
                        "type": "custom_tool_call_output",
                        "call_id": call_id,
                        "output": tool_output_to_wire(output, data_urls),
                    })
                }
                ResponseInputItem::WebSearchCall { id, status, action } => {
                    let mut value = serde_json::json!({
                        "type": "web_search_call",
                        "id": id,
                        "status": status,
                    });
                    if let Some(action) = action {
                        value["action"] = action.clone();
                    }
                    value
                }
                ResponseInputItem::ToolSearchCall { .. }
                | ResponseInputItem::ToolSearchOutput { .. } => serde_json::to_value(item)
                    .expect("native discovery items contain serializable data"),
            })
        })
        .collect()
}

/// Map the neutral durable tool content at the existing Responses wire seam.
fn tool_output_to_wire(
    output: &deepagent_core::tool_content::ToolResultBody,
    data_urls: &std::collections::BTreeMap<String, String>,
) -> serde_json::Value {
    use deepagent_core::tool_content::{ToolResultBody, ToolResultPart};
    match output {
        ToolResultBody::Text(text) => text.clone().into(),
        ToolResultBody::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ToolResultPart::Text { text } => {
                    serde_json::json!({"type":"input_text", "text":text})
                }
                ToolResultPart::Image { attachment } => serde_json::json!({
                    "type":"input_image", "image_url":data_urls.get(&attachment.id), "detail":"auto"
                }),
            })
            .collect(),
    }
}

/// Render a validated Responses image message without dropping references.
fn response_input_message_to_wire(
    role: &str,
    content: &str,
    attachments: &[deepagent_core::message::MessageAttachment],
    data_urls: &std::collections::BTreeMap<String, String>,
) -> Result<serde_json::Value> {
    let mut image_urls = Vec::new();
    for attachment in attachments
        .iter()
        .filter(|attachment| attachment.kind == "image")
    {
        crate::client::WireMode::Responses.validate_image_role(role)?;
        let url = data_urls.get(&attachment.id).filter(|url| !url.trim().is_empty())
            .ok_or_else(|| CoreError::invalid("image_input_message_unresolved: image message requires a resolved image reference; no image was sent"))?;
        image_urls.push(url);
    }
    if image_urls.is_empty() {
        return Ok(serde_json::json!({
            "type": "message",
            "role": role,
            "content": content,
        }));
    }
    let mut parts = Vec::new();
    if !content.is_empty() {
        // Per the DeepSeek Vision guide, Responses content parts use the
        // `input_text` type name (OpenAI Responses semantics), not the Chat
        // Completions `text` type. A `text`-typed block inside the parts
        // array is not recognized by the provider and breaks image messages.
        parts.push(serde_json::json!({ "type": "input_text", "text": content }));
    }
    for url in image_urls {
        parts.push(serde_json::json!({
            "type": "input_image",
            "image_url": url,
            "detail": "auto",
        }));
    }
    Ok(serde_json::json!({
        "type": "message",
        "role": role,
        "content": parts,
    }))
}

impl ResponseRequest {
    /// Build a non-streaming request for `model` with `messages`.
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        let (instructions, input) = crate::responses::response_items_from_messages(&messages);
        Self::from_response_items(model, instructions, input)
    }

    /// Build a request from a system/developer instruction string and a single
    /// user input item, without routing through the legacy chat `Message`
    /// projection.
    pub fn with_instructions_and_user_input(
        model: impl Into<String>,
        instructions: impl Into<String>,
        user_input: impl Into<String>,
    ) -> Self {
        Self::from_response_items(
            model,
            Some(instructions.into()),
            vec![ResponseInputItem::Message {
                role: "user".to_string(),
                content: user_input.into(),
            }],
        )
    }

    /// Build a request from already-normalized Responses input items.
    pub fn from_response_items(
        model: impl Into<String>,
        instructions: Option<String>,
        input: Vec<ResponseInputItem>,
    ) -> Self {
        let chat_projection_input = input.clone();
        Self {
            model: model.into(),
            instructions,
            // Provider-boundary guard: never resend a dangling Responses tool
            // call after a crash, cancellation, or partial history replay.
            input: crate::responses::repair_tool_call_pairs(&input),
            stream: false,
            temperature: None,
            max_output_tokens: None,
            reasoning_effort: None,
            tools: Vec::new(),
            tool_choice: None,
            top_p: None,
            top_logprobs: None,
            text: None,
            user: None,
            attachment_data_urls: std::collections::BTreeMap::new(),
            chat_projection_input,
        }
    }

    /// Attach the resolved attachment id → data URL map, materialized into
    /// `input_image` content parts at serialization time.
    pub fn with_attachment_data_urls(
        mut self,
        data_urls: std::collections::BTreeMap<String, String>,
    ) -> Self {
        self.attachment_data_urls = data_urls;
        self
    }

    /// Enable streaming (builder style). Also requests usage in the stream.
    pub fn streaming(mut self) -> Self {
        self.stream = true;
        self
    }

    /// Attach tool schemas (builder style).
    pub fn with_tools(mut self, tools: Vec<ToolSchema>) -> Self {
        self.tools = tools;
        self
    }

    /// Refresh the Responses `instructions`/`input` projection after the
    /// runtime mutates its provider-neutral conversation state. This keeps
    /// retry-time changes such as `max_output_tokens` escalation intact.
    pub fn replace_messages(&mut self, messages: &[Message]) {
        let (instructions, input) = crate::responses::response_items_from_messages(messages);
        self.instructions = instructions;
        self.chat_projection_input = input.clone();
        self.input = crate::responses::repair_tool_call_pairs(&input);
    }

    /// Replace the history fields from a freshly projected request.
    ///
    /// Retry and compaction paths must update both the repaired Responses
    /// sequence and the original order retained for the Chat compatibility
    /// route; assigning only `input` would leave a stale projection snapshot.
    pub fn replace_history_from(&mut self, other: &Self) {
        self.instructions = other.instructions.clone();
        self.input = other.input.clone();
        self.chat_projection_input = other.chat_projection_input.clone();
        self.attachment_data_urls = other.attachment_data_urls.clone();
    }

    /// Return the original order when it has complete, ordered tool pairs;
    /// otherwise fall back to the repaired sequence to avoid emitting a
    /// dangling call on the Chat wire.
    pub(crate) fn chat_projection_items(&self) -> Vec<ResponseInputItem> {
        let Ok(projection) =
            crate::responses::normalize_tool_output_kinds(&self.chat_projection_input)
        else {
            return self.input.clone();
        };
        if crate::responses::chat_projection_history_is_safe(&projection)
            && crate::responses::repair_tool_call_pairs(&projection) == self.input
        {
            projection.into_owned()
        } else {
            self.input.clone()
        }
    }

    /// Set temperature (builder style).
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    pub fn with_top_p(mut self, value: f32) -> Self {
        self.top_p = Some(value);
        self
    }

    pub fn with_top_logprobs(mut self, value: u8) -> Self {
        self.top_logprobs = Some(value);
        self
    }

    pub fn with_text(mut self, value: serde_json::Value) -> Self {
        self.text = Some(value);
        self
    }

    pub fn with_tool_choice(mut self, value: serde_json::Value) -> Self {
        self.tool_choice = Some(value);
        self
    }

    pub fn with_max_output_tokens(mut self, n: u32) -> Self {
        self.max_output_tokens = Some(n);
        self
    }

    /// Apply the canonical DeepSeek Responses reasoning effort.
    ///
    /// `max_output_tokens` remains independent; selecting an effort never
    /// invents a token ceiling or rewrites an explicit one.
    pub fn with_reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = Some(effort);
        self
    }

    /// Validate provider-bound fields after defaults have been applied.
    pub fn validate(&self) -> Result<()> {
        self.validate_input_presence()?;
        self.validate_image_roles(crate::client::WireMode::Responses)?;
        if let Some(user) = &self.user {
            validate_responses_user_identifier(user)?;
        }
        validate_function_tool_schemas(&self.tools)?;
        crate::responses::validate_tool_call_history(&self.input)?;
        self.validate_tool_image_resolution()?;
        Ok(())
    }

    /// Role-only gate used before reading bytes and before Chat projection.
    /// Resolution remains the caller's job until final serialization.
    pub(crate) fn validate_image_roles(&self, wire_mode: crate::client::WireMode) -> Result<()> {
        for item in &self.input {
            if let ResponseInputItem::InputMessage {
                role, attachments, ..
            } = item
            {
                if attachments
                    .iter()
                    .any(|attachment| attachment.kind == "image")
                {
                    wire_mode.validate_image_role(role)?;
                }
            }
        }
        Ok(())
    }

    fn validate_tool_image_resolution(&self) -> Result<()> {
        for item in &self.input {
            if let ResponseInputItem::FunctionCallOutput { output, .. }
            | ResponseInputItem::CustomToolCallOutput { output, .. } = item
            {
                for attachment in output.attachments() {
                    if attachment.kind != "image"
                        || !self
                            .attachment_data_urls
                            .get(&attachment.id)
                            .is_some_and(|url| !url.is_empty())
                    {
                        return Err(CoreError::invalid("image_input_tool_output_unresolved: image output requires a resolved image reference; no tool image was sent"));
                    }
                }
            }
        }
        Ok(())
    }

    /// Shared by Responses dispatch and the compatibility Chat conversion.
    /// Inspect the final public input, never a stale pre-repair projection.
    pub(crate) fn validate_input_presence(&self) -> Result<()> {
        if self.input.is_empty()
            && !self
                .instructions
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(CoreError::invalid(
                "Responses requires input items or nonblank instructions",
            ));
        }
        Ok(())
    }
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural end of message.
    Stop,
    /// Hit the token limit.
    Length,
    /// Stopped to emit tool calls.
    ToolCalls,
    /// Filtered by content policy.
    ContentFilter,
    /// Generation was interrupted by the provider; not proof of local user cancellation.
    Aborted,
    /// The provider interrupted generation because inference resources were unavailable.
    InsufficientSystemResource,
}

/// Token accounting returned by the provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Prompt tokens.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Completion tokens.
    #[serde(default)]
    pub completion_tokens: u32,
    /// Reasoning tokens included in `completion_tokens` by Responses.
    /// Kept separately for diagnostics and UI; never added again for billing.
    #[serde(default)]
    pub reasoning_tokens: u32,
    /// Total tokens.
    #[serde(default)]
    pub total_tokens: u32,
    /// DeepSeek: prompt tokens served from the context cache (a "hit").
    #[serde(default)]
    pub prompt_cache_hit_tokens: u32,
    /// DeepSeek: prompt tokens NOT served from cache (a "miss").
    #[serde(default)]
    pub prompt_cache_miss_tokens: u32,
}

impl Usage {
    /// Fold another answered call into `self`.
    ///
    /// Provider semantics (and the rule the run loop already follows):
    /// prompt/total/cache figures are *context totals* of the most recent call,
    /// so they are replaced; output and reasoning figures are per-call amounts,
    /// so they add up. Use this wherever several calls must be reported as one.
    pub fn absorb(&mut self, fresh: Self) {
        self.prompt_tokens = fresh.prompt_tokens;
        self.completion_tokens += fresh.completion_tokens;
        self.reasoning_tokens += fresh.reasoning_tokens;
        self.total_tokens = fresh.total_tokens;
        self.prompt_cache_hit_tokens = fresh.prompt_cache_hit_tokens;
        self.prompt_cache_miss_tokens = fresh.prompt_cache_miss_tokens;
    }
}

/// A fully assembled (non-streaming, or post-accumulation) Responses result.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// Provider-native Responses output items. Persistence and recovery retain
    /// these exact item semantics; text/tool projections are derived from this
    /// vector at the compatibility boundary.
    pub output_items: Vec<ResponseOutputItem>,
    /// Why generation finished, if reported.
    pub finish_reason: Option<FinishReason>,
    /// Token usage, if reported.
    pub usage: Option<Usage>,
    /// Raw provider Responses usage object, preserved for diagnostics and
    /// future billing fields without changing the existing UI projection.
    pub raw_usage: Option<serde_json::Value>,
}

impl Response {
    /// Reject explicit provider-failure terminals before adopting an answer.
    /// Raw client assembly deliberately preserves these typed outcomes and
    /// partial text/usage for inspection. Callers must not interpret `Ok(Response)`
    /// as generation success. Length keeps its existing caller-owned recovery;
    /// this method does not retry, cancel a run, or authorize any tool.
    pub fn into_generation_result(self) -> Result<Self> {
        let failure = match self.finish_reason {
            Some(FinishReason::ContentFilter) => {
                Some(("content_filter", "model stopped due to content filter"))
            }
            Some(FinishReason::Aborted) => {
                Some(("aborted", "model generation aborted by provider"))
            }
            Some(FinishReason::InsufficientSystemResource) => Some((
                "insufficient_system_resource",
                "model generation stopped due to insufficient system resource",
            )),
            _ => None,
        };
        match failure {
            Some((code, message)) => Err(CoreError::provider(None, Some(code.into()), message)),
            None => Ok(self),
        }
    }

    pub(crate) fn from_parts(
        output_items: Vec<ResponseOutputItem>,
        finish_reason: Option<FinishReason>,
        usage: Option<Usage>,
        raw_usage: Option<serde_json::Value>,
    ) -> Self {
        Self {
            output_items,
            finish_reason,
            usage,
            raw_usage,
        }
    }

    /// Project the assistant's visible text from native Responses output items.
    ///
    /// This is the preferred helper for one-shot classification, title,
    /// compaction and review calls that only need text.
    pub fn output_text_projection(&self) -> String {
        let mut content = String::new();
        for item in &self.output_items {
            if let ResponseOutputItem::Message {
                role,
                content: text,
            } = item
            {
                if role == "assistant" {
                    content.push_str(text);
                }
            }
        }
        content
    }

    /// Project the assistant's reasoning text from native Responses output
    /// items.
    pub fn reasoning_text_projection(&self) -> Option<String> {
        let mut reasoning: Option<String> = None;
        for item in &self.output_items {
            if let ResponseOutputItem::Reasoning { content: text, .. } = item {
                if !text.is_empty() {
                    reasoning = Some(match reasoning.take() {
                        Some(mut existing) => {
                            existing.push_str(text);
                            existing
                        }
                        None => text.clone(),
                    });
                }
            }
        }
        reasoning
    }

    /// Extract local tool invocations from native Responses output items.
    pub fn tool_invocations_from_items(&self) -> Vec<(String, String, serde_json::Value)> {
        let mut invocations = Vec::new();
        for item in &self.output_items {
            match item {
                ResponseOutputItem::FunctionCall {
                    call_id,
                    name,
                    arguments,
                } => {
                    invocations.push((
                        call_id.clone(),
                        name.clone(),
                        parse_function_arguments(arguments),
                    ));
                }
                ResponseOutputItem::CustomToolCall {
                    call_id,
                    name,
                    input,
                } => {
                    invocations.push((
                        call_id.clone(),
                        name.clone(),
                        serde_json::json!({ "patch": input }),
                    ));
                }
                ResponseOutputItem::ToolSearchCall {
                    call_id: Some(call_id),
                    execution: deepagent_core::response_item::ToolSearchExecution::Client,
                    status,
                    arguments,
                    ..
                } if status
                    .as_deref()
                    .map_or(true, |status| status == "completed")
                    && !call_id.trim().is_empty()
                    && arguments.is_object() =>
                {
                    invocations.push((call_id.clone(), "tool_search".into(), arguments.clone()));
                }
                _ => {}
            }
        }
        invocations
    }
}

fn parse_function_arguments(raw: &str) -> serde_json::Value {
    let args = if raw.trim().is_empty() {
        "{}"
    } else {
        raw.trim()
    };
    serde_json::from_str(args).unwrap_or_else(|error| {
        serde_json::json!({
            "__invalid_tool_arguments__": true,
            "raw": args,
            "parse_error": error.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_wire_materializes_user_image_attachment() {
        use deepagent_core::message::MessageAttachment;
        let item = ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "what is this?".into(),
            attachments: vec![MessageAttachment {
                sha256: None,
                id: "att_1".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
            }],
        };
        let mut urls = std::collections::BTreeMap::new();
        urls.insert(
            "att_1".to_string(),
            "data:image/png;base64,AAAA".to_string(),
        );
        let wire = response_input_items_to_wire(&[item], &urls).unwrap();
        let content = &wire[0]["content"];
        assert!(content.is_array(), "user message with image → parts array");
        // Responses content parts use the `input_*` type names (per the
        // DeepSeek Vision guide): `input_text` for text, `input_image` for
        // images. `text` would be the Chat Completions spelling and is not
        // recognized inside the Responses parts array.
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "data:image/png;base64,AAAA");
        assert_eq!(content[1]["detail"], "auto");
    }

    #[test]
    fn responses_wire_rejects_image_for_unsupported_role() {
        use deepagent_core::message::MessageAttachment;
        let item = ResponseInputItem::InputMessage {
            role: "assistant".into(),
            content: "hi".into(),
            attachments: vec![MessageAttachment {
                sha256: None,
                id: "att_1".into(),
                kind: "image".into(),
                media_type: None,
                path: None,
            }],
        };
        let mut urls = std::collections::BTreeMap::new();
        urls.insert(
            "att_1".to_string(),
            "data:image/png;base64,AAAA".to_string(),
        );
        let wire = response_input_items_to_wire(&[item], &urls);
        assert!(
            wire.is_err(),
            "illegal images must not become text-only messages"
        );
    }

    #[test]
    fn responses_wire_rejects_unresolved_image_without_text_fallback() {
        use deepagent_core::message::MessageAttachment;
        let item = ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "hi".into(),
            attachments: vec![MessageAttachment {
                sha256: None,
                id: "missing".into(),
                kind: "image".into(),
                media_type: None,
                path: None,
            }],
        };
        let wire = response_input_items_to_wire(&[item], &std::collections::BTreeMap::new());
        assert!(
            wire.is_err(),
            "unresolved images must not become text-only messages"
        );
    }

    #[test]
    fn reasoning_effort_flags() {
        assert_eq!(ReasoningEffort::None.label(), "none");
        assert_eq!(ReasoningEffort::Low.label(), "low");
        assert_eq!(ReasoningEffort::High.label(), "high");
        assert_eq!(ReasoningEffort::Max.label(), "max");
        assert_eq!(ReasoningEffort::default(), ReasoningEffort::High);
    }

    #[test]
    fn reasoning_effort_accepts_only_canonical_values() {
        for (label, effort) in [
            ("none", ReasoningEffort::None),
            ("low", ReasoningEffort::Low),
            ("high", ReasoningEffort::High),
            ("max", ReasoningEffort::Max),
        ] {
            assert_eq!(ReasoningEffort::parse(label), Some(effort));
            assert_eq!(
                serde_json::from_str::<ReasoningEffort>(&format!("\"{label}\"")).unwrap(),
                effort
            );
        }
        for alias in ["minimal", "medium", "xhigh", "simple", "deep", " HIGH "] {
            assert!(
                ReasoningEffort::parse(alias).is_none(),
                "must reject {alias}"
            );
            assert!(
                serde_json::from_str::<ReasoningEffort>(&format!("\"{alias}\"")).is_err(),
                "serde must reject {alias}"
            );
        }
    }

    #[test]
    fn reasoning_effort_serializes_canonical_wire_values() {
        for effort in [
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ] {
            let request = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
                .with_reasoning_effort(effort);
            let value = serde_json::to_value(request).unwrap();
            assert_eq!(value["reasoning"]["effort"], effort.label());
        }
    }

    #[test]
    fn request_builder_and_serialization() {
        let req = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .streaming()
            .with_reasoning_effort(ReasoningEffort::High)
            .with_temperature(0.2)
            .with_tools(vec![ToolSchema::function(
                "reverse",
                "reverse text",
                serde_json::json!({"type": "object"}),
            )]);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "deepseek-v4-flash");
        assert_eq!(json["stream"], true);
        assert_eq!(json["reasoning"]["effort"], "high");
        assert!(json.get("max_output_tokens").is_none());
        assert!((json["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(json["tools"][0]["name"], "reverse");
    }

    #[test]
    fn responses_request_validation_rejects_invalid_function_schema() {
        for parameters in [
            serde_json::Value::Null,
            serde_json::json!("not-a-schema"),
            serde_json::json!([]),
            serde_json::json!({"type": "not-a-json-schema-type"}),
        ] {
            let request = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
                .with_tools(vec![ToolSchema::function("invalid", "invalid", parameters)]);
            assert!(request.validate().is_err());
        }
    }

    #[test]
    fn responses_request_validation_accepts_complex_function_schema() {
        let request = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .with_tools(vec![ToolSchema::function(
                "search",
                "search docs",
                serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }),
            )]);
        assert!(request.validate().is_ok());
    }

    #[test]
    fn none_effort_disables_reasoning() {
        let req = ResponseRequest::new("deepseek-v4-flash", vec![Message::user("hi")])
            .with_reasoning_effort(ReasoningEffort::None);
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["reasoning"]["effort"], "none");
        assert!(json.get("max_output_tokens").is_none());
    }

    #[test]
    fn request_builder_accepts_native_instruction_and_user_item() {
        let req = ResponseRequest::with_instructions_and_user_input(
            "deepseek-v4-flash",
            "Classify safely.",
            "rm -rf /tmp",
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["instructions"], "Classify safely.");
        assert_eq!(json["input"][0]["type"], "message");
        assert_eq!(json["input"][0]["role"], "user");
        assert_eq!(json["input"][0]["content"], "rm -rf /tmp");
    }

    #[test]
    fn serializes_reasoning_input_as_content_parts() {
        let req = ResponseRequest::from_response_items(
            "deepseek-v4-flash",
            Some("sys".into()),
            vec![
                ResponseInputItem::Reasoning {
                    id: Some("rs_1".into()),
                    content: "The prior assistant thought privately.".into(),
                },
                ResponseInputItem::Message {
                    role: "assistant".into(),
                    content: "你好，有什么可以帮你？".into(),
                },
                ResponseInputItem::Message {
                    role: "user".into(),
                    content: "你好".into(),
                },
            ],
        );
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["input"][0]["type"], "reasoning");
        assert_eq!(json["input"][0]["id"], "rs_1");
        assert!(json["input"][0]["content"].is_array());
        assert_eq!(json["input"][0]["content"][0]["type"], "reasoning_text");
        assert_eq!(
            json["input"][0]["content"][0]["text"],
            "The prior assistant thought privately."
        );
    }

    #[test]
    fn usage_defaults_to_zero() {
        let u: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(u, Usage::default());
    }

    #[test]
    fn tool_invocations_are_derived_directly_from_response_items() {
        let response = Response::from_parts(
            vec![
                ResponseOutputItem::Message {
                    role: "assistant".into(),
                    content: "working".into(),
                },
                ResponseOutputItem::FunctionCall {
                    call_id: "call-1".into(),
                    name: "read_file".into(),
                    arguments: r#"{"path":"README.md"}"#.into(),
                },
                ResponseOutputItem::CustomToolCall {
                    call_id: "call-2".into(),
                    name: "apply_patch".into(),
                    input: "*** Begin Patch\n*** End Patch".into(),
                },
            ],
            None,
            None,
            None,
        );

        let invocations = response.tool_invocations_from_items();
        assert_eq!(invocations.len(), 2);
        assert_eq!(
            invocations[0],
            (
                "call-1".into(),
                "read_file".into(),
                serde_json::json!({"path": "README.md"})
            )
        );
        assert_eq!(invocations[1].0, "call-2");
        assert_eq!(invocations[1].1, "apply_patch");
        assert_eq!(
            invocations[1].2,
            serde_json::json!({"patch": "*** Begin Patch\n*** End Patch"})
        );
    }
}
