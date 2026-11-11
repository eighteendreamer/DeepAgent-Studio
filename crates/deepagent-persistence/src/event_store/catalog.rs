//! Bounded metadata/keyset query on the existing session projection. The only
//! extra durable state is a change epoch; provider remains authority-header data.
use super::*;
use deepagent_core::session_catalog::{
    SessionSortDirection, SessionSortKey, MAX_SESSION_PAGE_SIZE,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCatalogFilter {
    pub archived: bool,
    pub cwd: Option<String>,
    pub provider: Option<String>,
    pub search: Option<String>,
    pub updated_after: Option<i64>,
    pub sort_key: SessionSortKey,
    pub sort_direction: SessionSortDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionCatalogPosition {
    identity: String,
    revision: i64,
    number: Option<i64>,
    id: String,
}

impl SessionCatalogPosition {
    fn begin(tx: &rusqlite::Connection, cursor: Option<&Self>) -> Result<Self> {
        let (identity, revision): (String, i64) = tx
            .query_row(
                "SELECT identity,revision FROM session_catalog_state WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(map_sqlite)?;
        match cursor {
            Some(c) if c.identity != identity || c.revision != revision => Err(CoreError::invalid(
                "stale session catalog cursor; refresh from first page",
            )),
            Some(c) if c.id.len() > 128 || c.number.is_none() => {
                Err(CoreError::invalid("invalid session catalog position"))
            }
            Some(c) => Ok(c.clone()),
            None => Ok(Self {
                identity,
                revision,
                number: None,
                id: String::new(),
            }),
        }
    }
}

fn visible(root: &std::path::Path, id: SessionId, provider: Option<&String>) -> Result<bool> {
    match session_files::current_cursor(root, id) {
        Ok(None) => return Ok(false),
        Err(error) if provider.is_some() => return Err(error),
        // A metadata-only summary remains selectable on file-local failures.
        Ok(Some(_)) | Err(_) => {}
    }
    let Some(expected) = provider else {
        return Ok(true);
    };
    let actual = session_files::page::catalog_header(root, id)?
        .and_then(|h| h.execution_config)
        .and_then(|c| c.overrides.provider);
    Ok(actual.as_ref() == Some(expected))
}

#[derive(Debug)]
pub struct SessionCatalogRow {
    pub session: SessionRecord,
    pub position: SessionCatalogPosition,
}
#[derive(Debug)]
pub struct SessionCatalogPage {
    /// At most 100 candidate rows are inspected, even for sparse provider filters.
    pub rows: Vec<SessionCatalogRow>,
    pub end: SessionCatalogPosition,
    pub has_more: bool,
}

impl SessionCatalogFilter {
    pub fn validate(&self) -> Result<()> {
        for (value, max) in [
            (&self.cwd, 1024),
            (&self.provider, 128),
            (&self.search, 128),
        ] {
            if value
                .as_ref()
                .is_some_and(|s| s.trim().is_empty() || s.len() > max || s.contains('\0'))
            {
                return Err(CoreError::invalid(
                    "invalid session catalog filter length or value",
                ));
            }
        }
        Ok(())
    }
}

impl EventStore<'_> {
    pub(crate) fn catalog_changed(&self) -> Result<()> {
        self.db.with_conn(|c| {
            c.execute(
                "UPDATE session_catalog_state SET revision=revision+1 WHERE singleton=1",
                [],
            )
            .map_err(map_sqlite)?;
            Ok(())
        })
    }

    pub fn list_session_page(
        &self,
        filter: &SessionCatalogFilter,
        cursor: Option<&SessionCatalogPosition>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<SessionCatalogPage> {
        filter.validate()?;
        if !(1..=MAX_SESSION_PAGE_SIZE).contains(&limit)
            || !(4096..=1024 * 1024).contains(&max_bytes)
        {
            return Err(CoreError::invalid(
                "invalid session catalog limit or byte budget",
            ));
        }
        self.db.with_conn(|c| {
            let tx = c.unchecked_transaction().map_err(map_sqlite)?;
            let page = self.read_catalog_snapshot(&tx, filter, cursor, limit, max_bytes)?;
            tx.commit().map_err(map_sqlite)?;
            Ok(page)
        })
    }

    fn read_catalog_snapshot(
        &self,
        tx: &rusqlite::Connection,
        filter: &SessionCatalogFilter,
        cursor: Option<&SessionCatalogPosition>,
        limit: usize,
        max_bytes: usize,
    ) -> Result<SessionCatalogPage> {
        let mut position = SessionCatalogPosition::begin(tx, cursor)?;
        let column = match filter.sort_key {
            SessionSortKey::CreatedAt => "s.created_at",
            SessionSortKey::UpdatedAt => "s.updated_at",
        };
        let (op, direction) = match filter.sort_direction {
            SessionSortDirection::Asc => (">", "ASC"),
            SessionSortDirection::Desc => ("<", "DESC"),
        };
        let after = if position.number.is_some() {
            format!("({column},s.id) {op} (:number,:id)")
        } else {
            ":number IS NULL AND :id=''".into()
        };
        // Static column/operator selection, every caller value is bound.
        let sql=format!("SELECT s.rowid,{column},s.id FROM sessions s JOIN session_file_state f ON f.session_id=s.id
                WHERE (:cwd IS NULL OR s.project=:cwd)
                AND (:search IS NULL OR instr(COALESCE(s.title,''),:search)>0)
                AND (:updated IS NULL OR s.updated_at>:updated)
                AND EXISTS(SELECT 1 FROM documents a WHERE a.collection='archived_conversations' AND a.id=s.id)=:archived
                AND ({after})
                ORDER BY {column} {direction},s.id {direction} LIMIT 101");
        let mut statement = tx.prepare(&sql).map_err(map_sqlite)?;
        let mut keys=statement.query(rusqlite::named_params! {":cwd":filter.cwd,":search":filter.search,":updated":filter.updated_after,":archived":filter.archived,":number":position.number,":id":position.id}).map_err(map_sqlite)?;
        let mut hydrate=tx.prepare("SELECT id,title,mode,project,created_at,updated_at,ended_at FROM sessions WHERE rowid=?1").map_err(map_sqlite)?;
        let mut rows = Vec::new();
        let mut scanned = 0;
        let mut bytes = 0;
        let mut has_more = false;
        while let Some(key) = keys.next().map_err(map_sqlite)? {
            if scanned == MAX_SESSION_PAGE_SIZE || rows.len() == limit {
                has_more = true;
                break;
            }
            let rowid: i64 = key.get(0).map_err(map_sqlite)?;
            let number: i64 = key.get(1).map_err(map_sqlite)?;
            crate::bounded_read::check_raw_row(key, 256)?;
            let id: String = key.get(2).map_err(map_sqlite)?;
            let session_id = id
                .parse::<SessionId>()
                .map_err(|_| CoreError::Persistence("invalid catalog session identity".into()))?;
            let mut raw = hydrate.query([rowid]).map_err(map_sqlite)?;
            let row = raw
                .next()
                .map_err(map_sqlite)?
                .ok_or_else(|| CoreError::Persistence("catalog row disappeared".into()))?;
            crate::bounded_read::check_raw_row(row, max_bytes.min(64 * 1024))?;
            if visible(
                &self.db.session_files_root(),
                session_id,
                filter.provider.as_ref(),
            )? {
                let mode: String = row.get(2).map_err(map_sqlite)?;
                let ended: Option<i64> = row.get(6).map_err(map_sqlite)?;
                let session = SessionRecord {
                    id: session_id,
                    title: row.get(1).map_err(map_sqlite)?,
                    mode: parse_mode(&mode),
                    project: row.get(3).map_err(map_sqlite)?,
                    created_at: Timestamp::from_millis(row.get(4).map_err(map_sqlite)?),
                    updated_at: Timestamp::from_millis(row.get(5).map_err(map_sqlite)?),
                    ended_at: ended.map(Timestamp::from_millis),
                };
                let size = crate::bounded_read::json_len(
                    &(
                        &session.title,
                        &session.project,
                        session.created_at.as_millis(),
                        session.updated_at.as_millis(),
                        ended,
                        id.as_str(),
                    ),
                    max_bytes,
                )?;
                if bytes + size > max_bytes && !rows.is_empty() {
                    has_more = true;
                    break;
                }
                bytes += size;
                let next = SessionCatalogPosition {
                    number: Some(number),
                    id: id.clone(),
                    ..position.clone()
                };
                rows.push(SessionCatalogRow {
                    session,
                    position: next,
                });
            }
            position.number = Some(number);
            position.id = id;
            scanned += 1;
        }
        drop(keys);
        drop(statement);
        drop(hydrate);
        Ok(SessionCatalogPage {
            rows,
            end: position,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::execution_config::{ExecutionOverrides, SessionExecutionConfig};

    #[test]
    fn h220_filter_and_key_limit_precede_large_payload_hydration() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let good = SessionId::new();
        let huge = SessionId::new();
        for (id, project, time) in [(good, "keep", 2), (huge, "other", 1)] {
            store
                .create_session_full(
                    id,
                    None,
                    SessionMode::Normal,
                    Some(project),
                    Timestamp::from_millis(time),
                )
                .unwrap();
        }
        db.with_conn(|c| {
            c.execute(
                "UPDATE sessions SET title=?1 WHERE id=?2",
                params!["x".repeat(2 * 1024 * 1024), huge.to_string()],
            )
            .map(|_| ())
            .map_err(map_sqlite)
        })
        .unwrap();
        let page = store
            .list_session_page(&SessionCatalogFilter::default(), None, 1, 4096)
            .unwrap();
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].session.id, good);
        assert!(page.has_more);
        assert!(store
            .list_session_page(&SessionCatalogFilter::default(), Some(&page.end), 100, 4096)
            .is_err());
        let filter = SessionCatalogFilter {
            cwd: Some("keep".into()),
            ..Default::default()
        };
        let page = store.list_session_page(&filter, None, 100, 4096).unwrap();
        assert_eq!(page.rows.len(), 1);
        assert!(!page.has_more);
    }

    #[test]
    fn h220_pinned_provider_survives_fork_rewind_and_move_and_epoch_tracks_recycle() {
        let db = Database::open_in_memory().unwrap();
        let store = EventStore::new(&db);
        let id = SessionId::new();
        store
            .create_session_configured(
                id,
                None,
                SessionMode::Normal,
                Some("old"),
                Some(SessionExecutionConfig::new(ExecutionOverrides {
                    provider: Some("deepseek".into()),
                    ..Default::default()
                })),
                Timestamp::from_millis(1),
            )
            .unwrap();
        store
            .append(
                id,
                EventPayload::Note {
                    text: "synthetic".into(),
                },
                Timestamp::from_millis(2),
            )
            .unwrap();
        let child = SessionId::new();
        store
            .fork_session(id, child, 0, Timestamp::from_millis(3))
            .unwrap();
        store.truncate_after(id, 0).unwrap();
        store.move_project_sessions("old", "new").unwrap();
        let filter = SessionCatalogFilter {
            provider: Some("deepseek".into()),
            cwd: Some("new".into()),
            ..Default::default()
        };
        let page = store.list_session_page(&filter, None, 100, 4096).unwrap();
        assert_eq!(page.rows.len(), 2);
        let before = page.end;
        db.with_conn(|c| {
            c.execute_batch("BEGIN;UPDATE sessions SET title='rolled back';ROLLBACK;")
                .map_err(map_sqlite)
        })
        .unwrap();
        assert!(store
            .list_session_page(&filter, Some(&before), 100, 4096)
            .is_ok());
        store
            .trash_session(child, Timestamp::from_millis(7))
            .unwrap();
        assert!(store
            .list_session_page(&filter, Some(&before), 100, 4096)
            .unwrap_err()
            .to_string()
            .contains("stale"));
    }
}
