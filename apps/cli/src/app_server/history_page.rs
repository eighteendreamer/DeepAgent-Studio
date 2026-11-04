//! One bounded hydration over the existing authoritative file and repositories.
//! The cursor contains only a fixed-size traversal position, never all run IDs.

use super::{history_cursor, RpcResponse, ThreadReadRequest, CONTROL_PROJECTION_VERSION};
use deepagent_app_core::run_graph::{self, GraphHistoryCursor};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::id::SessionId;
use deepagent_persistence::artifact_store::ArtifactStore;
use deepagent_persistence::bounded_read::{
    json_len, HistoryCursor, HistoryPage, HistoryReadBudget, HistorySnapshot,
};
use deepagent_persistence::event_store::{EventStore, SessionPageBudget, SessionPageCursor};
use deepagent_persistence::run_control::RunControlStore;
use deepagent_persistence::run_store::{RunRecord, RunStore};
use deepagent_persistence::Database;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Filters {
    session: Option<u64>,
    run: Option<u64>,
    runs: BTreeMap<String, u64>,
}

impl Filters {
    fn from_request(params: &ThreadReadRequest) -> Self {
        Self {
            session: params.session_after_sequence.or(params.after_sequence),
            run: params.run_after_sequence.or(params.after_sequence),
            runs: params.run_after_sequences.clone().unwrap_or_default(),
        }
    }
    fn run_after(&self, id: &str) -> Option<u64> {
        self.runs.get(id).copied().or(self.run)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Section {
    Events,
    Actions,
    Approvals,
    Artifacts,
    Graph,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunPosition {
    id: String,
    section: Section,
    row: Option<HistoryCursor>,
    graph: Option<GraphHistoryCursor>,
}

impl RunPosition {
    fn advance(&mut self) {
        self.row = None;
        self.section = match self.section {
            Section::Events => Section::Actions,
            Section::Actions => Section::Approvals,
            Section::Approvals => Section::Artifacts,
            Section::Artifacts => Section::Graph,
            Section::Graph | Section::Done => Section::Done,
        };
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    snapshot: HistorySnapshot,
    filters: Filters,
    session: Option<SessionPageCursor>,
    session_done: bool,
    runs: Option<HistoryCursor>,
    runs_done: bool,
    run: Option<RunPosition>,
}

impl Position {
    fn done(&self) -> bool {
        self.session_done && self.runs_done && self.run.is_none()
    }
}

enum Piece {
    Session(Vec<Value>),
    Run(Value),
    Row(&'static str, Value),
}

#[derive(Clone, Copy)]
enum Location {
    Session,
    Runs,
    RunItems(&'static str),
}

impl Location {
    fn array(self, root: &mut Value) -> &mut Vec<Value> {
        match self {
            Self::Session => root["events"].as_array_mut().unwrap(),
            Self::Runs => root["runEvents"].as_array_mut().unwrap(),
            Self::RunItems(field) => {
                let run = root["runEvents"]
                    .as_array_mut()
                    .unwrap()
                    .last_mut()
                    .unwrap();
                if field == "graph" {
                    run["graph"]["nodes"].as_array_mut().unwrap()
                } else {
                    run[field].as_array_mut().unwrap()
                }
            }
        }
    }
}

/// Counts the complete envelope, stream tags, metadata, run wrappers and exact
/// encoded cursor length. A rejected piece is rolled back before its checkpoint
/// advances. No full history is loaded and then truncated.
struct Output {
    response: RpcResponse,
    count: usize,
    page_size: usize,
    max_bytes: usize,
}

impl Output {
    fn offer(&mut self, piece: Piece, after: &Position) -> Result<bool> {
        let count = match &piece {
            Piece::Session(events) => events.len(),
            _ => 1,
        };
        if self.count + count > self.page_size {
            if self.count == 0 {
                return Err(too_large());
            }
            return Ok(false);
        }
        let (location, values) = match piece {
            Piece::Session(events) => (Location::Session, events),
            Piece::Run(run) => (Location::Runs, vec![run]),
            Piece::Row(field, value) => (Location::RunItems(field), vec![value]),
        };
        let root = self.response.result.as_mut().expect("history output");
        let array = location.array(root);
        let previous_len = array.len();
        array.extend(values);
        self.placeholder(after)?;
        if json_len(&self.response, self.max_bytes).is_err() {
            let root = self.response.result.as_mut().unwrap();
            location.array(root).truncate(previous_len);
            if self.count == 0 {
                return Err(too_large());
            }
            return Ok(false);
        }
        self.count += count;
        Ok(true)
    }

    fn placeholder(&mut self, position: &Position) -> Result<()> {
        let result = self.response.result.as_mut().unwrap();
        result["hasMore"] = json!(!position.done());
        result["nextCursor"] = if position.done() {
            Value::Null
        } else {
            json!("x".repeat(history_cursor::encoded_len(position)?))
        };
        Ok(())
    }

    fn finish(mut self, db: &Database, thread: &str, position: &Position) -> Result<RpcResponse> {
        let result = self.response.result.as_mut().unwrap();
        result["hasMore"] = json!(!position.done());
        result["nextCursor"] = if position.done() {
            Value::Null
        } else {
            json!(history_cursor::encode(db, thread, position)?)
        };
        json_len(&self.response, self.max_bytes)?;
        Ok(self.response)
    }
}

pub(super) fn read(
    db: &Arc<Database>,
    id: Value,
    params: ThreadReadRequest,
) -> Result<RpcResponse> {
    let page_size = params.page_size.unwrap_or(500);
    let max_bytes = params.max_bytes.unwrap_or(256 * 1024);
    if !(1..=500).contains(&page_size) || !(4096..=1048576).contains(&max_bytes) {
        return Err(CoreError::invalid(
            "history pageSize must be 1..=500 and maxBytes 4096..=1048576",
        ));
    }
    let session: SessionId = params
        .thread_id
        .parse()
        .map_err(|_| CoreError::not_found("invalid thread id"))?;
    let thread = session.to_string();
    if let Some(filters) = &params.run_after_sequences {
        json_len(filters, 1024).map_err(|_| {
            CoreError::invalid("history per-run filters exceed continuation budget")
        })?;
    }
    let filters = Filters::from_request(&params);
    let mut position = if let Some(token) = params.cursor.as_deref() {
        let position: Position = history_cursor::decode(db, &thread, token)?;
        if (params.after_sequence.is_some()
            || params.session_after_sequence.is_some()
            || params.run_after_sequence.is_some()
            || params.run_after_sequences.is_some())
            && filters != position.filters
        {
            return Err(CoreError::invalid("history continuation filters changed"));
        }
        position
    } else {
        Position {
            snapshot: HistorySnapshot::capture(db)?,
            filters,
            session: None,
            session_done: false,
            runs: None,
            runs_done: false,
            run: None,
        }
    };
    history_cursor::encoded_len(&position)?;
    let initial = position.clone();
    let events = EventStore::new(db);
    let record = events
        .get_session_bounded(session, max_bytes)?
        .ok_or_else(|| CoreError::not_found("thread not found"))?;
    let mut output = Output {
        response: RpcResponse::success(
            id,
            json!({
                "threadId":thread,"title":record.title,"cwd":record.project,
                "controlProjectionVersion":CONTROL_PROJECTION_VERSION,"cursorMode":"stream_scoped",
                "historyPageVersion":1,"events":[],"runEvents":[],"hasMore":true,"nextCursor":null
            }),
        ),
        count: 0,
        page_size,
        max_bytes,
    };
    if read_session_page(db, session, &mut position, &mut output)? {
        read_run_pages(db, &thread, &mut position, &mut output)?;
    }
    if !position.done() && position == initial {
        return Err(too_large());
    }
    output.finish(db, &thread, &position)
}

fn read_session_page(
    db: &Database,
    session: SessionId,
    position: &mut Position,
    output: &mut Output,
) -> Result<bool> {
    let events = EventStore::new(db);
    // Even after session exhaustion this validates its fixed generation/cut;
    // rewinding while the run projections are paged invalidates the cursor.
    let page = events.read_session_page(
        session,
        position.filters.session,
        position.session.clone(),
        SessionPageBudget {
            max_events: output.page_size,
            max_bytes: output.max_bytes,
        },
    )?;
    position.session = Some(page.start_cursor);
    let mut event_iter = page.events.into_iter();
    let mut previous = 0;
    for (end, checkpoint) in page.groups {
        let group = event_iter
            .by_ref()
            .take(end - previous)
            .map(|event| {
                let sequence = event.sequence;
                let mut value = serde_json::to_value(event)?;
                value["stream"] = json!("session");
                value["streamSequence"] = json!(sequence);
                Ok(value)
            })
            .collect::<Result<Vec<_>>>()?;
        previous = end;
        let mut next = position.clone();
        next.session = Some(checkpoint);
        if !output.offer(Piece::Session(group), &next)? {
            return Ok(false);
        }
        *position = next;
    }
    position.session = Some(page.end_cursor);
    position.session_done = page.next_cursor.is_none();
    Ok(position.session_done)
}

fn read_run_pages(
    db: &Arc<Database>,
    thread: &str,
    position: &mut Position,
    output: &mut Output,
) -> Result<()> {
    let budget = HistoryReadBudget {
        max_rows: 1,
        max_bytes: output.max_bytes,
        snapshot: Some(position.snapshot),
    };
    let runs = RunStore::new(db);
    loop {
        if position.run.is_none() {
            if position.runs_done {
                break;
            }
            let page = runs.list_for_session_page(thread, position.runs.as_ref(), budget)?;
            let mut next = position.clone();
            next.runs_done = !page.has_more;
            if let Some(row) = page.rows.into_iter().next() {
                next.runs = Some(row.cursor);
                next.run = Some(RunPosition {
                    id: row.value.id.clone(),
                    section: Section::Events,
                    row: None,
                    graph: None,
                });
                if !output.offer(Piece::Run(run_view(row.value)), &next)? {
                    break;
                }
            }
            *position = next;
            continue;
        }
        let current = position.run.as_ref().unwrap();
        if current.section == Section::Done {
            position.run = None;
            continue;
        }
        let last_id = output.response.result()["runEvents"]
            .as_array()
            .unwrap()
            .last()
            .and_then(|run| run["runId"].as_str());
        if last_id != Some(current.id.as_str()) {
            let run = runs
                .get_bounded(&current.id, output.max_bytes)?
                .ok_or_else(|| CoreError::invalid("history run removed; restart hydration"))?;
            if run.session_id != thread {
                return Err(CoreError::invalid("foreign history run"));
            }
            if !output.offer(Piece::Run(run_view(run)), position)? {
                break;
            }
        }
        let mut next = position.clone();
        let run = next.run.as_mut().unwrap();
        let controls = RunControlStore::new(db);
        let piece = match run.section {
            Section::Events => sql_piece(
                runs.events_page(
                    &run.id,
                    position.filters.run_after(&run.id),
                    run.row.as_ref(),
                    budget,
                )?,
                run,
                "events",
            )?,
            Section::Actions => sql_piece(
                controls.list_actions_page(&run.id, run.row.as_ref(), budget)?,
                run,
                "actions",
            )?,
            Section::Approvals => sql_piece(
                controls.list_approvals_page(&run.id, run.row.as_ref(), budget)?,
                run,
                "approvals",
            )?,
            Section::Artifacts => sql_piece(
                ArtifactStore::new(db).list_for_run_page(&run.id, run.row.as_ref(), budget)?,
                run,
                "artifacts",
            )?,
            Section::Graph => {
                let page = run_graph::load_page(db, &run.id, run.graph.as_ref(), budget)?;
                if !page.has_more {
                    run.advance();
                }
                if let Some(row) = page.rows.into_iter().next() {
                    run.graph = Some(row.cursor);
                    Some(Piece::Row("graph", serde_json::to_value(row.value)?))
                } else {
                    None
                }
            }
            Section::Done => unreachable!("handled before reading"),
        };
        if let Some(piece) = piece {
            if !output.offer(piece, &next)? {
                break;
            }
        }
        *position = next;
    }
    Ok(())
}

fn sql_piece<T: Serialize>(
    page: HistoryPage<T>,
    run: &mut RunPosition,
    field: &'static str,
) -> Result<Option<Piece>> {
    let has_more = page.has_more;
    let Some(row) = page.rows.into_iter().next() else {
        run.advance();
        return Ok(None);
    };
    run.row = Some(row.cursor);
    let mut value = serde_json::to_value(row.value)?;
    if field == "events" {
        value["stream"] = json!("run");
        value["runId"] = json!(run.id);
        value["streamSequence"] = value["sequence"].clone();
    }
    if !has_more {
        run.advance();
    }
    Ok(Some(Piece::Row(field, value)))
}

fn run_view(run: RunRecord) -> Value {
    json!({"runId":run.id,"state":run.state,"terminalKind":run.terminal_kind,
        "events":[],"actions":[],"approvals":[],"artifacts":[],
        "graph":{"rootRunId":run.id,"nodes":[]}})
}

fn too_large() -> CoreError {
    CoreError::invalid("history_item_too_large: complete history item and envelope must fit pageSize/maxBytes; increase the budget")
}
