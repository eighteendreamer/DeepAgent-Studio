//! A legacy resolver may read bytes even from is_resolvable's default method.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct ReadingPrice(AtomicUsize);
impl AttachmentDataResolver for ReadingPrice {
    fn data_url(&self, _: &deepagent_core::message::MessageAttachment) -> Option<String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Some(concat!("data:image/png;base64,",
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=").into())
    }
}

#[test]
fn h71_context_pricing_cannot_materialize_unsupported_images() {
    for declaration in [
        serde_json::json!(["text"]),
        serde_json::json!(["text", "image"]),
    ] {
        let model: deepagent_models::ModelInfo = serde_json::from_value(serde_json::json!({
            "id":"h71-pricing", "input_modalities":declaration,
        }))
        .unwrap();
        let mut config = deepagent_models::ModelConfig::deepseek("synthetic");
        config
            .capabilities
            .push(deepagent_models::ModelCapabilityResolver::new().resolve_model(&model));
        let client = Arc::new(deepagent_models::ModelClient::new(
            Arc::new(deepagent_models::transport::MockTransport::default()),
            config,
        ));
        let reference = deepagent_core::message::MessageAttachment {
            id: "h71-pricing-image".into(),
            kind: "image".into(),
            path: None,
            media_type: None,
            sha256: None,
        };
        let reads = Arc::new(ReadingPrice(AtomicUsize::new(0)));
        let agent = ModelAgent::new_with_attachments(
            client,
            "h71-pricing",
            "rules",
            "synthetic input",
            vec![reference],
            vec![],
        )
        .with_attachment_resolver(reads.clone());
        agent.context_measurement();
        assert_eq!(
            reads.0.load(Ordering::SeqCst),
            usize::from(declaration.as_array().unwrap().iter().any(|v| v == "image"))
        );
    }
}
