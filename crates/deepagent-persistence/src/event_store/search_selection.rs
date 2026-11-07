//! Caller-owned visibility projected into the existing search/health queries.
use super::*;
use std::collections::HashSet;

/// One search's immutable visibility snapshot, supplied by its existing owner.
/// This is not an authorization policy or a stored second session index.
/// `None` leaves that dimension unrestricted; an empty project set permits
/// only sessions without a project. Supplying a filter also excludes orphan
/// health records that have no owning session metadata.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionSearchFilter<'a> {
    pub excluded_sessions: Option<&'a HashSet<String>>,
    pub allowed_projects: Option<&'a HashSet<String>>,
}

pub(super) struct SearchSelection<'a> {
    project: Option<&'a str>,
    no_project: bool,
    require_owner: bool,
    excluded: Option<String>,
    projects: Option<String>,
}

impl<'a> SearchSelection<'a> {
    // JSON1 is bundled with the existing SQLite dependency. Sets are bound
    // data, never interpolated SQL or one variable per session/project.
    pub(super) const PREDICATE: &'static str = "
        (:ss_project IS NULL OR s.project=:ss_project)
        AND (:ss_no_project=0 OR (s.id IS NOT NULL AND s.project IS NULL))
        AND (:ss_owner=0 OR s.id IS NOT NULL)
        AND (:ss_excluded IS NULL OR s.id NOT IN (SELECT value FROM json_each(:ss_excluded)))
        AND (:ss_projects IS NULL OR s.project IS NULL OR s.project IN (SELECT value FROM json_each(:ss_projects)))";

    pub(super) fn new(
        scope: SessionSearchScope<'a>,
        filter: Option<SessionSearchFilter<'_>>,
    ) -> Result<Self> {
        let (project, no_project) = match scope {
            SessionSearchScope::All => (None, false),
            SessionSearchScope::Project(project) => (Some(project), false),
            SessionSearchScope::NoProject => (None, true),
        };
        Ok(Self {
            project,
            no_project,
            require_owner: filter.is_some(),
            excluded: filter
                .and_then(|f| f.excluded_sessions)
                .map(serde_json::to_string)
                .transpose()?,
            projects: filter
                .and_then(|f| f.allowed_projects)
                .map(serde_json::to_string)
                .transpose()?,
        })
    }

    pub(super) fn bindings(&self) -> Vec<(&'static str, &dyn rusqlite::ToSql)> {
        vec![
            (":ss_project", &self.project),
            (":ss_no_project", &self.no_project),
            (":ss_owner", &self.require_owner),
            (":ss_excluded", &self.excluded),
            (":ss_projects", &self.projects),
        ]
    }
}

pub(super) type SearchCandidate = (SessionRecord, Sequence, Option<Role>, Timestamp);

impl EventStore<'_> {
    pub(super) fn search_candidates(
        &self,
        query: &str,
        limit: i64,
        selection: &SearchSelection<'_>,
        control: &SessionSearchControl<'_>,
    ) -> Result<Vec<SearchCandidate>> {
        let char_count = query.chars().count();
        let is_cjk_bigram = char_count == 2 && query.chars().all(is_cjk);
        self.db.with_maintenance_conn(|c| {
            let (source, predicate, order) = if is_cjk_bigram {
                (
                    "FROM session_search_cjk_bigrams b
                      JOIN session_search_docs d ON d.id = b.doc_id
                      JOIN sessions s ON s.id = d.session_id",
                    "b.gram = :term",
                    "d.timestamp DESC",
                )
            } else if char_count >= 3 {
                (
                    "FROM session_search_fts f
                      JOIN session_search_docs d ON d.id = CAST(f.doc_id AS INTEGER)
                      JOIN sessions s ON s.id = d.session_id",
                    "session_search_fts MATCH :term",
                    "bm25(session_search_fts), d.timestamp DESC",
                )
            } else {
                (
                    "FROM session_search_fts f
                      JOIN session_search_docs d ON d.id = CAST(f.doc_id AS INTEGER)
                      JOIN sessions s ON s.id = d.session_id",
                    "f.content LIKE :term ESCAPE '\\'",
                    "d.timestamp DESC",
                )
            };
            let sql = format!(
                "SELECT d.session_id, d.sequence, d.role, d.timestamp
                    {source} WHERE {predicate} AND {selection}
                    AND NOT EXISTS (SELECT 1 FROM documents h
                        WHERE h.collection='session_file_health_v1' AND h.id=d.session_id)
                    ORDER BY {order}, d.session_id, d.sequence LIMIT :limit",
                selection = SearchSelection::PREDICATE
            );
            let search_term = if is_cjk_bigram {
                query.to_owned()
            } else if char_count >= 3 {
                format!("\"{}\"", query.replace('"', "\"\""))
            } else {
                format!("%{}%", escape_like(query))
            };
            let mut stmt = c.prepare(&sql).map_err(map_sqlite)?;
            let mut bindings = selection.bindings();
            bindings.extend([
                (":term", &search_term as &dyn rusqlite::ToSql),
                (":limit", &limit),
            ]);
            let rows = stmt
                .query_map(bindings.as_slice(), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .map_err(map_sqlite)?;
            let mut candidates = Vec::new();
            for row in rows {
                control.check_cancelled()?;
                let (id, sequence, role, timestamp) = row.map_err(map_sqlite)?;
                let id = id
                    .parse::<SessionId>()
                    .map_err(|error| CoreError::Persistence(error.to_string()))?;
                // Keep metadata in this same SQLite read snapshot. A later
                // move must not relabel a hit selected from another scope.
                let Some(session) = Self::read_session_record(c, id, None)? else {
                    continue;
                };
                candidates.push((
                    session,
                    sequence as Sequence,
                    role.as_deref().and_then(parse_role),
                    Timestamp::from_millis(timestamp),
                ));
            }
            Ok(candidates)
        })
    }
}
