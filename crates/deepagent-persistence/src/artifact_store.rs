//! Metadata index for stored artifacts: truncated tool output and canvas media.
//!
//! One table owns every stored blob's *index*; the bytes themselves live on
//! disk under a managed root and are written by whoever imports them. This
//! replaced the run-scoped `tool_artifacts` index so that canvas images, video
//! and audio — which have no owning run — share the same bookkeeping instead of
//! growing a second media store.

use deepagent_core::error::{CoreError, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::{map_sqlite, Database};

/// What an artifact is. Persisted as its lowercase label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// Truncated tool output kept out of the event stream.
    ToolResult,
    Image,
    Video,
    Audio,
    /// Anything stored but not renderable as media (documents, JSON dumps).
    Document,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolResult => "tool_result",
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "tool_result" => Some(Self::ToolResult),
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            "audio" => Some(Self::Audio),
            "document" => Some(Self::Document),
            _ => None,
        }
    }
}

/// One stored blob and where to find it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub id: String,
    pub kind: ArtifactKind,
    /// Owning run for tool output; `None` for canvas media.
    pub run_id: Option<String>,
    pub call_id: Option<String>,
    pub workspace_id: Option<String>,
    /// Absolute path of the bytes on disk.
    pub path: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub digest: Option<String>,
    pub created_at: i64,
}

const COLUMNS: &str =
    "id, kind, run_id, call_id, workspace_id, path, media_type, byte_size, digest, created_at";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArtifactRecord> {
    let kind_label: String = row.get(1)?;
    Ok(ArtifactRecord {
        id: row.get(0)?,
        kind: ArtifactKind::from_label(&kind_label).unwrap_or(ArtifactKind::Document),
        run_id: row.get(2)?,
        call_id: row.get(3)?,
        workspace_id: row.get(4)?,
        path: row.get(5)?,
        media_type: row.get(6)?,
        byte_size: row.get(7)?,
        digest: row.get(8)?,
        created_at: row.get(9)?,
    })
}

pub struct ArtifactStore<'db> {
    db: &'db Database,
}

impl<'db> ArtifactStore<'db> {
    pub const fn new(db: &'db Database) -> Self {
        Self { db }
    }

    pub fn put(&self, record: &ArtifactRecord) -> Result<()> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO artifacts (id, kind, run_id, call_id, workspace_id, path, media_type, byte_size, digest, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.id,
                    record.kind.as_str(),
                    record.run_id,
                    record.call_id,
                    record.workspace_id,
                    record.path,
                    record.media_type,
                    record.byte_size,
                    record.digest,
                    record.created_at,
                ],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<ArtifactRecord>> {
        let statement = format!("SELECT {COLUMNS} FROM artifacts WHERE id=?1");
        self.db.with_conn(|conn| {
            let mut statement = conn.prepare(&statement).map_err(map_sqlite)?;
            let mut rows = statement
                .query_map([id], row_to_record)
                .map_err(map_sqlite)?;
            match rows.next() {
                Some(row) => row.map(Some).map_err(map_sqlite),
                None => Ok(None),
            }
        })
    }

    pub fn list_for_run(&self, run_id: &str) -> Result<Vec<ArtifactRecord>> {
        let statement =
            format!("SELECT {COLUMNS} FROM artifacts WHERE run_id=?1 ORDER BY created_at, id");
        self.db.with_conn(|conn| {
            let mut statement = conn.prepare(&statement).map_err(map_sqlite)?;
            let rows = statement
                .query_map([run_id], row_to_record)
                .map_err(map_sqlite)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }

    /// Workspace artifacts, newest first, optionally narrowed to one kind.
    pub fn list_for_workspace(
        &self,
        workspace_id: &str,
        kind: Option<ArtifactKind>,
    ) -> Result<Vec<ArtifactRecord>> {
        let statement = match kind {
            Some(_) => format!(
                "SELECT {COLUMNS} FROM artifacts WHERE workspace_id=?1 AND kind=?2 ORDER BY created_at DESC, id DESC"
            ),
            None => format!(
                "SELECT {COLUMNS} FROM artifacts WHERE workspace_id=?1 ORDER BY created_at DESC, id DESC"
            ),
        };
        self.db.with_conn(|conn| {
            let mut statement = conn.prepare(&statement).map_err(map_sqlite)?;
            let rows = match kind {
                Some(kind) => statement
                    .query_map(params![workspace_id, kind.as_str()], row_to_record)
                    .map_err(map_sqlite)?,
                None => statement
                    .query_map(params![workspace_id], row_to_record)
                    .map_err(map_sqlite)?,
            };
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)
        })
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        self.db.with_conn(|conn| {
            let removed = conn
                .execute("DELETE FROM artifacts WHERE id=?1", params![id])
                .map_err(map_sqlite)?;
            Ok(removed > 0)
        })
    }

    /// Reject ids that would escape a managed root when joined to it.
    pub fn validate_id(id: &str) -> Result<()> {
        let ok = !id.is_empty()
            && id.len() <= 120
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if ok {
            Ok(())
        } else {
            Err(CoreError::invalid(format!(
                "artifact id `{id}` must be 1..=120 ASCII letters, digits, underscore or hyphen"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_store::RunStore;

    fn record(id: &str, kind: ArtifactKind) -> ArtifactRecord {
        ArtifactRecord {
            id: id.to_string(),
            kind,
            run_id: None,
            call_id: None,
            workspace_id: None,
            path: format!("/tmp/{id}"),
            media_type: Some("image/png".to_string()),
            byte_size: 42,
            digest: Some("sha256:abc".to_string()),
            created_at: 2,
        }
    }

    #[test]
    fn records_and_lists_tool_artifacts_by_run() {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at) VALUES ('s1', 1, 1)",
                [],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
        .unwrap();
        RunStore::new(&db).create("r1", "s1", None, 1).unwrap();
        let mut tool_output = record("a1", ArtifactKind::ToolResult);
        tool_output.run_id = Some("r1".to_string());
        tool_output.call_id = Some("c1".to_string());
        tool_output.media_type = Some("application/json".to_string());
        ArtifactStore::new(&db).put(&tool_output).unwrap();

        let records = ArtifactStore::new(&db).list_for_run("r1").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].call_id.as_deref(), Some("c1"));
        assert_eq!(records[0].kind, ArtifactKind::ToolResult);
        let json = serde_json::to_value(&records[0]).unwrap();
        assert_eq!(json["runId"], "r1");
        assert_eq!(json["kind"], "tool_result");
        assert!(json.get("run_id").is_none());
        assert_eq!(
            ArtifactStore::new(&db)
                .get("a1")
                .unwrap()
                .expect("present")
                .path,
            "/tmp/a1"
        );
        assert!(ArtifactStore::new(&db).get("missing").unwrap().is_none());
    }

    #[test]
    fn canvas_media_is_indexed_per_workspace_without_a_run() {
        let db = Database::open_in_memory().unwrap();
        let mut image = record("art_img_1", ArtifactKind::Image);
        image.workspace_id = Some("ws-1".to_string());
        image.created_at = 10;
        let mut video = record("art_vid_1", ArtifactKind::Video);
        video.workspace_id = Some("ws-1".to_string());
        video.created_at = 20;
        let mut other = record("art_img_2", ArtifactKind::Image);
        other.workspace_id = Some("ws-2".to_string());
        for item in [&image, &video, &other] {
            ArtifactStore::new(&db).put(item).unwrap();
        }

        let all = ArtifactStore::new(&db)
            .list_for_workspace("ws-1", None)
            .unwrap();
        assert_eq!(all.len(), 2);
        // Newest first.
        assert_eq!(all[0].id, "art_vid_1");
        let images = ArtifactStore::new(&db)
            .list_for_workspace("ws-1", Some(ArtifactKind::Image))
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, "art_img_1");
        assert!(ArtifactStore::new(&db)
            .list_for_run("r-none")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn delete_removes_the_row_once() {
        let db = Database::open_in_memory().unwrap();
        ArtifactStore::new(&db)
            .put(&record("a1", ArtifactKind::Image))
            .unwrap();
        assert!(ArtifactStore::new(&db).delete("a1").unwrap());
        assert!(!ArtifactStore::new(&db).delete("a1").unwrap());
    }

    #[test]
    fn rejects_id_shapes_that_could_escape_the_managed_root() {
        assert!(ArtifactStore::validate_id("art_0196-abc").is_ok());
        for bad in ["", "..", "a/b", "a\\b", "with space", &"x".repeat(121)] {
            assert!(ArtifactStore::validate_id(bad).is_err(), "accepted {bad:?}");
        }
    }
}
