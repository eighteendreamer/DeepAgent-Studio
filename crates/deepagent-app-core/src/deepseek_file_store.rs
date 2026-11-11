//! Durable cache for DeepSeek Files API image identities. Original bytes stay
//! in AttachmentService; this SQLite document stores only provider metadata.

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_models::file_references::{ImageFileReference, ImageFileReferenceStore};
use deepagent_persistence::document_store::DocumentStore;
use deepagent_persistence::fs_commit::{lock_operation_file, PathOperationGuard};
use deepagent_persistence::Database;
use std::path::PathBuf;
use std::sync::Arc;

const COLLECTION: &str = "deepseek_image_file_references";

#[derive(Clone)]
pub(crate) struct SqliteImageFileReferenceStore {
    db: Arc<Database>,
}

impl SqliteImageFileReferenceStore {
    pub(crate) fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    fn lock_path(&self, key: &str) -> Result<PathBuf> {
        let directory = self
            .db
            .data_root()
            .join("files")
            .join("deepseek-image-reference-locks");
        std::fs::create_dir_all(&directory).map_err(|error| {
            CoreError::Persistence(format!("create image reference lock directory: {error}"))
        })?;
        let directory = std::fs::canonicalize(directory).map_err(|error| {
            CoreError::Persistence(format!("resolve image reference lock directory: {error}"))
        })?;
        Ok(directory.join(format!("{key}.lock")))
    }
}

impl ImageFileReferenceStore for SqliteImageFileReferenceStore {
    fn lock(&self, key: &str) -> Result<Box<dyn Send>> {
        let path = self.lock_path(key)?;
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::other("image reference lock has no parent"))?;
        std::fs::create_dir_all(parent).map_err(|error| {
            CoreError::Persistence(format!("create image reference lock directory: {error}"))
        })?;
        let guard: PathOperationGuard = lock_operation_file(&path)
            .map_err(|error| CoreError::Persistence(format!("lock image reference: {error}")))?;
        Ok(Box::new(guard))
    }

    fn get(&self, key: &str) -> Result<Option<ImageFileReference>> {
        let Some(document) = DocumentStore::new(&self.db).get(COLLECTION, key)? else {
            return Ok(None);
        };
        serde_json::from_str(&document.body)
            .map(Some)
            .map_err(|_| CoreError::Persistence("invalid DeepSeek image reference record".into()))
    }

    fn put(&self, key: &str, value: &ImageFileReference) -> Result<()> {
        let body = serde_json::to_string(value)?;
        DocumentStore::new(&self.db).put(COLLECTION, key, &body, None, SystemClock.now())
    }

    fn remove_if_matches(&self, key: &str, file_id: &str) -> Result<()> {
        let Some(current) = self.get(key)? else {
            return Ok(());
        };
        if current.file_id == file_id {
            DocumentStore::new(&self.db).delete(COLLECTION, key)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use deepagent_models::file_references::{DeepSeekImageFileCache, FILES_TTL_SECONDS};
    use deepagent_models::transport::{
        EventSink, FileUploadRequest, HttpTransport, TransportRequest, UploadedFile,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn provider_reference_survives_sqlite_reopen_without_key_or_image_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let api_key = "synthetic-api-key-never-persist";
        let image_bytes = b"synthetic image bytes that are not stored";
        let key = deepagent_models::file_references::DeepSeekImageFileCache::key(
            "https://api.deepseek.com/v1",
            api_key,
            "deepseek-chat",
            "image/png",
            image_bytes,
        );
        let reference = ImageFileReference {
            file_id: "file-api-synthetic-opaque-id".into(),
            expires_at: Some(1_900_000_000),
        };

        let first_db = Arc::new(Database::open(&path).unwrap());
        let first_store = SqliteImageFileReferenceStore::new(first_db.clone());
        first_store.put(&key, &reference).unwrap();
        drop(first_store);
        drop(first_db);

        let second_db = Arc::new(Database::open(&path).unwrap());
        let second_store = SqliteImageFileReferenceStore::new(second_db.clone());
        assert_eq!(second_store.get(&key).unwrap(), Some(reference.clone()));
        let document = DocumentStore::new(&second_db)
            .get(COLLECTION, &key)
            .unwrap()
            .unwrap();
        assert!(!document.id.contains(api_key));
        assert!(!document.body.contains(api_key));
        assert!(!document
            .body
            .contains(std::str::from_utf8(image_bytes).unwrap()));
        assert_eq!(document.body, serde_json::to_string(&reference).unwrap());
    }

    struct DelayedUpload(AtomicUsize);

    #[async_trait]
    impl HttpTransport for DelayedUpload {
        async fn stream(
            &self,
            _request: TransportRequest,
            _sink: &mut dyn EventSink,
        ) -> Result<()> {
            panic!("cache test must not dispatch a model")
        }

        async fn upload_file(&self, _request: FileUploadRequest) -> Result<UploadedFile> {
            let count = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            Ok(UploadedFile {
                id: format!("file-api-concurrent-{count}"),
                expires_at: Some(
                    DeepSeekImageFileCache::now_unix_seconds() + FILES_TTL_SECONDS as i64,
                ),
            })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn separate_sqlite_handles_share_one_upload_without_blocking_the_async_runtime() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let one = DeepSeekImageFileCache::new(Arc::new(SqliteImageFileReferenceStore::new(
            Arc::new(Database::open(&path).unwrap()),
        )));
        let two = DeepSeekImageFileCache::new(Arc::new(SqliteImageFileReferenceStore::new(
            Arc::new(Database::open(&path).unwrap()),
        )));
        let transport = DelayedUpload(AtomicUsize::new(0));
        let now = DeepSeekImageFileCache::now_unix_seconds();
        let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(
                one.resolve(
                    &transport,
                    FileUploadRequest {
                        url: format!(
                            "{}/files",
                            ("https://api.deepseek.com").trim_end_matches('/')
                        ),
                        api_key: ("synthetic").to_owned(),
                        filename: ("image.png").to_owned(),
                        mime: ("image/png").to_owned(),
                        bytes: (b"same-image").to_vec(),
                    },
                    "model",
                    now,
                    None,
                ),
                two.resolve(
                    &transport,
                    FileUploadRequest {
                        url: format!(
                            "{}/files",
                            ("https://api.deepseek.com").trim_end_matches('/')
                        ),
                        api_key: ("synthetic").to_owned(),
                        filename: ("image.png").to_owned(),
                        mime: ("image/png").to_owned(),
                        bytes: (b"same-image").to_vec(),
                    },
                    "model",
                    now,
                    None,
                ),
            )
        })
        .await
        .expect("a file gate must not block the single-threaded executor");
        assert_eq!(first.unwrap().reference, second.unwrap().reference);
        assert_eq!(transport.0.load(Ordering::SeqCst), 1);
    }
}
