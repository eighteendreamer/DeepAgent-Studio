use super::*;
use crate::transport::TransportRequest;

pub(super) struct MessagesCodec;

#[async_trait::async_trait]
impl WireCodec for MessagesCodec {
    fn auth(&self) -> crate::transport::TransportAuth {
        crate::transport::TransportAuth::Messages
    }

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
        crate::image_input::validate_response_payloads(request.clone(), cancel.clone()).await?;
        let body = self.encode_body(client, request)?;
        client
            .stream_attempt(
                self,
                TransportRequest {
                    auth: self.auth(),
                    url: client.config().messages_endpoint(),
                    api_key: client.config().api_key.clone(),
                    body,
                },
                observer,
                cancel,
            )
            .await
    }

    fn decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(crate::messages::MessagesAccumulator::new())
    }

    fn validate_image_role(&self, role: &str) -> Result<()> {
        if role == "user" {
            return Ok(());
        }
        Err(deepagent_core::error::CoreError::invalid(
            "image_input_role_unsupported: Messages images require a user message or a tool_result",
        ))
    }

    fn name(&self) -> &'static str {
        "messages"
    }

    fn project_tools(&self, client: &ModelClient, tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        crate::messages::project_tools(tools, client.messages_cache_policy())
    }

    fn tool_kind<'a>(&self, schema: &'a serde_json::Value) -> Result<&'a str> {
        if let Some(kind) = schema.get("type").and_then(serde_json::Value::as_str) {
            return Ok(kind);
        }
        if schema
            .get("name")
            .and_then(serde_json::Value::as_str)
            .is_some()
            && schema
                .get("input_schema")
                .is_some_and(serde_json::Value::is_object)
        {
            return Ok("custom");
        }
        Err(deepagent_core::error::CoreError::invalid(
            "invalid Messages tool schema",
        ))
    }

    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String> {
        let request = client.prepare_response_request(request);
        let max_tokens = request.max_output_tokens.ok_or_else(|| {
            deepagent_core::error::CoreError::invalid(
                "Messages requests require a positive max_output_tokens value",
            )
        })?;
        crate::messages::encode_request_with_cache(
            &request,
            max_tokens,
            client.messages_cache_policy(),
        )
    }
}
