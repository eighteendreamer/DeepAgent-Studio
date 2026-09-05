use deepagent_mobile_core::ArtifactRef;
use serde::{Deserialize, Serialize};

/// Maximum allowed artifact size (256 MB).
pub const MAX_ARTIFACT_SIZE: u64 = 256 * 1024 * 1024;

/// Known artifact kinds for query and cleanup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Screenshot,
    ScreenRecording,
    LogDump,
    UiSnapshot,
    NetworkCapture,
    Other,
}

/// Lifecycle state of an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactLifecycle {
    Active,
    Expired,
    Purged,
}

/// Full artifact metadata stored in the artifact store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub artifact_id: String,
    pub device_id: String,
    pub kind: ArtifactKind,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
    pub storage_path: String,
    pub lifecycle: ArtifactLifecycle,
    pub created_at_ms: u64,
    pub expires_at_ms: Option<u64>,
}

impl ArtifactRecord {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        matches!(self.lifecycle, ArtifactLifecycle::Expired)
            || self.expires_at_ms.is_some_and(|exp| now_ms >= exp)
    }
}

impl ArtifactRecord {
    /// Convert to an `ArtifactRef` for event emission.
    pub fn to_ref(&self) -> ArtifactRef {
        ArtifactRef {
            artifact_id: self.artifact_id.clone(),
            mime: self.mime.clone(),
            size_bytes: self.size_bytes,
            sha256: self.sha256.clone(),
            storage_path: self.storage_path.clone(),
        }
    }
}

/// Query filter for listing artifacts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactQuery {
    pub device_id: Option<String>,
    pub kind: Option<ArtifactKind>,
    pub lifecycle: Option<ArtifactLifecycle>,
    pub since_ms: Option<u64>,
    pub before_ms: Option<u64>,
}

/// Request to list artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactListRequest {
    pub query: ArtifactQuery,
    pub limit: u32,
}

/// Response for artifact listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactListResponse {
    pub artifacts: Vec<ArtifactRecord>,
    pub total: u32,
    pub truncated: bool,
}

/// Request to purge expired artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPurgeRequest {
    pub device_id: Option<String>,
    pub dry_run: bool,
}

/// Result of a purge operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactPurgeResult {
    pub purged_count: u32,
    pub purged_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_kind_serde() {
        let kind = ArtifactKind::Screenshot;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"screenshot\"");
        let back: ArtifactKind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, back);
    }

    #[test]
    fn artifact_record_expired_by_time() {
        let record = ArtifactRecord {
            artifact_id: "art-1".into(),
            device_id: "dev-1".into(),
            kind: ArtifactKind::Screenshot,
            mime: "image/png".into(),
            size_bytes: 1024,
            sha256: None,
            storage_path: "/tmp/art-1.png".into(),
            lifecycle: ArtifactLifecycle::Active,
            created_at_ms: 1000,
            expires_at_ms: Some(2000),
        };
        assert!(!record.is_expired(1500));
        assert!(record.is_expired(2000));
        assert!(record.is_expired(3000));
    }

    #[test]
    fn artifact_record_no_expiry_never_expires() {
        let record = ArtifactRecord {
            artifact_id: "art-2".into(),
            device_id: "dev-1".into(),
            kind: ArtifactKind::LogDump,
            mime: "text/plain".into(),
            size_bytes: 512,
            sha256: None,
            storage_path: "/tmp/art-2.log".into(),
            lifecycle: ArtifactLifecycle::Active,
            created_at_ms: 1000,
            expires_at_ms: None,
        };
        assert!(!record.is_expired(u64::MAX));
    }

    #[test]
    fn artifact_record_to_ref() {
        let record = ArtifactRecord {
            artifact_id: "art-3".into(),
            device_id: "dev-1".into(),
            kind: ArtifactKind::Screenshot,
            mime: "image/png".into(),
            size_bytes: 2048,
            sha256: Some("abc".into()),
            storage_path: "/tmp/art-3.png".into(),
            lifecycle: ArtifactLifecycle::Active,
            created_at_ms: 1000,
            expires_at_ms: None,
        };
        let r#ref = record.to_ref();
        assert_eq!(r#ref.artifact_id, "art-3");
        assert_eq!(r#ref.mime, "image/png");
        assert_eq!(r#ref.size_bytes, 2048);
        assert_eq!(r#ref.sha256, Some("abc".into()));
        assert_eq!(r#ref.storage_path, "/tmp/art-3.png");
    }

    #[test]
    fn artifact_query_default_is_empty() {
        let q = ArtifactQuery::default();
        assert!(q.device_id.is_none());
        assert!(q.kind.is_none());
        assert!(q.lifecycle.is_none());
        assert!(q.since_ms.is_none());
        assert!(q.before_ms.is_none());
    }

    #[test]
    fn artifact_list_request_serde() {
        let req = ArtifactListRequest {
            query: ArtifactQuery {
                device_id: Some("dev-1".into()),
                kind: Some(ArtifactKind::Screenshot),
                ..Default::default()
            },
            limit: 50,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ArtifactListRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn purge_result_serde() {
        let result = ArtifactPurgeResult {
            purged_count: 5,
            purged_bytes: 10240,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: ArtifactPurgeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn max_artifact_size_constant() {
        assert_eq!(MAX_ARTIFACT_SIZE, 256 * 1024 * 1024);
    }
}
