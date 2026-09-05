use deepagent_mobile_core::ArtifactRef;
use deepagent_mobile_protocol::{
    ArtifactKind, ArtifactLifecycle, ArtifactPurgeResult, ArtifactQuery, ArtifactRecord,
    MAX_ARTIFACT_SIZE,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Parameters for registering a new artifact.
#[derive(Debug, Clone)]
pub struct RegisterArtifactRequest<'a> {
    pub device_id: &'a str,
    pub kind: ArtifactKind,
    pub mime: &'a str,
    pub size_bytes: u64,
    pub sha256: Option<&'a str>,
    pub storage_path: &'a str,
    pub ttl_ms: Option<u64>,
}

/// In-memory artifact store. Tracks artifact metadata, enforces size limits,
/// and supports query/purge operations.
///
/// The store does not manage actual file I/O — it tracks metadata and lifecycle.
/// File writing is the caller's responsibility; the store records the resulting
/// `ArtifactRecord` after successful writes.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    inner: Arc<Mutex<StoreInner>>,
}

#[derive(Debug)]
struct StoreInner {
    artifacts: HashMap<String, ArtifactRecord>,
    next_id: u64,
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(StoreInner {
                artifacts: HashMap::new(),
                next_id: 1,
            })),
        }
    }

    /// Register a new artifact. Returns the assigned `ArtifactRecord`.
    ///
    /// Rejects artifacts exceeding `MAX_ARTIFACT_SIZE`.
    pub fn register(
        &self,
        req: RegisterArtifactRequest<'_>,
    ) -> Result<ArtifactRecord, ArtifactStoreError> {
        if req.size_bytes > MAX_ARTIFACT_SIZE {
            return Err(ArtifactStoreError::ExceedsMaxSize {
                size: req.size_bytes,
                max: MAX_ARTIFACT_SIZE,
            });
        }

        let now = current_time_ms();
        let mut inner = self.inner.lock().unwrap();
        let artifact_id = format!("art-{}", inner.next_id);
        inner.next_id += 1;

        let record = ArtifactRecord {
            artifact_id,
            device_id: req.device_id.to_string(),
            kind: req.kind,
            mime: req.mime.to_string(),
            size_bytes: req.size_bytes,
            sha256: req.sha256.map(|s| s.to_string()),
            storage_path: req.storage_path.to_string(),
            lifecycle: ArtifactLifecycle::Active,
            created_at_ms: now,
            expires_at_ms: req.ttl_ms.map(|ttl| now + ttl),
        };

        inner
            .artifacts
            .insert(record.artifact_id.clone(), record.clone());
        Ok(record)
    }

    /// Look up an artifact by ID.
    pub fn get(&self, artifact_id: &str) -> Option<ArtifactRecord> {
        let inner = self.inner.lock().unwrap();
        inner.artifacts.get(artifact_id).cloned()
    }

    /// Convert an artifact to an `ArtifactRef` for event emission.
    pub fn to_ref(&self, artifact_id: &str) -> Option<ArtifactRef> {
        self.get(artifact_id).map(|r| r.to_ref())
    }

    /// List artifacts matching the query filter.
    pub fn list(&self, query: &ArtifactQuery, limit: u32) -> (Vec<ArtifactRecord>, u32, bool) {
        let inner = self.inner.lock().unwrap();
        let now = current_time_ms();

        let matched: Vec<ArtifactRecord> = inner
            .artifacts
            .values()
            .filter(|r| matches_query(r, query, now))
            .cloned()
            .collect();

        let total = matched.len() as u32;
        let truncated = total > limit;
        let page = matched.into_iter().take(limit as usize).collect();
        (page, total, truncated)
    }

    /// Mark an artifact as expired.
    pub fn expire(&self, artifact_id: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if let Some(record) = inner.artifacts.get_mut(artifact_id) {
            record.lifecycle = ArtifactLifecycle::Expired;
            true
        } else {
            false
        }
    }

    /// Purge expired or time-expired artifacts. Returns purge statistics.
    ///
    /// When `device_id` is `Some`, only artifacts for that device are considered.
    /// When `dry_run` is true, no artifacts are actually removed.
    pub fn purge_expired(&self, device_id: Option<&str>, dry_run: bool) -> ArtifactPurgeResult {
        let mut inner = self.inner.lock().unwrap();
        let now = current_time_ms();

        let to_purge: Vec<String> = inner
            .artifacts
            .values()
            .filter(|r| {
                if let Some(did) = device_id {
                    if r.device_id != did {
                        return false;
                    }
                }
                r.is_expired(now)
            })
            .map(|r| r.artifact_id.clone())
            .collect();

        let mut purged_bytes = 0u64;
        let mut purged_count = 0u32;

        if dry_run {
            return ArtifactPurgeResult {
                purged_count: 0,
                purged_bytes: 0,
            };
        }

        for id in &to_purge {
            if let Some(record) = inner.artifacts.remove(id) {
                purged_bytes += record.size_bytes;
                purged_count += 1;
            }
        }

        ArtifactPurgeResult {
            purged_count,
            purged_bytes,
        }
    }

    /// Remove all artifacts for a device (e.g., on device disconnect).
    pub fn clear_device(&self, device_id: &str) -> u32 {
        let mut inner = self.inner.lock().unwrap();
        let before = inner.artifacts.len();
        inner.artifacts.retain(|_, r| r.device_id != device_id);
        (before - inner.artifacts.len()) as u32
    }

    /// Total number of tracked artifacts.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.artifacts.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total size of all tracked artifacts in bytes.
    pub fn total_size(&self) -> u64 {
        let inner = self.inner.lock().unwrap();
        inner.artifacts.values().map(|r| r.size_bytes).sum()
    }
}

fn matches_query(record: &ArtifactRecord, query: &ArtifactQuery, now: u64) -> bool {
    if let Some(ref did) = query.device_id {
        if record.device_id != *did {
            return false;
        }
    }
    if let Some(ref kind) = query.kind {
        if record.kind != *kind {
            return false;
        }
    }
    if let Some(ref lifecycle) = query.lifecycle {
        if record.lifecycle != *lifecycle {
            return false;
        }
    }
    if let Some(since) = query.since_ms {
        if record.created_at_ms < since {
            return false;
        }
    }
    if let Some(before) = query.before_ms {
        if record.created_at_ms >= before {
            return false;
        }
    }
    let _ = now;
    true
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Errors from artifact store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactStoreError {
    ExceedsMaxSize { size: u64, max: u64 },
}

impl std::fmt::Display for ArtifactStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExceedsMaxSize { size, max } => {
                write!(f, "artifact size {size} exceeds maximum {max}")
            }
        }
    }
}

impl std::error::Error for ArtifactStoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(
        device_id: &'a str,
        kind: ArtifactKind,
        mime: &'a str,
        size: u64,
        path: &'a str,
    ) -> RegisterArtifactRequest<'a> {
        RegisterArtifactRequest {
            device_id,
            kind,
            mime,
            size_bytes: size,
            sha256: None,
            storage_path: path,
            ttl_ms: None,
        }
    }

    #[test]
    fn register_and_get() {
        let store = ArtifactStore::new();
        let record = store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                1024,
                "/tmp/s.png",
            ))
            .unwrap();
        assert_eq!(record.artifact_id, "art-1");
        assert_eq!(record.device_id, "dev-1");
        assert_eq!(record.size_bytes, 1024);
        assert_eq!(record.lifecycle, ArtifactLifecycle::Active);

        let fetched = store.get("art-1").unwrap();
        assert_eq!(fetched.artifact_id, "art-1");
    }

    #[test]
    fn register_rejects_oversized() {
        let store = ArtifactStore::new();
        let result = store.register(RegisterArtifactRequest {
            device_id: "dev-1",
            kind: ArtifactKind::ScreenRecording,
            mime: "video/mp4",
            size_bytes: MAX_ARTIFACT_SIZE + 1,
            sha256: None,
            storage_path: "/tmp/big.mp4",
            ttl_ms: None,
        });
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ArtifactStoreError::ExceedsMaxSize { .. }
        ));
    }

    #[test]
    fn register_with_hash() {
        let store = ArtifactStore::new();
        let record = store
            .register(RegisterArtifactRequest {
                device_id: "dev-1",
                kind: ArtifactKind::Screenshot,
                mime: "image/png",
                size_bytes: 512,
                sha256: Some("deadbeef"),
                storage_path: "/tmp/s.png",
                ttl_ms: None,
            })
            .unwrap();
        assert_eq!(record.sha256, Some("deadbeef".into()));
    }

    #[test]
    fn to_ref_returns_artifact_ref() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                256,
                "/tmp/s.png",
            ))
            .unwrap();
        let r = store.to_ref("art-1").unwrap();
        assert_eq!(r.artifact_id, "art-1");
        assert_eq!(r.mime, "image/png");
        assert_eq!(r.size_bytes, 256);
        assert_eq!(r.storage_path, "/tmp/s.png");
    }

    #[test]
    fn to_ref_returns_none_for_missing() {
        let store = ArtifactStore::new();
        assert!(store.to_ref("nonexistent").is_none());
    }

    #[test]
    fn list_by_device() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-2",
                ArtifactKind::Screenshot,
                "image/png",
                200,
                "/tmp/b.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-1",
                ArtifactKind::LogDump,
                "text/plain",
                300,
                "/tmp/c.log",
            ))
            .unwrap();

        let query = ArtifactQuery {
            device_id: Some("dev-1".into()),
            ..Default::default()
        };
        let (results, total, truncated) = store.list(&query, 100);
        assert_eq!(total, 2);
        assert!(!truncated);
        assert!(results.iter().all(|r| r.device_id == "dev-1"));
    }

    #[test]
    fn list_by_kind() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-1",
                ArtifactKind::LogDump,
                "text/plain",
                200,
                "/tmp/b.log",
            ))
            .unwrap();

        let query = ArtifactQuery {
            kind: Some(ArtifactKind::LogDump),
            ..Default::default()
        };
        let (results, total, _) = store.list(&query, 100);
        assert_eq!(total, 1);
        assert_eq!(results[0].kind, ArtifactKind::LogDump);
    }

    #[test]
    fn list_with_limit_truncates() {
        let store = ArtifactStore::new();
        for i in 0..5 {
            store
                .register(req(
                    "dev-1",
                    ArtifactKind::Screenshot,
                    "image/png",
                    100,
                    &format!("/tmp/{i}.png"),
                ))
                .unwrap();
        }

        let (results, total, truncated) = store.list(&ArtifactQuery::default(), 3);
        assert_eq!(total, 5);
        assert!(truncated);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn expire_and_purge() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-1",
                ArtifactKind::LogDump,
                "text/plain",
                200,
                "/tmp/b.log",
            ))
            .unwrap();

        assert!(store.expire("art-1"));
        assert_eq!(
            store.get("art-1").unwrap().lifecycle,
            ArtifactLifecycle::Expired
        );

        let result = store.purge_expired(None, false);
        assert_eq!(result.purged_count, 1);
        assert_eq!(result.purged_bytes, 100);
        assert!(store.get("art-1").is_none());
        assert!(store.get("art-2").is_some());
    }

    #[test]
    fn purge_dry_run_does_not_remove() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store.expire("art-1");

        let result = store.purge_expired(None, true);
        assert_eq!(result.purged_count, 0);
        assert_eq!(result.purged_bytes, 0);
        assert!(store.get("art-1").is_some());
    }

    #[test]
    fn purge_by_device() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-2",
                ArtifactKind::Screenshot,
                "image/png",
                200,
                "/tmp/b.png",
            ))
            .unwrap();
        store.expire("art-1");
        store.expire("art-2");

        let result = store.purge_expired(Some("dev-1"), false);
        assert_eq!(result.purged_count, 1);
        assert!(store.get("art-1").is_none());
        assert!(store.get("art-2").is_some());
    }

    #[test]
    fn clear_device_removes_all_for_device() {
        let store = ArtifactStore::new();
        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-1",
                ArtifactKind::LogDump,
                "text/plain",
                200,
                "/tmp/b.log",
            ))
            .unwrap();
        store
            .register(req(
                "dev-2",
                ArtifactKind::Screenshot,
                "image/png",
                300,
                "/tmp/c.png",
            ))
            .unwrap();

        let removed = store.clear_device("dev-1");
        assert_eq!(removed, 2);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn len_and_total_size() {
        let store = ArtifactStore::new();
        assert!(store.is_empty());
        assert_eq!(store.total_size(), 0);

        store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        store
            .register(req(
                "dev-1",
                ArtifactKind::LogDump,
                "text/plain",
                200,
                "/tmp/b.log",
            ))
            .unwrap();

        assert_eq!(store.len(), 2);
        assert!(!store.is_empty());
        assert_eq!(store.total_size(), 300);
    }

    #[test]
    fn sequential_ids() {
        let store = ArtifactStore::new();
        let r1 = store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/a.png",
            ))
            .unwrap();
        let r2 = store
            .register(req(
                "dev-1",
                ArtifactKind::Screenshot,
                "image/png",
                100,
                "/tmp/b.png",
            ))
            .unwrap();
        assert_eq!(r1.artifact_id, "art-1");
        assert_eq!(r2.artifact_id, "art-2");
    }

    #[test]
    fn ttl_sets_expiry() {
        let store = ArtifactStore::new();
        let record = store
            .register(RegisterArtifactRequest {
                device_id: "dev-1",
                kind: ArtifactKind::Screenshot,
                mime: "image/png",
                size_bytes: 100,
                sha256: None,
                storage_path: "/tmp/a.png",
                ttl_ms: Some(60_000),
            })
            .unwrap();
        assert!(record.expires_at_ms.is_some());
        assert!(record.expires_at_ms.unwrap() > record.created_at_ms);
    }
}
