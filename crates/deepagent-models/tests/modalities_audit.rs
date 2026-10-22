//! Model discovery metadata and the provider request boundary, without network.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use deepagent_core::error::Result;
use deepagent_core::message::MessageAttachment;
use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
use deepagent_models::{
    ModelCatalog, ModelClient, ModelConfig, ModelDiscovery, ModelRole, ResponseInputItem,
    ResponseRequest,
};

// Public /models shape and declared modalities from DeepSeek's documentation
// retrieved 2026-10-22. No account data or live credentials are used.
const CATALOG: &str = r#"{"object":"list","data":[
  {"id":"deepseek-flash","object":"model","owned_by":"deepseek",
   "context_window":1048576,"max_output_tokens":393216,
   "input_modalities":["text","image"],"output_modalities":["text"]},
  {"id":"deepseek-v4-pro","object":"model","owned_by":"deepseek",
   "context_window":1048576,"max_output_tokens":393216,
   "input_modalities":["text"],"output_modalities":["text"]}
]}"#;

#[derive(Default)]
struct ModalityTransport {
    requests: Mutex<Vec<(String, usize)>>,
}

#[async_trait]
impl HttpTransport for ModalityTransport {
    async fn get_json(&self, url: &str, _key: &str) -> Result<String> {
        assert_eq!(url, "https://api.deepseek.com/models");
        Ok(CATALOG.into())
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
        let images = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item["content"].as_array())
            .flatten()
            .filter(|part| part["type"] == "input_image")
            .count();
        self.requests
            .lock()
            .unwrap()
            .push((body["model"].as_str().unwrap().into(), images));
        for event in [
            r#"{"type":"response.output_text.delta","delta":"Offline modality audit."}"#,
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
        ] {
            if sink.on_event(event)? {
                break;
            }
        }
        Ok(())
    }
}

struct Observation {
    accepted: bool,
    requests: Vec<(String, usize)>,
    modalities_retained: bool,
}

async fn sample(model: &str, with_image: bool) -> Observation {
    let transport = Arc::new(ModalityTransport::default());
    let discovered = ModelDiscovery::deepseek(transport.clone())
        .discover("offline-audit-placeholder")
        .await
        .unwrap();
    let encoded = serde_json::to_value(&discovered).unwrap();
    let modalities_retained = encoded["available"]
        .as_array()
        .unwrap()
        .iter()
        .all(|model| model.get("input_modalities").is_some());
    let restored: ModelCatalog = serde_json::from_value(encoded).unwrap();
    let config = ModelConfig::from_catalog("offline-audit-placeholder", &restored, ModelRole::Chat);
    let client = ModelClient::new(transport.clone(), config);
    // Numeric capacity metadata is already preserved; test the independent
    // media contract without incorrectly claiming all metadata is discarded.
    assert_eq!(client.capability(model).context_window, 1_048_576);
    let item = if with_image {
        ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "Inspect the image.".into(),
            attachments: vec![MessageAttachment {
                id: "audit-pixel".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
            }],
        }
    } else {
        ResponseInputItem::Message {
            role: "user".into(),
            content: "Text control.".into(),
        }
    };
    // Supply the valid resolved bytes at the provider seam. This deliberately
    // excludes H-67's current-user attachment assembly and filesystem handling.
    let urls = if with_image {
        BTreeMap::from([("audit-pixel".into(), concat!(
        "data:image/png;base64,",
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII="
    ).into())])
    } else {
        BTreeMap::new()
    };
    let request = ResponseRequest::from_response_items(model, None, vec![item])
        .with_attachment_data_urls(urls);
    let accepted = client
        .stream_response_cancelled(request, Arc::new(AtomicBool::new(false)))
        .await
        .is_ok();
    let requests = transport.requests.lock().unwrap().clone();
    eprintln!("model-modality model={model} image={with_image} numeric_capacity_retained=true modalities_retained={modalities_retained} transport_requests={requests:?} accepted={accepted}");
    Observation {
        accepted,
        requests,
        modalities_retained,
    }
}

#[tokio::test]
async fn supported_image_and_text_only_requests_reach_transport() {
    let image = sample("deepseek-flash", true).await;
    assert!(image.accepted);
    assert_eq!(image.requests, vec![("deepseek-flash".into(), 1)]);
    let text = sample("deepseek-v4-pro", false).await;
    assert!(text.accepted);
    assert_eq!(text.requests, vec![("deepseek-v4-pro".into(), 0)]);
}

#[tokio::test]
#[ignore = "audit probe: declared text-only models must reject image requests before transport"]
async fn audit_declared_input_modalities_constrain_requests() {
    let text_only = sample("deepseek-v4-pro", true).await;
    assert!(
        text_only.modalities_retained && !text_only.accepted && text_only.requests.is_empty(),
        "declared input modalities were lost and an unsupported image request reached transport"
    );
}
