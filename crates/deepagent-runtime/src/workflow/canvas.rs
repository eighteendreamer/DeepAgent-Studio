//! Canvas model bridge: the seam between workflow nodes and canvas providers.
//!
//! The workflow agent must be able to call the model a canvas node selected
//! (an OpenAI-compatible, Anthropic or Gemini endpoint configured in the
//! desktop database) without `deepagent-runtime` learning anything about
//! providers, api keys or the desktop app. So the trait is declared here and
//! implemented by `deepagent-app-core`'s canvas gateway, exactly like
//! [`crate::workflow::knowledge::KnowledgeRetriever`] and
//! [`crate::workflow::tools::ToolExecutor`].
//!
//! A model reference is the opaque `providerId::modelId` string the canvas UI
//! stores on a node. The bridge resolves it; callers never see a URL or a key.

use async_trait::async_trait;
use deepagent_core::error::Result;
use serde::{Deserialize, Serialize};

/// A text (or vision) completion request for one canvas node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasCompletionRequest {
    /// `providerId::modelId`; empty means "use the scenario default models".
    pub model_ref: String,
    pub system_prompt: Option<String>,
    pub prompt: String,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Data URLs of resolved image inputs, for vision prompts.
    pub images: Vec<String>,
}

/// Completion result, including which model actually answered so the node
/// event can record provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasCompletionResponse {
    pub text: String,
    pub provider_id: String,
    pub model_id: String,
}

/// An image generation or edit request for one canvas node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasImageRequest {
    pub model_ref: String,
    pub prompt: String,
    /// Reference images as data URLs. Non-empty selects the edit endpoint.
    pub reference_images: Vec<String>,
    pub size: Option<String>,
}

/// Image result. `data_url` keeps the existing canvas node contract
/// (`data.imageUrl`) intact while artifacts move to the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasImageResponse {
    pub data_url: String,
    pub mime: String,
    pub provider_id: String,
    pub model_id: String,
    /// `generate` or `edit`, decided by the router and reported back for events.
    pub operation: String,
}

/// An embedding request for one canvas node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEmbeddingRequest {
    pub model_ref: String,
    pub texts: Vec<String>,
}

/// Embedding result. Only the dimensions and reference are reportable; the
/// vector itself stays out of events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEmbeddingResponse {
    pub dimensions: usize,
    pub count: usize,
    pub provider_id: String,
    pub model_id: String,
    pub vectors: Vec<Vec<f32>>,
}

/// Executes canvas-node model calls through the configured providers.
#[async_trait]
pub trait CanvasModelBridge: Send + Sync {
    /// Text / vision completion.
    async fn complete(&self, request: CanvasCompletionRequest) -> Result<CanvasCompletionResponse>;

    /// Image generation or editing, decided by whether references are present.
    async fn generate_image(&self, request: CanvasImageRequest) -> Result<CanvasImageResponse>;

    /// Text embeddings.
    async fn embed(&self, request: CanvasEmbeddingRequest) -> Result<CanvasEmbeddingResponse>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeBridge {
        calls: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl CanvasModelBridge for FakeBridge {
        async fn complete(
            &self,
            request: CanvasCompletionRequest,
        ) -> Result<CanvasCompletionResponse> {
            self.calls.lock().unwrap().push(request.model_ref);
            Ok(CanvasCompletionResponse {
                text: "fake".to_string(),
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
            })
        }

        async fn generate_image(&self, request: CanvasImageRequest) -> Result<CanvasImageResponse> {
            Ok(CanvasImageResponse {
                data_url: format!("data:image/png;base64,{}", request.prompt.len()),
                mime: "image/png".to_string(),
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
                operation: if request.reference_images.is_empty() {
                    "generate".to_string()
                } else {
                    "edit".to_string()
                },
            })
        }

        async fn embed(&self, request: CanvasEmbeddingRequest) -> Result<CanvasEmbeddingResponse> {
            Ok(CanvasEmbeddingResponse {
                dimensions: 2,
                count: request.texts.len(),
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
                vectors: request.texts.iter().map(|_| vec![0.5, 0.25]).collect(),
            })
        }
    }

    #[tokio::test]
    async fn bridge_trait_is_object_safe_and_awaitable() {
        let bridge: std::sync::Arc<dyn CanvasModelBridge> = std::sync::Arc::new(FakeBridge {
            calls: Mutex::new(Vec::new()),
        });
        let answer = bridge
            .complete(CanvasCompletionRequest {
                model_ref: "cvp-1::gpt-5.6-sol".to_string(),
                prompt: "hi".to_string(),
                ..Default::default()
            })
            .await
            .expect("completion");
        assert_eq!(answer.text, "fake");
        let image = bridge
            .generate_image(CanvasImageRequest {
                prompt: "draw".to_string(),
                reference_images: vec!["data:image/png;base64,AA".to_string()],
                ..Default::default()
            })
            .await
            .expect("image");
        assert_eq!(image.operation, "edit");
        let embedded = bridge
            .embed(CanvasEmbeddingRequest {
                texts: vec!["a".to_string(), "b".to_string()],
                ..Default::default()
            })
            .await
            .expect("embedding");
        assert_eq!(embedded.count, 2);
    }
}
