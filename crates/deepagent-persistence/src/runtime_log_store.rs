//! Dedicated SQLite-backed runtime diagnostics log.
//!
//! This database is intentionally separate from the session event store. The
//! event store is the replayable product state; this log is a detailed,
//! append-only diagnostic trail for answering "where did the run get stuck?"

use std::path::Path;
use std::sync::mpsc;
use std::time::{SystemTime, UNIX_EPOCH};

use deepagent_core::error::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::map_sqlite;

/// A new runtime log entry. All entries use the same envelope and put
/// event-specific fields in `data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRuntimeLogEntry {
    /// Severity label (`trace`, `debug`, `info`, `warn`, `error`).
    pub level: String,
    /// High-level subsystem (`chat`, `runtime`, `model`, `tool`, `hook`,
    /// `permission`, `cancel`, etc.).
    pub category: String,
    /// Stable event name inside the category.
    pub event: String,
    /// Correlates all entries from one user-submitted run.
    pub run_id: Option<String>,
    /// Session id when known.
    pub session_id: Option<String>,
    /// Task id when known.
    pub task_id: Option<String>,
    /// Tool call id / hook id / approval id / request id.
    pub correlation_id: Option<String>,
    /// Producer label, usually the crate/module.
    pub source: Option<String>,
    /// Short human-readable message.
    pub message: Option<String>,
    /// Full structured payload for later investigation.
    pub data: serde_json::Value,
}

impl NewRuntimeLogEntry {
    /// Create an info-level entry.
    pub fn info(category: impl Into<String>, event: impl Into<String>) -> Self {
        Self {
            level: "info".into(),
            category: category.into(),
            event: event.into(),
            run_id: None,
            session_id: None,
            task_id: None,
            correlation_id: None,
            source: None,
            message: None,
            data: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Attach a run id.
    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// Attach a session id.
    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Attach a task id.
    pub fn with_task_id(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    /// Attach a correlation id.
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    /// Attach a source label.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Attach a short message.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Attach the structured payload.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = data;
        self
    }
}

/// A persisted runtime log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeLogEntry {
    /// Monotonic row id.
    pub id: i64,
    /// Unix timestamp in milliseconds.
    pub ts_ms: i64,
    pub level: String,
    pub category: String,
    pub event: String,
    pub run_id: Option<String>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub correlation_id: Option<String>,
    pub source: Option<String>,
    pub message: Option<String>,
    pub data: serde_json::Value,
}

/// Dedicated runtime log database handle.
pub struct RuntimeLogStore {
    commands: mpsc::Sender<RuntimeLogCommand>,
}

enum RuntimeLogCommand {
    Append(NewRuntimeLogEntry, mpsc::Sender<Result<i64>>),
    Recent {
        session_id: Option<String>,
        limit: i64,
        reply: mpsc::Sender<Result<Vec<RuntimeLogEntry>>>,
    },
    Get {
        id: i64,
        reply: mpsc::Sender<Result<Option<RuntimeLogEntry>>>,
    },
}

impl RuntimeLogStore {
    /// Open the runtime log database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreError::Persistence(format!(
                    "create runtime log directory '{}': {e}",
                    parent.display()
                ))
            })?;
        }
        let conn = Connection::open(path).map_err(map_sqlite)?;
        Self::from_connection(conn)
    }

    /// Open an isolated in-memory runtime log database.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(map_sqlite)?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        configure_pragmas(&conn)?;
        init_schema(&conn)?;
        let (commands, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("deepagent-runtime-log-writer".into())
            .spawn(move || runtime_log_writer(conn, receiver))
            .map_err(|error| {
                CoreError::Persistence(format!("start runtime log writer thread: {error}"))
            })?;
        Ok(Self { commands })
    }

    /// Append one entry and return its row id.
    pub fn append(&self, entry: NewRuntimeLogEntry) -> Result<i64> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(RuntimeLogCommand::Append(entry, reply))
            .map_err(|_| CoreError::Persistence("runtime log writer stopped".into()))?;
        result
            .recv()
            .map_err(|_| CoreError::Persistence("runtime log writer stopped".into()))?
    }

    /// Fetch recent entries, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<RuntimeLogEntry>> {
        let limit = limit.clamp(1, 1000) as i64;
        self.query_recent(None, limit)
    }

    /// Fetch recent entries for a session, newest first.
    pub fn recent_for_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<RuntimeLogEntry>> {
        let limit = limit.clamp(1, 1000) as i64;
        self.query_recent(Some(session_id), limit)
    }

    /// Return one entry by id.
    pub fn get(&self, id: i64) -> Result<Option<RuntimeLogEntry>> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(RuntimeLogCommand::Get { id, reply })
            .map_err(|_| CoreError::Persistence("runtime log writer stopped".into()))?;
        result
            .recv()
            .map_err(|_| CoreError::Persistence("runtime log writer stopped".into()))?
    }

    fn query_recent(&self, session_id: Option<&str>, limit: i64) -> Result<Vec<RuntimeLogEntry>> {
        let (reply, result) = mpsc::channel();
        self.commands
            .send(RuntimeLogCommand::Recent {
                session_id: session_id.map(ToOwned::to_owned),
                limit,
                reply,
            })
            .map_err(|_| CoreError::Persistence("runtime log writer stopped".into()))?;
        result
            .recv()
            .map_err(|_| CoreError::Persistence("runtime log writer stopped".into()))?
    }
}

fn runtime_log_writer(conn: Connection, receiver: mpsc::Receiver<RuntimeLogCommand>) {
    for command in receiver {
        match command {
            RuntimeLogCommand::Append(entry, reply) => {
                let _ = reply.send(append_on_conn(&conn, entry));
            }
            RuntimeLogCommand::Recent {
                session_id,
                limit,
                reply,
            } => {
                let _ = reply.send(query_recent_on_conn(&conn, session_id.as_deref(), limit));
            }
            RuntimeLogCommand::Get { id, reply } => {
                let _ = reply.send(get_on_conn(&conn, id));
            }
        }
    }
}

fn append_on_conn(conn: &Connection, entry: NewRuntimeLogEntry) -> Result<i64> {
    let ts_ms = now_ms();
    let data_json = serde_json::to_string(&entry.data)
        .map_err(|e| CoreError::Persistence(format!("serialize runtime log data: {e}")))?;
    conn.execute(
        "INSERT INTO runtime_logs (
            ts_ms, level, category, event, run_id, session_id, task_id,
            correlation_id, source, message, data_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            ts_ms,
            entry.level,
            entry.category,
            entry.event,
            entry.run_id,
            entry.session_id,
            entry.task_id,
            entry.correlation_id,
            entry.source,
            entry.message,
            data_json,
        ],
    )
    .map_err(map_sqlite)?;
    Ok(conn.last_insert_rowid())
}

fn get_on_conn(conn: &Connection, id: i64) -> Result<Option<RuntimeLogEntry>> {
    conn.query_row(
        "SELECT id, ts_ms, level, category, event, run_id, session_id, task_id,
                correlation_id, source, message, data_json
         FROM runtime_logs
         WHERE id = ?1",
        params![id],
        row_to_entry,
    )
    .optional()
    .map_err(map_sqlite)
}

fn query_recent_on_conn(
    conn: &Connection,
    session_id: Option<&str>,
    limit: i64,
) -> Result<Vec<RuntimeLogEntry>> {
    let mut out = Vec::new();
    if let Some(session_id) = session_id {
        let mut stmt = conn
            .prepare(
                "SELECT id, ts_ms, level, category, event, run_id, session_id, task_id,
                        correlation_id, source, message, data_json
                 FROM runtime_logs
                 WHERE session_id = ?1
                 ORDER BY id DESC
                 LIMIT ?2",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![session_id, limit], row_to_entry)
            .map_err(map_sqlite)?;
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT id, ts_ms, level, category, event, run_id, session_id, task_id,
                        correlation_id, source, message, data_json
                 FROM runtime_logs
                 ORDER BY id DESC
                 LIMIT ?1",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![limit], row_to_entry)
            .map_err(map_sqlite)?;
        for row in rows {
            out.push(row.map_err(map_sqlite)?);
        }
    }
    Ok(out)
}

fn configure_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )
    .map_err(map_sqlite)
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_ms INTEGER NOT NULL,
            level TEXT NOT NULL,
            category TEXT NOT NULL,
            event TEXT NOT NULL,
            run_id TEXT,
            session_id TEXT,
            task_id TEXT,
            correlation_id TEXT,
            source TEXT,
            message TEXT,
            data_json TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_runtime_logs_session_id_id
            ON runtime_logs(session_id, id);
        CREATE INDEX IF NOT EXISTS idx_runtime_logs_run_id_id
            ON runtime_logs(run_id, id);
        CREATE INDEX IF NOT EXISTS idx_runtime_logs_category_event_id
            ON runtime_logs(category, event, id);
        CREATE INDEX IF NOT EXISTS idx_runtime_logs_ts_ms
            ON runtime_logs(ts_ms);",
    )
    .map_err(map_sqlite)
}

fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeLogEntry> {
    let data_json: String = row.get(11)?;
    let data = serde_json::from_str(&data_json).unwrap_or_else(|_| {
        serde_json::json!({
            "parse_error": "invalid data_json",
            "raw": data_json,
        })
    });
    Ok(RuntimeLogEntry {
        id: row.get(0)?,
        ts_ms: row.get(1)?,
        level: row.get(2)?,
        category: row.get(3)?,
        event: row.get(4)?,
        run_id: row.get(5)?,
        session_id: row.get(6)?,
        task_id: row.get(7)?,
        correlation_id: row.get(8)?,
        source: row.get(9)?,
        message: row.get(10)?,
        data,
    })
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_recent_entries() {
        let store = RuntimeLogStore::open_in_memory().unwrap();
        let id = store
            .append(
                NewRuntimeLogEntry::info("chat", "run_requested")
                    .with_run_id("run_1")
                    .with_session_id("ses_1")
                    .with_message("started")
                    .with_data(serde_json::json!({"step": 1})),
            )
            .unwrap();

        let one = store.get(id).unwrap().unwrap();
        assert_eq!(one.category, "chat");
        assert_eq!(one.event, "run_requested");
        assert_eq!(one.data["step"], 1);

        let recent = store.recent_for_session("ses_1", 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].run_id.as_deref(), Some("run_1"));
    }

    #[test]
    fn concurrent_appends_are_serialized_by_writer() {
        let store = std::sync::Arc::new(RuntimeLogStore::open_in_memory().unwrap());
        let mut handles = Vec::new();
        for index in 0..16 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                store
                    .append(
                        NewRuntimeLogEntry::info("runtime", "concurrent_append")
                            .with_session_id("ses_parallel")
                            .with_data(serde_json::json!({ "index": index })),
                    )
                    .unwrap()
            }));
        }

        let mut ids = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 16);

        let recent = store.recent_for_session("ses_parallel", 100).unwrap();
        assert_eq!(recent.len(), 16);
    }
}
