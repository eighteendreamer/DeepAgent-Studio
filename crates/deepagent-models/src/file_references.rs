//! DeepSeek Files API image references. Provider files are account-scoped,
//! while only metadata and a digest-scoped lookup key are persisted locally.

use crate::transport::{FileUploadRequest, HttpTransport};
use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub const FILES_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;
const REFRESH_SAFETY_SECONDS: i64 = 60 * 60;

pub(crate) fn valid_file_id(file_id: &str) -> bool {
    let Some(suffix) = file_id.strip_prefix("file-api-") else {
        return false;
    };
    !suffix.is_empty()
        && file_id.len() <= 256
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageFileReference {
    pub file_id: String,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedImageFile {
    pub key: String,
    pub reference: ImageFileReference,
    pub created_now: bool,
}

/// AppCore backs this seam with the existing SQLite document store. The lock
/// covers a single content identity, never a database lock, and may span an
/// upload to close the cross-process check/upload/publish race.
pub trait ImageFileReferenceStore: Send + Sync {
    fn lock(&self, key: &str) -> Result<Box<dyn Send>>;
    fn get(&self, key: &str) -> Result<Option<ImageFileReference>>;
    fn put(&self, key: &str, value: &ImageFileReference) -> Result<()>;
    fn remove_if_matches(&self, key: &str, file_id: &str) -> Result<()>;
}

#[derive(Clone)]
pub struct DeepSeekImageFileCache {
    store: Arc<dyn ImageFileReferenceStore>,
}

impl DeepSeekImageFileCache {
    pub fn new(store: Arc<dyn ImageFileReferenceStore>) -> Self {
        Self { store }
    }

    pub fn now_unix_seconds() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_secs()).ok())
            .unwrap_or_default()
    }

    /// Return an account/endpoint/model/format/content scoped persistent key.
    /// API keys are hashed into the key and are never stored by this module.
    pub fn key(endpoint: &str, api_key: &str, model: &str, mime: &str, bytes: &[u8]) -> String {
        let mut hash = Sha256::new();
        let endpoint = endpoint.trim_end_matches('/');
        for part in [
            endpoint.as_bytes(),
            api_key.as_bytes(),
            model.as_bytes(),
            mime.as_bytes(),
            bytes,
        ] {
            hash.update((part.len() as u64).to_be_bytes());
            hash.update(part);
        }
        format!("sha256-{:x}", hash.finalize())
    }

    fn usable(reference: &ImageFileReference, now: i64) -> bool {
        valid_file_id(&reference.file_id)
            && reference
                .expires_at
                .is_some_and(|expires_at| expires_at > now.saturating_add(REFRESH_SAFETY_SECONDS))
    }

    /// Lookup or upload once under the per-content cross-process gate.
    pub async fn resolve(
        &self,
        transport: &dyn HttpTransport,
        request: FileUploadRequest,
        model: &str,
        now: i64,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<ResolvedImageFile> {
        let endpoint = request.url.clone();
        let api_key = request.api_key.clone();
        let key = Self::key(&endpoint, &api_key, model, &request.mime, &request.bytes);
        let (guard, reference) = self.load_locked(&key, now, cancel.clone()).await?;
        if let Some(reference) = reference {
            return Ok(ResolvedImageFile {
                key,
                reference,
                created_now: false,
            });
        }
        if cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(CoreError::other("request cancelled"));
        }
        let cleanup_cancel = cancel.clone();
        let uploaded = match cancel.as_ref() {
            Some(cancel) => {
                transport
                    .upload_file_cancelled(request, cancel.clone())
                    .await?
            }
            None => transport.upload_file(request).await?,
        };
        let Some(expires_at) = uploaded.expires_at else {
            cleanup_uploaded_file(transport, &endpoint, &api_key, &uploaded.id, cleanup_cancel)
                .await;
            return Err(CoreError::other(
                "DeepSeek Files upload did not confirm the requested expiration",
            ));
        };
        let reference = ImageFileReference {
            file_id: uploaded.id,
            expires_at: Some(expires_at),
        };
        if !Self::usable(&reference, now) {
            cleanup_uploaded_file(
                transport,
                &endpoint,
                &api_key,
                &reference.file_id,
                cleanup_cancel,
            )
            .await;
            return Err(CoreError::other(
                "DeepSeek Files upload returned an invalid or near-expiry file reference",
            ));
        }
        // A started SQLite publication must settle under the same content
        // gate. Cancelling the await while a blocking commit is still running
        // could publish a reference after compensation deleted its file.
        if let Err(error) = self.put_locked(&key, &reference, guard).await {
            cleanup_uploaded_file(
                transport,
                &endpoint,
                &api_key,
                &reference.file_id,
                cleanup_cancel,
            )
            .await;
            return Err(error);
        }
        if cancel
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Acquire))
        {
            return Err(CoreError::other("request cancelled"));
        }
        Ok(ResolvedImageFile {
            key,
            reference,
            created_now: true,
        })
    }

    pub async fn invalidate(
        &self,
        endpoint: &str,
        api_key: &str,
        model: &str,
        mime: &str,
        bytes: &[u8],
        file_id: &str,
    ) -> Result<()> {
        let key = Self::key(endpoint, api_key, model, mime, bytes);
        self.remove_if_matches(&key, file_id, None).await
    }

    async fn load_locked(
        &self,
        key: &str,
        now: i64,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<(Box<dyn Send>, Option<ImageFileReference>)> {
        let store = self.store.clone();
        let key = key.to_owned();
        await_store_operation(
            tokio::task::spawn_blocking(move || {
                let guard = store.lock(&key)?;
                let reference = store.get(&key)?;
                if let Some(reference) = &reference {
                    if !Self::usable(reference, now) {
                        store.remove_if_matches(&key, &reference.file_id)?;
                        return Ok((guard, None));
                    }
                }
                Ok((guard, reference))
            }),
            cancel,
        )
        .await
    }

    async fn put_locked(
        &self,
        key: &str,
        value: &ImageFileReference,
        guard: Box<dyn Send>,
    ) -> Result<()> {
        let store = self.store.clone();
        let key = key.to_owned();
        let value = value.clone();
        await_store_operation(
            tokio::task::spawn_blocking(move || {
                let _guard = guard;
                store.put(&key, &value)
            }),
            None,
        )
        .await
    }

    async fn remove_if_matches(
        &self,
        key: &str,
        file_id: &str,
        cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<()> {
        let store = self.store.clone();
        let key = key.to_owned();
        let file_id = file_id.to_owned();
        await_store_operation(
            tokio::task::spawn_blocking(move || {
                let guard = store.lock(&key)?;
                let result = store.remove_if_matches(&key, &file_id);
                drop(guard);
                result
            }),
            cancel,
        )
        .await
    }
}

async fn await_store_operation<T: Send + 'static>(
    operation: tokio::task::JoinHandle<Result<T>>,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<T> {
    let operation = async move {
        operation.await.map_err(|error| {
            CoreError::other(format!("image reference store operation failed: {error}"))
        })?
    };
    match cancel {
        Some(cancel) => {
            if cancel.load(std::sync::atomic::Ordering::Acquire) {
                return Err(CoreError::other("request cancelled"));
            }
            let execution = deepagent_tools::ToolExecutionContext::new(cancel);
            tokio::select! {
                result = operation => result,
                _ = execution.cancelled() => Err(CoreError::other("request cancelled")),
            }
        }
        None => operation.await,
    }
}

async fn cleanup_uploaded_file(
    transport: &dyn HttpTransport,
    endpoint: &str,
    api_key: &str,
    file_id: &str,
    cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
) {
    match cancel {
        Some(cancel) => {
            let _ = transport
                .delete_file_cancelled(endpoint, api_key, file_id, cancel)
                .await;
        }
        None => {
            let _ = transport.delete_file(endpoint, api_key, file_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{EventSink, TransportRequest, UploadedFile};
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[test]
    fn file_reference_ids_cannot_escape_the_provider_files_route() {
        assert!(valid_file_id("file-api-1234-abcd"));
        for invalid in [
            "",
            "file-api-",
            "file-api-../responses",
            "file-api-id?next=/responses",
            "other-provider-file-id",
        ] {
            assert!(
                !valid_file_id(invalid),
                "accepted unsafe file id {invalid:?}"
            );
        }
    }

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

    type UploadObservation = (String, String, String, Vec<u8>);

    struct UploadRecorder {
        uploads: Mutex<Vec<UploadObservation>>,
        next_expiry: Mutex<Option<i64>>,
        deleted: Mutex<Vec<String>>,
    }

    impl UploadRecorder {
        fn with_expiry(expiry: i64) -> Self {
            Self {
                uploads: Mutex::new(Vec::new()),
                next_expiry: Mutex::new(Some(expiry)),
                deleted: Mutex::new(Vec::new()),
            }
        }

        fn set_expiry(&self, expiry: Option<i64>) {
            *self.next_expiry.lock().unwrap() = expiry;
        }
    }

    #[async_trait]
    impl HttpTransport for UploadRecorder {
        fn supports_file_upload(&self) -> bool {
            true
        }

        async fn stream(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
        ) -> Result<()> {
            unreachable!("file-reference tests do not send a model request")
        }

        async fn upload_file(&self, request: FileUploadRequest) -> Result<UploadedFile> {
            let index = self.uploads.lock().unwrap().len() + 1;
            self.uploads.lock().unwrap().push((
                request.url,
                request.filename,
                request.mime,
                request.bytes,
            ));
            Ok(UploadedFile {
                id: format!("file-api-test-{index}"),
                expires_at: *self.next_expiry.lock().unwrap(),
            })
        }

        async fn delete_file(&self, _url: &str, _api_key: &str, file_id: &str) -> Result<()> {
            self.deleted.lock().unwrap().push(file_id.to_owned());
            Ok(())
        }
    }

    #[tokio::test]
    async fn same_scoped_image_reuses_one_durable_reference() {
        let now = DeepSeekImageFileCache::now_unix_seconds();
        let transport = UploadRecorder::with_expiry(now + FILES_TTL_SECONDS as i64);
        let cache = DeepSeekImageFileCache::new(Arc::new(MemoryStore::default()));
        let bytes = b"synthetic image bytes";
        let first = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com/v1/").trim_end_matches('/')
                    ),
                    api_key: ("synthetic-api-key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (bytes).to_vec(),
                },
                "deepseek-chat",
                now,
                None,
            )
            .await
            .unwrap();
        let second = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com/v1").trim_end_matches('/')
                    ),
                    api_key: ("synthetic-api-key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (bytes).to_vec(),
                },
                "deepseek-chat",
                now + 60,
                None,
            )
            .await
            .unwrap();

        assert_eq!(transport.uploads.lock().unwrap().len(), 1);
        assert_eq!(first.reference.file_id, second.reference.file_id);
        assert_eq!(first.reference.file_id, "file-api-test-1");
        assert!(!first.key.contains("synthetic-api-key"));
        assert_eq!(
            transport.uploads.lock().unwrap()[0],
            (
                "https://api.deepseek.com/v1/files".into(),
                "image.png".into(),
                "image/png".into(),
                bytes.to_vec(),
            )
        );
    }

    #[tokio::test]
    async fn endpoint_account_model_mime_and_content_are_isolated() {
        let now = DeepSeekImageFileCache::now_unix_seconds();
        let transport = UploadRecorder::with_expiry(now + FILES_TTL_SECONDS as i64);
        let cache = DeepSeekImageFileCache::new(Arc::new(MemoryStore::default()));
        let identities = [
            (
                "https://api.deepseek.com/v1",
                "key-a",
                "model-a",
                "image/png",
                b"a".as_slice(),
            ),
            (
                "https://api.deepseek.com/v2",
                "key-a",
                "model-a",
                "image/png",
                b"a".as_slice(),
            ),
            (
                "https://api.deepseek.com/v1",
                "key-b",
                "model-a",
                "image/png",
                b"a".as_slice(),
            ),
            (
                "https://api.deepseek.com/v1",
                "key-a",
                "model-b",
                "image/png",
                b"a".as_slice(),
            ),
            (
                "https://api.deepseek.com/v1",
                "key-a",
                "model-a",
                "image/jpeg",
                b"a".as_slice(),
            ),
            (
                "https://api.deepseek.com/v1",
                "key-a",
                "model-a",
                "image/png",
                b"b".as_slice(),
            ),
        ];
        for (endpoint, api_key, model, mime, bytes) in identities {
            cache
                .resolve(
                    &transport,
                    FileUploadRequest {
                        url: format!("{}/files", (endpoint).trim_end_matches('/')),
                        api_key: (api_key).to_owned(),
                        filename: ("image.png").to_owned(),
                        mime: (mime).to_owned(),
                        bytes: (bytes).to_vec(),
                    },
                    model,
                    now,
                    None,
                )
                .await
                .unwrap();
        }
        assert_eq!(transport.uploads.lock().unwrap().len(), 6);
    }

    #[tokio::test]
    async fn near_expiry_reference_is_refreshed_with_the_requested_retention() {
        let now = DeepSeekImageFileCache::now_unix_seconds();
        let store = Arc::new(MemoryStore::default());
        let transport = UploadRecorder::with_expiry(now + FILES_TTL_SECONDS as i64);
        let cache = DeepSeekImageFileCache::new(store.clone());
        let first = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com/v1").trim_end_matches('/')
                    ),
                    api_key: ("key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (b"image").to_vec(),
                },
                "model",
                now,
                None,
            )
            .await
            .unwrap();
        transport.set_expiry(Some(now + FILES_TTL_SECONDS as i64 + 2_000));
        let refreshed = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com/v1").trim_end_matches('/')
                    ),
                    api_key: ("key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (b"image").to_vec(),
                },
                "model",
                first.reference.expires_at.unwrap() - 3_600,
                None,
            )
            .await
            .unwrap();

        assert_ne!(first.reference.file_id, refreshed.reference.file_id);
        assert_eq!(transport.uploads.lock().unwrap().len(), 2);
        assert_eq!(
            refreshed.reference.expires_at,
            Some(now + FILES_TTL_SECONDS as i64 + 2_000)
        );
    }

    #[tokio::test]
    async fn missing_expiration_is_rejected_and_not_cached() {
        let now = DeepSeekImageFileCache::now_unix_seconds();
        let store = Arc::new(MemoryStore::default());
        let transport = UploadRecorder::with_expiry(now + FILES_TTL_SECONDS as i64);
        let cache = DeepSeekImageFileCache::new(store.clone());
        let first = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com/v1").trim_end_matches('/')
                    ),
                    api_key: ("key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (b"image").to_vec(),
                },
                "model",
                now,
                None,
            )
            .await
            .unwrap();
        transport.set_expiry(None);
        let error = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com/v1").trim_end_matches('/')
                    ),
                    api_key: ("key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (b"another image").to_vec(),
                },
                "model",
                now,
                None,
            )
            .await
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("did not confirm the requested expiration"));
        assert!(store.get(&first.key).unwrap().is_some());
        assert_eq!(transport.uploads.lock().unwrap().len(), 2);
        assert_eq!(
            transport.deleted.lock().unwrap().as_slice(),
            &["file-api-test-2"]
        );
    }

    struct FailingStore;

    impl ImageFileReferenceStore for FailingStore {
        fn lock(&self, _key: &str) -> Result<Box<dyn Send>> {
            Ok(Box::new(()))
        }
        fn get(&self, _key: &str) -> Result<Option<ImageFileReference>> {
            Ok(None)
        }
        fn put(&self, _key: &str, _value: &ImageFileReference) -> Result<()> {
            Err(CoreError::Persistence("synthetic commit failure".into()))
        }
        fn remove_if_matches(&self, _key: &str, _file_id: &str) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_index_commit_compensates_only_its_uploaded_file() {
        let now = DeepSeekImageFileCache::now_unix_seconds();
        let cache = DeepSeekImageFileCache::new(Arc::new(FailingStore));
        let transport = UploadRecorder::with_expiry(now + FILES_TTL_SECONDS as i64);
        let error = cache
            .resolve(
                &transport,
                FileUploadRequest {
                    url: format!(
                        "{}/files",
                        ("https://api.deepseek.com").trim_end_matches('/')
                    ),
                    api_key: ("key").to_owned(),
                    filename: ("image.png").to_owned(),
                    mime: ("image/png").to_owned(),
                    bytes: (b"image").to_vec(),
                },
                "model",
                now,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, CoreError::Persistence(_)));
        assert_eq!(transport.uploads.lock().unwrap().len(), 1);
        assert_eq!(
            transport.deleted.lock().unwrap().as_slice(),
            &["file-api-test-1"]
        );
    }
}
