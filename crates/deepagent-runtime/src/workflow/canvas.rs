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
    /// Canvas node kind, so the app layer can assemble the node's bound prompt
    /// profile. Empty for professional nodes that carry their own system prompt.
    #[serde(default)]
    pub node_kind: String,
    /// Skills requested for this node; validated against the node contract.
    #[serde(default)]
    pub skill_ids: Vec<String>,
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
    /// Provider-side reasoning, preserved as its own field when returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    pub provider_id: String,
    pub model_id: String,
}

/// An image generation or edit request for one canvas node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasImageRequest {
    pub model_ref: String,
    pub prompt: String,
    /// Reference images: `artifact://<id>`, a `data:` URL or an http URL.
    pub reference_images: Vec<String>,
    pub size: Option<String>,
    /// Operation chosen by the router (`image_generate` / `image_edit`). Empty
    /// means the caller had no router and presence of references decides.
    #[serde(default)]
    pub operation: String,
}

/// Image result. `artifact_uri` is the only form a canvas node may persist:
/// the bytes stay in the artifact store, so no base64 reaches the graph, the
/// node events or the session history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasImageResponse {
    pub artifact_uri: String,
    pub mime: String,
    pub provider_id: String,
    pub model_id: String,
    /// `generate` or `edit`, decided by the router and reported back for events.
    pub operation: String,
}

/// A speech request for one canvas audio node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasAudioRequest {
    pub model_ref: String,
    /// `speech_synthesize` or `speech_transcribe`, as chosen by the router.
    #[serde(default)]
    pub operation: String,
    /// Text to speak.
    pub text: String,
    /// Audio to transcribe: `artifact://<id>`, a `data:` URL or an http URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_reference: Option<String>,
    /// Provider voice id. `None` leaves the choice to the provider, which
    /// rejects the call with its own error; the kernel never invents one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Requested container, e.g. `mp3`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub timeout_ms: u64,
}

/// Speech result. Synthesis reports an artifact reference, transcription the
/// recognized text; raw audio bytes never leave the gateway.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasAudioResponse {
    pub operation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub provider_id: String,
    pub model_id: String,
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

/// What the kernel knows about one node when it must decide an operation.
///
/// The kernel never guesses: it hands the node kind, the node's explicit
/// operation and the *resolved* artifact kinds to the app layer, which owns the
/// node contract registry and the deterministic router.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasRouteRequest {
    pub node_kind: String,
    /// `auto` or empty means "decide from the input facts".
    pub explicit_operation: Option<String>,
    /// Resolved input kinds, e.g. `Text`, `Image`, `Video`, `Audio`, `Empty`.
    pub input_kinds: Vec<String>,
    pub has_prompt: bool,
}

/// The routed outcome: exactly one operation for this run of the node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasRouteOutcome {
    pub operation: String,
    /// Why it was chosen: `explicit_operation`, `artifact_facts` or `contract_default`.
    pub reason: String,
}

/// Executes canvas-node model calls through the configured providers.
#[async_trait]
pub trait CanvasModelBridge: Send + Sync {
    /// Text / vision completion.
    async fn complete(&self, request: CanvasCompletionRequest) -> Result<CanvasCompletionResponse>;

    /// Image generation or editing, decided by whether references are present.
    async fn generate_image(&self, request: CanvasImageRequest) -> Result<CanvasImageResponse>;

    /// Text-to-speech or speech-to-text for the audio node.
    async fn run_audio(&self, request: CanvasAudioRequest) -> Result<CanvasAudioResponse>;

    /// Text embeddings.
    async fn embed(&self, request: CanvasEmbeddingRequest) -> Result<CanvasEmbeddingResponse>;

    /// Decide the concrete operation for a node from its contract and the
    /// resolved input facts. Ambiguity is an error, never a silent default.
    fn route_operation(&self, request: CanvasRouteRequest) -> Result<CanvasRouteOutcome>;

    /// Report the real media kind behind each reference (`Text`, `Image`,
    /// `Video`, `Audio`, `Json` or `Unknown`).
    ///
    /// Routing must use what the stored bytes actually are rather than which
    /// config array a value arrived in.
    fn inspect_input_kinds(&self, references: &[String]) -> Result<Vec<String>>;
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
            let _ = (&request.node_kind, &request.skill_ids);
            Ok(CanvasCompletionResponse {
                text: "fake".to_string(),
                reasoning: None,
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
            })
        }

        async fn generate_image(&self, request: CanvasImageRequest) -> Result<CanvasImageResponse> {
            Ok(CanvasImageResponse {
                artifact_uri: format!("artifact://art_fake_{}", request.prompt.len()),
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

        async fn run_audio(&self, request: CanvasAudioRequest) -> Result<CanvasAudioResponse> {
            self.calls.lock().unwrap().push(request.model_ref);
            Ok(CanvasAudioResponse {
                operation: request.operation,
                audio_url: if request.audio_reference.is_some() {
                    None
                } else {
                    Some("artifact://art_voice_fake".to_string())
                },
                text: if request.audio_reference.is_some() {
                    Some("fake transcription".to_string())
                } else {
                    None
                },
                provider_id: "p".to_string(),
                model_id: "m".to_string(),
            })
        }

        fn route_operation(&self, request: CanvasRouteRequest) -> Result<CanvasRouteOutcome> {
            self.calls.lock().unwrap().push(request.node_kind.clone());
            Ok(CanvasRouteOutcome {
                operation: if request.input_kinds.iter().any(|kind| kind == "Image") {
                    "image_edit".to_string()
                } else {
                    "image_generate".to_string()
                },
                reason: "artifact_facts".to_string(),
            })
        }

        fn inspect_input_kinds(&self, references: &[String]) -> Result<Vec<String>> {
            Ok(references
                .iter()
                .map(|reference| {
                    if reference.contains("image") || reference.starts_with("artifact://") {
                        "Image".to_string()
                    } else {
                        "Text".to_string()
                    }
                })
                .collect())
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

    #[test]
    fn routing_is_part_of_the_bridge_contract() {
        let request = CanvasRouteRequest {
            node_kind: "image-gen".to_string(),
            input_kinds: vec!["Text".to_string(), "Image".to_string()],
            has_prompt: true,
            ..Default::default()
        };
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(json["nodeKind"], "image-gen");
        assert_eq!(json["inputKinds"].as_array().expect("kinds").len(), 2);
        assert!(request.explicit_operation.is_none());
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
