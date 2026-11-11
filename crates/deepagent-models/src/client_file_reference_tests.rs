use super::*;
use crate::file_references::{
    DeepSeekImageFileCache, ImageFileReference, ImageFileReferenceStore, FILES_TTL_SECONDS,
};
use crate::responses::ResponseInputItem;
use crate::stream::{DeltaObserver, ModelStreamEvent};
use crate::transport::{EventSink, FileUploadRequest, UploadedFile};
use async_trait::async_trait;
use deepagent_core::error::{CoreError, Result};
use deepagent_core::message::{Message, MessageAttachment};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=";

#[derive(Default)]
struct MemoryStore(Mutex<HashMap<String, ImageFileReference>>);

impl ImageFileReferenceStore for MemoryStore {
    fn lock(&self, _key: &str) -> Result<Box<dyn Send>> {
        Ok(Box::new(()))
    }

    fn get(&self, key: &str) -> Result<Option<ImageFileReference>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn put(&self, key: &str, value: &ImageFileReference) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), value.clone());
        Ok(())
    }

    fn remove_if_matches(&self, key: &str, file_id: &str) -> Result<()> {
        let mut entries = self.0.lock().unwrap();
        if entries
            .get(key)
            .is_some_and(|entry| entry.file_id == file_id)
        {
            entries.remove(key);
        }
        Ok(())
    }
}

struct FileReferenceTransport {
    uploads: AtomicBool,
    upload_count: std::sync::atomic::AtomicUsize,
    fail_after_output: bool,
    requests: Mutex<Vec<serde_json::Value>>,
    reject_count: usize,
    reject_message: &'static str,
    upload_fail_at: Option<usize>,
    cancel_after_reject: Option<Arc<AtomicBool>>,
}

impl FileReferenceTransport {
    fn new(fail_after_output: bool) -> Self {
        Self {
            uploads: AtomicBool::new(true),
            upload_count: std::sync::atomic::AtomicUsize::new(0),
            fail_after_output,
            requests: Mutex::new(Vec::new()),
            reject_count: 1,
            reject_message: "file_id expired",
            upload_fail_at: None,
            cancel_after_reject: None,
        }
    }
}

#[async_trait]
impl HttpTransport for FileReferenceTransport {
    fn supports_file_upload(&self) -> bool {
        self.uploads.load(Ordering::Acquire)
    }

    async fn upload_file(&self, _request: FileUploadRequest) -> Result<UploadedFile> {
        let id = self.upload_count.fetch_add(1, Ordering::SeqCst) + 1;
        if self.upload_fail_at == Some(id) {
            return Err(CoreError::provider_with_metadata(
                Some(429),
                None,
                "synthetic upload quota",
                None,
                None,
            ));
        }
        Ok(UploadedFile {
            id: format!("file-api-test-{id}"),
            expires_at: Some(DeepSeekImageFileCache::now_unix_seconds() + FILES_TTL_SECONDS as i64),
        })
    }

    async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
        let index = self.requests.lock().unwrap().len();
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::from_str(&request.body).unwrap());
        if index < self.reject_count {
            if self.fail_after_output {
                sink.on_event(
                    &crate::test_sequence::FixtureSequence::default()
                        .event(r#"{"type":"response.output_text.delta","delta":"partial"}"#),
                )?;
            }
            if let Some(cancel) = &self.cancel_after_reject {
                cancel.store(true, Ordering::Release);
            }
            return Err(CoreError::provider_with_metadata(
                Some(400),
                Some("invalid_request_error".into()),
                self.reject_message,
                None,
                None,
            ));
        }

        if request.url.ends_with("/chat/completions") {
            sink.on_event(r#"{"choices":[{"delta":{"content":"ok"},"finish_reason":"stop"}]}"#)?;
            sink.on_event("[DONE]")?;
            return Ok(());
        }
        let event = r#"{"type":"response.completed","response":{"id":"file-reference-retry","object":"response","created_at":1,"model":"deepseek-chat","output":[],"status":"completed"}}"#;
        sink.on_event(&crate::test_sequence::FixtureSequence::default().event(event))?;
        Ok(())
    }
}

#[derive(Default)]
struct Observer(Vec<ModelStreamEvent>);

impl DeltaObserver for Observer {
    fn on_event(&mut self, event: ModelStreamEvent) {
        self.0.push(event);
    }
}

fn image_request() -> ResponseRequest {
    ResponseRequest::from_response_items(
        "deepseek-flash",
        None,
        vec![ResponseInputItem::InputMessage {
            role: "user".into(),
            content: "Inspect the image".into(),
            attachments: vec![MessageAttachment {
                id: "synthetic-image".into(),
                kind: "image".into(),
                media_type: Some("image/png".into()),
                path: None,
                sha256: None,
            }],
        }],
    )
    .with_attachment_data_urls(BTreeMap::from([(
        "synthetic-image".into(),
        PNG_DATA_URL.into(),
    )]))
}

fn chat_image_request() -> ChatCompletionRequest {
    let attachment = MessageAttachment {
        id: "synthetic-image".into(),
        kind: "image".into(),
        media_type: Some("image/png".into()),
        path: None,
        sha256: None,
    };
    let mut request = ChatCompletionRequest::new(
        "deepseek-flash",
        vec![Message::user("Inspect the image").with_attachments(vec![attachment])],
    );
    request
        .attachment_data_urls
        .insert("synthetic-image".into(), PNG_DATA_URL.into());
    request
}

fn client(transport: Arc<FileReferenceTransport>) -> ModelClient {
    ModelClient::new(transport, ModelConfig::deepseek("synthetic-key")).with_image_file_cache(
        DeepSeekImageFileCache::new(Arc::new(MemoryStore::default())),
    )
}

#[tokio::test]
async fn expired_file_reference_retries_once_inline_and_then_reuploads() {
    let transport = Arc::new(FileReferenceTransport::new(false));
    let client = client(transport.clone());
    let mut observer = Observer::default();

    client
        .stream_response_observed(image_request(), &mut observer)
        .await
        .unwrap();
    client
        .stream_response_observed(image_request(), &mut observer)
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "one failed file request, one inline fallback, one later re-upload"
    );
    assert_eq!(
        requests[0]["input"][0]["content"][1]["file_id"],
        "file-api-test-1"
    );
    assert!(requests[0]["input"][0]["content"][1]
        .get("image_url")
        .is_none());
    assert_eq!(
        requests[1]["input"][0]["content"][1]["image_url"],
        PNG_DATA_URL
    );
    assert!(requests[1]["input"][0]["content"][1]
        .get("file_id")
        .is_none());
    assert_eq!(
        requests[2]["input"][0]["content"][1]["file_id"],
        "file-api-test-2"
    );
    assert_eq!(transport.upload_count.load(Ordering::SeqCst), 2);
    let response_events: Vec<_> = observer
        .0
        .iter()
        .filter_map(|event| match event {
            ModelStreamEvent::ResponseStreamEvent { response_id, .. } => response_id.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(
        response_events,
        vec!["file-reference-retry", "file-reference-retry"]
    );
}

#[tokio::test]
async fn chat_completions_uses_file_blocks_and_retries_once_inline_when_reference_expires() {
    let transport = Arc::new(FileReferenceTransport::new(false));
    let client = client(transport.clone());

    client
        .stream_chat_completion(chat_image_request())
        .await
        .unwrap();
    client
        .stream_chat_completion(chat_image_request())
        .await
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["messages"][0]["content"][1]["type"], "file");
    assert_eq!(
        requests[0]["messages"][0]["content"][1]["file_id"],
        "file-api-test-1"
    );
    assert_eq!(
        requests[1]["messages"][0]["content"][1]["type"],
        "image_url"
    );
    assert_eq!(
        requests[1]["messages"][0]["content"][1]["image_url"]["url"],
        PNG_DATA_URL
    );
    assert_eq!(requests[2]["messages"][0]["content"][1]["type"], "file");
    assert_eq!(
        requests[2]["messages"][0]["content"][1]["file_id"],
        "file-api-test-2"
    );
    assert_eq!(transport.upload_count.load(Ordering::SeqCst), 2);
}

#[test]
fn files_endpoint_matches_deepseek_route_when_model_base_has_legacy_v1_suffix() {
    let transport = Arc::new(crate::transport::MockTransport::default());
    let mut config = ModelConfig::deepseek("synthetic-key");
    config.base_url = "https://api.deepseek.com/v1/".into();
    let client = ModelClient::new(transport, config);

    assert_eq!(
        client.files_endpoint().unwrap(),
        "https://api.deepseek.com/files"
    );
}

#[test]
fn files_endpoint_keeps_non_versioned_base_paths_and_rejects_url_credentials() {
    let transport = Arc::new(crate::transport::MockTransport::default());
    let mut config = ModelConfig::deepseek("synthetic-key");
    config.base_url = "https://gateway.example/deepseek".into();
    let client = ModelClient::new(transport, config.clone());
    assert_eq!(
        client.files_endpoint().unwrap(),
        "https://gateway.example/deepseek/files"
    );

    config.base_url = "https://user:secret@gateway.example/v1".into();
    let client = ModelClient::new(Arc::new(crate::transport::MockTransport::default()), config);
    assert!(client.files_endpoint().is_err());
}

#[tokio::test]
async fn rejected_file_reference_is_not_retried_after_partial_output_was_observed() {
    let transport = Arc::new(FileReferenceTransport::new(true));
    let client = client(transport.clone());
    let mut observer = Observer::default();

    let error = client
        .stream_response_observed(image_request(), &mut observer)
        .await
        .unwrap_err();

    assert!(
        matches!(
            error,
            CoreError::Provider {
                status: Some(400),
                ..
            }
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 1);
    assert_eq!(transport.upload_count.load(Ordering::SeqCst), 1);
    assert!(observer.0.iter().any(
        |event| matches!(event, ModelStreamEvent::ContentDelta { text } if text == "partial")
    ));

    client
        .stream_response_observed(image_request(), &mut observer)
        .await
        .unwrap();
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]["input"][0]["content"][1]["file_id"],
        "file-api-test-2"
    );
    assert_eq!(transport.upload_count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn upload_failure_uses_all_inline_and_keeps_successful_reference_for_later() {
    for mode in [WireMode::Responses, WireMode::ChatCompletions] {
        let mut transport = FileReferenceTransport::new(false);
        transport.reject_count = 0;
        transport.upload_fail_at = Some(2);
        let transport = Arc::new(transport);
        let client = ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("synthetic-key").with_wire_mode(mode),
        )
        .with_image_file_cache(DeepSeekImageFileCache::new(Arc::new(
            MemoryStore::default(),
        )));
        let mut request = image_request();
        // A second real raster with different encoded identity, after the
        // same H-169 decoder gate, makes the second upload fail.
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 1)
            .write_to(&mut encoded, image::ImageFormat::Png)
            .unwrap();
        use base64::Engine;
        let second_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(encoded.into_inner())
        );
        let ResponseInputItem::InputMessage { attachments, .. } = &mut request.input[0] else {
            unreachable!()
        };
        let mut second = attachments[0].clone();
        second.id = "image-two".into();
        attachments.push(second);
        // Refresh the compatibility projection through its public mutator.
        let updated = ResponseRequest::from_response_items(
            request.model.clone(),
            None,
            request.input.clone(),
        );
        request.replace_history_from(&updated);
        request
            .attachment_data_urls
            .insert("synthetic-image".into(), PNG_DATA_URL.into());
        request
            .attachment_data_urls
            .insert("image-two".into(), second_url);
        client.stream_response(request.clone()).await.unwrap();
        client.stream_response(request).await.unwrap();
        let requests = transport.requests.lock().unwrap();
        let key = if mode == WireMode::Responses {
            "input"
        } else {
            "messages"
        };
        let first = requests[0][key][0]["content"].as_array().unwrap();
        assert_eq!(first.len(), 3);
        assert!(first[1].get("file_id").is_none());
        assert!(first[2].get("file_id").is_none());
        let second = requests[1][key][0]["content"].as_array().unwrap();
        assert!(second[1].get("file_id").is_some());
        assert!(second[2].get("file_id").is_some());
        assert_eq!(transport.upload_count.load(Ordering::SeqCst), 3);
    }
}

#[tokio::test]
async fn general_400_and_repeated_file_error_never_create_an_unbounded_retry() {
    for (message, rejected, expected) in
        [("invalid query", 1, 1), ("file_id expired", usize::MAX, 2)]
    {
        let mut transport = FileReferenceTransport::new(false);
        transport.reject_message = message;
        transport.reject_count = rejected;
        let transport = Arc::new(transport);
        let client = client(transport.clone());
        client.stream_response(image_request()).await.unwrap_err();
        assert_eq!(transport.requests.lock().unwrap().len(), expected);
        assert_eq!(transport.upload_count.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn cancellation_after_file_rejection_prevents_inline_fallback() {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut transport = FileReferenceTransport::new(false);
    transport.cancel_after_reject = Some(cancel.clone());
    let transport = Arc::new(transport);
    let client = client(transport.clone());
    client
        .stream_response_cancelled(image_request(), cancel)
        .await
        .unwrap_err();
    assert_eq!(transport.requests.lock().unwrap().len(), 1);
}

struct PendingUpload {
    entered: tokio::sync::Notify,
    dropped: Arc<AtomicBool>,
    requests: std::sync::atomic::AtomicUsize,
}

struct UploadLifetime(Arc<AtomicBool>);
impl Drop for UploadLifetime {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[async_trait]
impl HttpTransport for PendingUpload {
    fn supports_file_upload(&self) -> bool {
        true
    }
    async fn upload_file(&self, _request: FileUploadRequest) -> Result<UploadedFile> {
        let _lifetime = UploadLifetime(self.dropped.clone());
        self.entered.notify_one();
        std::future::pending().await
    }
    async fn stream(&self, _request: TransportRequest, _sink: &mut dyn EventSink) -> Result<()> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        panic!("pending or cancelled upload must not start a model request")
    }
}

#[tokio::test]
async fn upload_cancel_and_dropped_model_future_release_the_inflight_upload() {
    for drop_future in [false, true] {
        let transport = Arc::new(PendingUpload {
            entered: tokio::sync::Notify::new(),
            dropped: Arc::new(AtomicBool::new(false)),
            requests: std::sync::atomic::AtomicUsize::new(0),
        });
        let cache = Arc::new(MemoryStore::default());
        let client = ModelClient::new(transport.clone(), ModelConfig::deepseek("synthetic"))
            .with_image_file_cache(DeepSeekImageFileCache::new(cache.clone()));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut request =
            Box::pin(client.stream_response_cancelled(image_request(), cancel.clone()));
        tokio::select! {
            _ = transport.entered.notified() => {},
            result = &mut request => panic!("upload completed before cancellation: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => panic!("upload did not start"),
        }
        if drop_future {
            drop(request);
        } else {
            cancel.store(true, Ordering::Release);
            let error = tokio::time::timeout(std::time::Duration::from_secs(1), request)
                .await
                .expect("upload cancellation must settle")
                .unwrap_err();
            assert!(error.to_string().contains("cancelled"));
        }
        assert!(transport.dropped.load(Ordering::Acquire));
        assert_eq!(transport.requests.load(Ordering::SeqCst), 0);
        assert!(cache.0.lock().unwrap().is_empty());
    }
}
