//! Schedule origins are evidence in the existing admission transaction/ledger.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduledRunOrigin {
    pub version: u32,
    pub occurrence_id: String,
    pub task_id: String,
    pub schedule_anchor_ms: i64,
    pub scheduled_for: String,
    pub accepted_at_ms: i64,
}

impl ScheduledRunOrigin {
    pub(super) fn validate(&self, run_id: &str) -> Result<()> {
        if self.version != 1
            || self.occurrence_id != run_id
            || self.task_id.is_empty()
            || self.scheduled_for.is_empty()
            || self.accepted_at_ms <= self.schedule_anchor_ms
        {
            return Err(CoreError::invalid("invalid scheduled run origin"));
        }
        Ok(())
    }
}

impl RunStore<'_> {
    /// Missing run means no admission. An existing run without matching,
    /// versioned evidence is an error, never permission to redispatch.
    pub fn scheduled_origin(&self, run_id: &str) -> Result<Option<ScheduledRunOrigin>> {
        self.db.with_conn(|conn| {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
                    [run_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite)?;
            if !exists {
                return Ok(None);
            }
            let mut query = conn
                .prepare(
                    "SELECT data FROM run_events WHERE run_id=?1 AND event_type='schedule.accepted'
                 ORDER BY sequence LIMIT 2",
                )
                .map_err(map_sqlite)?;
            let records = query
                .query_map([run_id], |row| row.get::<_, String>(0))
                .map_err(map_sqlite)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(map_sqlite)?;
            if records.len() != 1 {
                return Err(CoreError::Persistence(
                    "scheduled run admission evidence missing or ambiguous; do not redispatch"
                        .into(),
                ));
            }
            let origin: ScheduledRunOrigin =
                serde_json::from_str(&records[0]).map_err(|error| {
                    CoreError::Persistence(format!(
                        "invalid scheduled run admission evidence; do not redispatch: {error}"
                    ))
                })?;
            origin.validate(run_id)?;
            Ok(Some(origin))
        })
    }
}
