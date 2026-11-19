pub(super) mod request;

use super::*;
use crate::client::files::ImageRequest;
use crate::transport::TransportRequest;

pub(super) struct ResponsesCodec;

#[async_trait::async_trait]
impl WireCodec for ResponsesCodec {
    async fn stream(
        &self,
        client: &ModelClient,
        mut request: ResponseRequest,
        observer: &mut dyn DeltaObserver,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Response> {
        request.stream = true;
        client.prepare_response_sampling(&mut request)?;
        request.validate()?;
        client.validate_output_tokens(request.max_output_tokens)?;
        client.validate_response_input_modalities(&request)?;
        crate::client::log_response_input_pairs(&request);
        crate::image_input::validate_response_payloads(request.clone(), cancel.clone()).await?;
        client
            .prepare_response_files_for_attempt(&mut request, cancel.clone())
            .await?;
        let body = client.encode_response_body(&request)?;
        let mut transport = TransportRequest {
            auth: self.auth(),
            url: client.config().endpoint(),
            api_key: client.config().api_key.clone(),
            body,
        };
        if !request.image_file_ids.is_empty() {
            return client
                .stream_files_aware_attempt(
                    self,
                    ImageRequest::Responses(&mut request),
                    &mut transport,
                    observer,
                    cancel,
                )
                .await;
        }
        client
            .stream_attempt(self, transport, observer, cancel)
            .await
    }

    fn decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(crate::ResponseAccumulator::new())
    }

    fn validate_image_role(&self, role: &str) -> Result<()> {
        if matches!(role, "user" | "developer") {
            return Ok(());
        }
        Err(deepagent_core::error::CoreError::invalid("image_input_role_unsupported: Responses images require a user or developer message, or a function/custom tool output"))
    }

    fn sampling(
        &self,
        client: &ModelClient,
        request: &ResponseRequest,
    ) -> Option<crate::EffectiveSamplingConfig> {
        // Tool kind is explicit; evidence and dispatch must not infer it by name.
        let supported_tools = request.tools.iter().any(|tool| {
            tool.kind == "function"
                || (tool.kind == "custom" && tool.function.name == "apply_patch")
                || (tool.kind == "web_search" && request.model == "deepseek-v4-pro")
        });
        Some(crate::resolve_responses_sampling(
            &request.model,
            request.top_p.or(client.config().defaults.top_p),
            client.reasoning_effort_hint(&request.model, request.reasoning_effort),
            supported_tools,
            request
                .tool_choice
                .as_ref()
                .or(client.config().defaults.tool_choice.as_ref()),
        ))
    }

    fn legacy_reasoning_default(
        &self,
        capability: &crate::ModelCapability,
    ) -> Option<ReasoningEffort> {
        capability
            .effort
            .as_ref()?
            .default_level
            .as_deref()
            .and_then(ReasoningEffort::parse_responses)
            .filter(|effort| *effort != ReasoningEffort::None)
    }

    fn resolve_reasoning_effort(
        &self,
        capability: &crate::ModelCapability,
        requested: Option<ReasoningEffort>,
    ) -> Result<Option<ReasoningEffort>> {
        capability.resolve_reasoning_effort(requested)
    }

    fn name(&self) -> &'static str {
        "responses"
    }

    fn project_tools(&self, _client: &ModelClient, tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        crate::chat::response_tools_to_wire(tools)
    }

    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String> {
        let mut request = client.prepare_response_request(request);
        request.reasoning_effort =
            client.effective_reasoning_effort(&request.model, request.reasoning_effort)?;
        client.encode_response_body(&request)
    }
}
