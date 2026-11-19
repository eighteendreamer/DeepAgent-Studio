use super::*;
use crate::client::files::ImageRequest;
use crate::transport::TransportRequest;
use crate::ResponseInputItem;

pub(crate) struct ChatCompletionsCodec;

#[async_trait::async_trait]
impl WireCodec for ChatCompletionsCodec {
    async fn stream(
        &self,
        client: &ModelClient,
        request: ResponseRequest,
        observer: &mut dyn DeltaObserver,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Response> {
        client.validate_response_input_modalities(&request)?;
        self.stream_chat(
            client,
            response_to_chat_completion(request)?,
            observer,
            cancel,
        )
        .await
    }

    fn decoder(&self) -> Box<dyn StreamDecoder> {
        Box::new(crate::ChatCompletionAccumulator::new())
    }

    fn validate_image_role(&self, role: &str) -> Result<()> {
        if role == "user" {
            return Ok(());
        }
        Err(deepagent_core::error::CoreError::invalid(
            "image_input_role_unsupported: Chat Completions images require a user message",
        ))
    }

    fn validate_tool_images(&self, request: &ResponseRequest) -> Result<()> {
        if request.input.iter().any(|item| {
            matches!(item,
                ResponseInputItem::FunctionCallOutput { output, .. }
                | ResponseInputItem::CustomToolCallOutput { output, .. }
                if output.attachments().next().is_some())
        }) {
            return Err(deepagent_core::error::CoreError::invalid(
                "image_input_tool_output_unsupported: Chat Completions tool output does not support the Responses image contract; select Responses",
            ));
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "chat_completions"
    }

    fn project_tools(&self, _client: &ModelClient, tools: &[ToolSchema]) -> Vec<serde_json::Value> {
        crate::chat_completions::chat_tools_to_wire(tools)
    }

    fn encode_body(&self, client: &ModelClient, request: ResponseRequest) -> Result<String> {
        let mut chat = response_to_chat_completion(request)?;
        chat.stream = true;
        client.apply_chat_defaults(&mut chat)?;
        client.encode_chat_body(&chat)
    }
}

impl ChatCompletionsCodec {
    pub(crate) async fn stream_chat(
        &self,
        client: &ModelClient,
        mut request: crate::ChatCompletionRequest,
        observer: &mut dyn DeltaObserver,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Response> {
        request.stream = true;
        client.apply_chat_defaults(&mut request)?;
        request.validate()?;
        client.validate_output_tokens(request.max_tokens)?;
        client.validate_chat_input_modalities(&request)?;
        crate::image_input::validate_chat_payloads(request.clone(), cancel.clone()).await?;
        client
            .prepare_chat_files_for_attempt(&mut request, cancel.clone())
            .await?;
        let body = client.encode_chat_body(&request)?;
        let mut transport = TransportRequest {
            url: client.config().chat_endpoint(),
            api_key: client.config().api_key.clone(),
            body,
        };
        if !request.image_file_ids.is_empty() {
            return client
                .stream_files_aware_attempt(
                    self,
                    ImageRequest::Chat(&mut request),
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
}
