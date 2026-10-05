//! Session memory: a rolling on-disk summary of the CURRENT conversation.
//!
//! This is the layer Claude Code keeps in a per-session note: a restricted run
//! reads what happened since its last pass and *edits* the note, so the summary
//! grows incrementally instead of being re-derived from the whole transcript. It
//! pays for compaction: when the context gets too big, the note (not a fresh
//! model call) becomes the text that replaces the compacted prefix.
//!
//! Two properties are load-bearing and both come from the reference
//! implementation:
//! - **The token delta is a hard prerequisite.** Extraction is gated on the
//!   context growing by enough since the last pass; the tool-call count only
//!   picks *whether* the pass may happen inside that window and never overrides
//!   the token gate.
//! - **Two different cursors.** `tool_cursor_sequence` advances whenever a pass
//!   is scheduled and is only used for counting tool calls.
//!   `last_summarized_sequence` — the cut point for compaction — advances ONLY
//!   when the newest assistant turn made no tool call; cutting right after a
//!   tool call would orphan a `tool_use` from its result.
//!
//! The extraction run is not a second execution chain: it is a `RuntimeEngine`
//! run over a registry holding nothing but the pinned note tools, following the
//! existing precedent in [`crate::hook_runtime`]. Cost is returned to the caller
//! so it lands in the ordinary cost ledger rather than a private one.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use deepagent_builtins::{note_edit_tools, WorkspaceRoot};
use deepagent_core::clock::{Clock, SystemClock, Timestamp};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::{Event, EventPayload, Sequence};
use deepagent_core::id::SessionId;
use deepagent_core::message::Message;
use deepagent_models::{ModelClient, ToolSchema};
use deepagent_persistence::Database;
use deepagent_runtime::agent::RunUsage;
use deepagent_runtime::{Agent, ModelAgent, RunOutcome, RuntimeConfig, RuntimeEngine};
use deepagent_session::Session;
use deepagent_tools::permission::PermissionSet;
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// Collection holding the note's cursor state; the note text itself is a file
/// next to the session's other artifacts.
const SESSION_MEMORY_COLLECTION: &str = "session_memory";

/// Template written when a note is first created.
const NOTE_TEMPLATE: &str = "# Session notes\n\n(nothing recorded yet)\n";

/// Persisted progress of the rolling summary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMemoryState {
    /// Current note contents.
    pub summary: String,
    /// Event sequence the note covers: compaction may replace everything up to
    /// and including it. `None` until a pass has finished at a safe point.
    pub last_summarized_sequence: Option<Sequence>,
    /// Cursor for counting tool calls between passes.
    pub tool_cursor_sequence: Option<Sequence>,
    /// Total context tokens when the last pass started.
    pub tokens_at_last_extraction: u64,
    /// Whether a first pass has ever run.
    pub initialized: bool,
    /// Last update (Unix ms).
    pub updated_at: i64,
}

/// Thresholds for scheduling a pass. Defaults follow the reference values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionGate {
    /// Context size before the note is created at all.
    pub init_tokens: u64,
    /// Context growth since the last pass required for another one.
    pub min_delta_tokens: u64,
    /// Tool calls since the cursor that make a pass worthwhile.
    pub min_tool_calls: u64,
}

impl Default for ExtractionGate {
    fn default() -> Self {
        Self {
            init_tokens: 10_000,
            min_delta_tokens: 5_000,
            min_tool_calls: 3,
        }
    }
}

/// Why a pass was (or was not) taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// First note for this session.
    Initialize,
    /// Grow the existing note.
    Update,
    /// Not yet.
    Wait,
}

impl GateDecision {
    /// Whether a pass should run.
    pub fn should_extract(&self) -> bool {
        matches!(self, Self::Initialize | Self::Update)
    }
}

impl ExtractionGate {
    /// Decide whether to extract.
    ///
    /// Mirrors the reference rule `(tokens && tool_calls) || (tokens &&
    /// last turn made no tool call)`, with the token condition mandatory in
    /// both branches. Before the note exists the gate is only a size check, so
    /// a long session cannot stay note-less forever.
    pub fn decide(
        &self,
        state: Option<&SessionMemoryState>,
        total_tokens: u64,
        tool_calls_since_cursor: u64,
        last_turn_has_tool_call: bool,
    ) -> GateDecision {
        let Some(state) = state.filter(|state| state.initialized) else {
            return if total_tokens >= self.init_tokens {
                GateDecision::Initialize
            } else {
                GateDecision::Wait
            };
        };
        let grew_enough =
            total_tokens.saturating_sub(state.tokens_at_last_extraction) >= self.min_delta_tokens;
        if !grew_enough {
            return GateDecision::Wait;
        }
        if tool_calls_since_cursor >= self.min_tool_calls || !last_turn_has_tool_call {
            GateDecision::Update
        } else {
            GateDecision::Wait
        }
    }
}

/// Advance the cursors after a pass. The cut point only moves at a safe
/// boundary; the tool cursor always moves so the next pass counts from here.
pub fn advance_cursors(
    mut state: SessionMemoryState,
    candidate: Sequence,
    safe_to_cut: bool,
    total_tokens: u64,
    now: Timestamp,
) -> SessionMemoryState {
    state.tool_cursor_sequence = Some(candidate);
    if safe_to_cut {
        state.last_summarized_sequence = Some(candidate);
    }
    state.tokens_at_last_extraction = total_tokens;
    state.initialized = true;
    state.updated_at = now.as_millis();
    state
}

/// Tool wrapper that lets a model tool act on exactly one file.
///
/// Registry construction already restricts the extraction pass to the pinned
/// tools; this is the second layer, because the argument is model-supplied and
/// the note lives outside the conversation workspace. Anything other than the
/// exact resolved path is refused without invoking the inner tool.
pub struct PinnedFileTool {
    inner: Arc<dyn Tool>,
    allowed_path: PathBuf,
}

impl PinnedFileTool {
    /// Wrap `inner`, permitting only `allowed_path`.
    pub fn new(inner: Arc<dyn Tool>, allowed_path: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            allowed_path: allowed_path.into(),
        }
    }

    /// Resolve the `path` argument to an absolute path the way the guarded file
    /// tools do (lexically, so it also works for not-yet-created files), then
    /// drop `.` segments so `./note.md` and `note.md` compare equal. `..` is
    /// left in place: it can then only ever mismatch and be refused.
    fn requested(&self, args: &serde_json::Value) -> Option<PathBuf> {
        let raw = args.get("path").and_then(serde_json::Value::as_str)?;
        let candidate = Path::new(raw);
        let absolute = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.allowed_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(candidate)
        };
        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            if component != Component::CurDir {
                normalized.push(component.as_os_str());
            }
        }
        Some(normalized)
    }
}

#[async_trait::async_trait]
impl Tool for PinnedFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        self.inner.descriptor()
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let requested = self
            .requested(&args)
            .ok_or_else(|| CoreError::invalid("file tool called without a 'path' argument"))?;
        if requested != self.allowed_path {
            return Ok(ToolOutput::failure(format!(
                "only {} may be accessed by this run",
                self.allowed_path.display()
            )));
        }
        self.inner.invoke(args).await
    }
}

/// Outcome of one scheduling attempt, returned to the caller so the token cost
/// goes through the ordinary cost ledger instead of a private one.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractionOutcome {
    /// Whether a pass ran.
    pub extracted: bool,
    /// Why not, when it did not.
    pub reason: &'static str,
    /// What the pass reported, including DeepSeek cache hit/miss and reasoning
    /// tokens. `None` when the pass did not run or the provider reported nothing.
    pub usage: Option<RunUsage>,
}

impl ExtractionOutcome {
    fn skipped(reason: &'static str) -> Self {
        Self {
            extracted: false,
            reason,
            usage: None,
        }
    }
}

/// Reads and writes the note and its cursors.
pub struct SessionMemoryStore {
    db: Arc<Database>,
}

impl SessionMemoryStore {
    /// Build over the shared database.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Where this session's note lives.
    pub fn memory_path(&self, session_id: SessionId) -> PathBuf {
        self.db.session_memory_path(session_id)
    }

    /// Load the persisted cursors, if any.
    pub fn load(&self, session_id: SessionId) -> Result<Option<SessionMemoryState>> {
        let key = session_id.to_string();
        self.db.with_conn(|conn| {
            let body: Option<String> = conn
                .query_row(
                    "SELECT body FROM documents WHERE collection = ?1 AND id = ?2",
                    rusqlite::params![SESSION_MEMORY_COLLECTION, key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| CoreError::Persistence(error.to_string()))?;
            let Some(body) = body else { return Ok(None) };
            let state: SessionMemoryState = serde_json::from_str(&body)
                .map_err(|error| CoreError::Serialization(format!("session memory: {error}")))?;
            Ok(Some(state))
        })
    }

    /// Persist the cursors (the caller serialises passes per session).
    pub fn save(&self, session_id: SessionId, state: &SessionMemoryState) -> Result<()> {
        let body = serde_json::to_string(state)
            .map_err(|error| CoreError::Serialization(format!("session memory: {error}")))?;
        let key = session_id.to_string();
        let now = SystemClock.now().as_millis();
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO documents (id, collection, body, embedding, created_at, updated_at)
                 VALUES (?2, ?1, ?3, NULL, ?4, ?4)
                 ON CONFLICT(collection, id) DO UPDATE SET body = excluded.body,
                                                            updated_at = excluded.updated_at",
                rusqlite::params![SESSION_MEMORY_COLLECTION, key, body, now],
            )
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
            Ok(())
        })
    }
}

/// Create the note from [`NOTE_TEMPLATE`] if it is missing, otherwise return it
/// unchanged. Never clobbers an existing note.
pub fn ensure_note(path: &Path) -> Result<String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::Persistence(format!("create session dir: {error}")))?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(_) => {
            std::fs::write(path, NOTE_TEMPLATE)
                .map_err(|error| CoreError::Persistence(format!("write session note: {error}")))?;
            Ok(NOTE_TEMPLATE.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            std::fs::read_to_string(path)
                .map_err(|error| CoreError::Persistence(format!("read session note: {error}")))
        }
        Err(error) => Err(CoreError::Persistence(format!(
            "create session note: {error}"
        ))),
    }
}

/// The prompt handed to the extraction pass: the current note plus the
/// transcript to fold into it.
pub fn build_extraction_prompt(note_path: &Path, note: &str, transcript: &str) -> String {
    format!(
        "You maintain the session note for an ongoing agent conversation.\n\
         Read the note file, then edit it so it records anything from the recent \
         transcript worth keeping: the goal, decisions taken, facts learned, files \
         touched, failed approaches not to repeat, and open questions.\n\
         Keep it terse and factual. Do not invent anything not present in the \
         transcript, and never copy credentials into it.\n\n\
         Note file (the only path you may read or edit): {}\n\n\
         Current note:\n{}\n\n\
         Recent transcript:\n{}\n\n\
         Finish by editing the note file in place.",
        note_path.display(),
        note,
        transcript
    )
}

/// Whether the newest tool call in the log still has no result.
///
/// This is what makes a cut point unsafe: compaction that replaces everything
/// up to the newest event would leave a `tool_use` whose `tool_result` was
/// dropped. A completed run answers this with `false`; a run cancelled between
/// a call and its result answers `true`.
pub fn pending_tool_call_at_tail(events: &[Event]) -> bool {
    let completed: std::collections::HashSet<String> = events
        .iter()
        .filter_map(|event| match &event.payload {
            EventPayload::ToolCallCompleted { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    events
        .iter()
        .rev()
        .find_map(|event| match &event.payload {
            EventPayload::ToolCallRequested { call } => Some(!completed.contains(&call.id)),
            _ => None,
        })
        .unwrap_or(false)
}

/// Host-scoped scheduler: at most one extraction pass per session at a time,
/// plus a way to wait for the passes already in flight.
///
/// Two passes for the same session would both read the same note and the later
/// save would silently discard the earlier one, so a second request is dropped
/// rather than queued — the next due turn re-runs it with a bigger window.
///
/// The wait matters because the pass is a background task: a one-shot process
/// (the headless CLI) would otherwise exit and take the unfinished note with it.
#[derive(Clone, Default)]
pub struct SessionMemoryPasses {
    book: Arc<PassBook>,
}

/// The shared state behind [`SessionMemoryPasses`]: claimed slots and the
/// wakeup every finished pass fires.
#[derive(Default)]
struct PassBook {
    slots: std::sync::Mutex<std::collections::HashSet<SessionId>>,
    idle: tokio::sync::Notify,
}

/// Releases the session slot when the pass finishes (including on panic).
pub struct SessionMemoryPassGuard {
    session_id: SessionId,
    book: Arc<PassBook>,
}

impl Drop for SessionMemoryPassGuard {
    fn drop(&mut self) {
        self.book
            .slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&self.session_id);
        // `notify_waiters` only wakes already-registered waiters, which is why
        // `wait_idle` parks before it reads the set.
        self.book.idle.notify_waiters();
    }
}

impl SessionMemoryPasses {
    /// Claim the session's pass slot, or `None` when a pass is already running.
    pub fn try_begin(&self, session_id: SessionId) -> Option<SessionMemoryPassGuard> {
        let mut set = self
            .book
            .slots
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !set.insert(session_id) {
            return None;
        }
        Some(SessionMemoryPassGuard {
            session_id,
            book: self.book.clone(),
        })
    }

    /// Wait until no extraction pass is in flight, up to `timeout`.
    ///
    /// Returns `true` when the passes drained and `false` when the timeout hit,
    /// which is the honest answer for a pass whose model call is still hanging:
    /// the caller gets to decide whether to keep waiting, not a silent success.
    pub async fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            // Park first, then look: a pass finishing between the two would
            // otherwise be missed and this would wait out the whole timeout.
            let notified = self.book.idle.notified();
            tokio::pin!(notified);
            if self
                .book
                .slots
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .is_empty()
            {
                return true;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            if tokio::time::timeout(remaining, notified.as_mut())
                .await
                .is_err()
            {
                return false;
            }
        }
    }
}

/// Where a persisted note can take over from the summarizing model, if anywhere.
///
/// Returns the history index the verbatim tail must start at and the text that
/// replaces everything before it. Compaction then costs no model call. It is
/// offered only when it is *provably* safe:
/// - the note has a cut point (`last_summarized_sequence`) and non-empty text;
/// - the tail starts at a message that maps back to an event after that point;
/// - every dropped message maps back to an event at or before it, so nothing
///   newer than the note is silently discarded.
pub fn compaction_from_note(
    store: &SessionMemoryStore,
    session_id: SessionId,
    events: &[Event],
    history: &[Message],
) -> Option<(usize, String)> {
    let state = store.load(session_id).ok()??;
    let covered = state.last_summarized_sequence?;
    if state.summary.trim().is_empty() {
        return None;
    }
    let sequence_of =
        |message: &Message| crate::input_runtime::sequence_of_message(events, message);
    let split = history.iter().position(|message| {
        sequence_of(message).is_some_and(|message_sequence| message_sequence > covered)
    })?;
    if split == 0 {
        // The note covers nothing that is actually being dropped, so it cannot
        // pay for this compaction.
        return None;
    }
    if history[..split]
        .iter()
        .any(|message| sequence_of(message).is_none())
    {
        return None;
    }
    Some((split, state.summary))
}

/// Runs the extraction pass over a registry containing nothing but the pinned
/// note tools.
pub struct SessionMemoryExtractor {
    db: Arc<Database>,
    client: Arc<ModelClient>,
    model: String,
    gate: ExtractionGate,
}

impl SessionMemoryExtractor {
    /// Build an extractor over the caller's already-configured model client.
    pub fn new(db: Arc<Database>, client: Arc<ModelClient>, model: impl Into<String>) -> Self {
        Self {
            db,
            client,
            model: model.into(),
            gate: ExtractionGate::default(),
        }
    }

    /// Override the scheduling thresholds.
    pub fn with_gate(mut self, gate: ExtractionGate) -> Self {
        self.gate = gate;
        self
    }

    /// Cheap pre-filter for hosts that must read a large session log to build
    /// the transcript: load only the cursor row and ask whether the token
    /// prerequisite could open the gate at all. The token condition is hard in
    /// both branches of [`ExtractionGate::decide`], so if even the most
    /// permissive tool-call count cannot fire it, no pass is possible and the
    /// read is skipped.
    pub fn might_be_due(&self, session_id: SessionId, total_tokens: u64) -> Result<bool> {
        let state = SessionMemoryStore::new(self.db.clone()).load(session_id)?;
        Ok(self
            .gate
            .decide(state.as_ref(), total_tokens, u64::MAX, false)
            .should_extract())
    }

    /// Run a pass when the gate allows it.
    ///
    /// `history` is the model-facing history and `events` this session's full
    /// event log. Only the messages after the note's cut point are handed to the
    /// pass, so the note grows incrementally; `events` also carry the cursors and
    /// the message-to-sequence mapping that decides the cut.
    /// `total_tokens` is deliberately the same metric auto-compaction uses —
    /// the question is "has the context grown", not "have the newest messages
    /// grown".
    pub async fn run_if_due(
        &self,
        session_id: SessionId,
        history: &[Message],
        events: &[Event],
        total_tokens: u64,
        last_turn_has_tool_call: bool,
    ) -> Result<ExtractionOutcome> {
        let store = SessionMemoryStore::new(self.db.clone());
        let state = store.load(session_id)?;
        let cursor = state
            .as_ref()
            .and_then(|state| state.tool_cursor_sequence)
            .unwrap_or(0);
        // Tool calls since the last scheduled pass, counted on the event log so
        // the cursor means what it says.
        let tool_calls = events
            .iter()
            .filter(|event| {
                event.sequence > cursor
                    && matches!(event.payload, EventPayload::ToolCallRequested { .. })
            })
            .count() as u64;
        let decision = self.gate.decide(
            state.as_ref(),
            total_tokens,
            tool_calls,
            last_turn_has_tool_call,
        );
        if !decision.should_extract() {
            return Ok(ExtractionOutcome::skipped("gate_wait"));
        }

        // Fold in only what the note does not cover yet. Re-sending the whole
        // visible history would make every pass cost a full context window and
        // would invite the model to rewrite rather than append.
        let covered_through = state
            .as_ref()
            .and_then(|state| state.last_summarized_sequence);
        let uncovered: Vec<&Message> = history
            .iter()
            .filter(|message| match covered_through {
                // A message that cannot be mapped back to an event is not provably
                // covered, so it stays in the window.
                None => true,
                Some(sequence) => crate::input_runtime::sequence_of_message(events, message)
                    .map_or(true, |message_sequence| message_sequence > sequence),
            })
            .collect();
        if uncovered.is_empty() {
            // The gate measures context growth, which is not the same thing as
            // new content: after a rewind the note's cut point can sit at or past
            // the end of the surviving stream, and a long single turn can be
            // folded in while nothing follows it. A pass over an empty transcript
            // would still pay for a read, an edit attempt and an answer, so it
            // stops here, before the note is even created.
            return Ok(ExtractionOutcome::skipped("nothing_uncovered"));
        }
        let transcript = uncovered
            .iter()
            .map(|message| format!("{:?}: {}", message.role, message.content))
            .collect::<Vec<_>>()
            .join("\n");

        let note_path = store.memory_path(session_id);
        let note = ensure_note(&note_path)?;
        let prompt = build_extraction_prompt(&note_path, &note, &transcript);

        let permissions = PermissionSet::developer();
        let registry = note_registry(&note_path, &permissions)?;
        let tools = registry
            .visible_to(&permissions)
            .into_iter()
            .map(|descriptor| {
                ToolSchema::function(
                    descriptor.name,
                    descriptor.description,
                    descriptor.parameters,
                )
            })
            .collect();
        let system = "You are the session-note maintenance agent. The ONLY file you may read or \
                      edit is the session note named in the task. Make minimal, precise edits; \
                      never rewrite the note wholesale unless it is still the empty template."
            .to_string();
        let mut agent = ModelAgent::new(
            self.client.clone(),
            self.model.clone(),
            system,
            prompt.as_str(),
            tools,
        )
        .with_max_model_attempts(2);

        // Own run log for the pass, as the hook agent does: its tool calls and
        // terminal state land in a normal kernel run, not a bespoke loop.
        let run_db = Database::open_in_memory()?;
        let clock = SystemClock;
        let mut run_session = Session::create(&run_db, &clock, Some("session-memory"))?;
        let task = run_session.create_task(&prompt)?;
        let config = RuntimeConfig {
            max_steps: 6,
            max_total_tokens: Some(32_000),
            permissions,
            auto_approve: false,
            // The loop derives "this run must have mutated a file" from the very
            // fact that `edit_file` ran, and validates it against checkpoint
            // evidence this internal pass deliberately does not capture. One
            // blocked completion would cost a whole extra model call to re-assert
            // work already proven by reading the note back below, so the gate
            // fails open immediately here.
            max_completion_retries: 0,
            ..RuntimeConfig::default()
        };
        let engine: RuntimeEngine<'_, SystemClock> =
            RuntimeEngine::new(&registry, Default::default(), config);
        let completed = match engine.run(&mut run_session, task, &mut agent).await? {
            RunOutcome::Completed(_) => true,
            other => {
                tracing::warn!(outcome = ?other, "session-memory pass did not complete");
                false
            }
        };
        let usage = agent.cumulative_usage();
        if !completed {
            // The pass may have burned several answered calls before it stopped.
            // That is real spend and still belongs in the ledger, even though the
            // note gained nothing from it.
            return Ok(ExtractionOutcome {
                extracted: false,
                reason: "pass_failed",
                usage,
            });
        }

        let written = std::fs::read_to_string(&note_path)
            .map_err(|error| CoreError::Persistence(format!("read back session note: {error}")))?;
        // Editing the note is the whole job. If the agent declared completion
        // without changing a byte, the note cannot be trusted to cover the
        // transcript, so the cut point must NOT move: compaction that believed it
        // would drop transcript content the note never summarised. The token
        // watermark still advances, because that attempt really was paid for and
        // the next turn must not re-buy the same window.
        let covered = written != note;
        // The note now covers everything handed in, so the candidate cut point is
        // the session's newest event. It becomes the actual cut point only when
        // the last turn opened no tool call — otherwise the compaction split
        // would orphan a `tool_use` from its result.
        let newest_sequence = events.last().map(|event| event.sequence).unwrap_or(0);
        let mut advanced = advance_cursors(
            state.unwrap_or_default(),
            newest_sequence,
            covered && !last_turn_has_tool_call,
            total_tokens,
            SystemClock.now(),
        );
        advanced.summary = written;
        store.save(session_id, &advanced)?;

        Ok(ExtractionOutcome {
            extracted: covered,
            reason: if covered {
                "extracted"
            } else {
                "note_unchanged"
            },
            usage,
        })
    }
}

/// Registry holding nothing but the note's `read_file` + `edit_file` pair, each
/// pinned to `note_path` and visible under `permissions`.
fn note_registry(
    note_path: &Path,
    permissions: &PermissionSet,
) -> Result<deepagent_tools::ToolRegistry> {
    let root = note_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut registry = deepagent_tools::ToolRegistry::new();
    for tool in note_edit_tools(WorkspaceRoot::new(root)) {
        registry.register(Arc::new(PinnedFileTool::new(tool, note_path.to_path_buf())))?;
    }
    // A pinned tool that the granted permissions would hide is a silent failure
    // waiting to happen, so refuse to build such a registry at all.
    let visible = registry.visible_to(permissions).len();
    if visible != registry.len() {
        return Err(CoreError::invalid(format!(
            "session-note registry needs permissions the agent does not grant ({visible} of {} tools visible)",
            registry.len()
        )));
    }
    Ok(registry)
}

/// Scripted model transport shared by this module's tests and by the run
/// finalizer's wiring tests: one assistant response per model call.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use deepagent_core::error::{CoreError, Result};
    use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
    use deepagent_models::{ModelClient, ModelConfig};

    pub(crate) struct ScriptedTransport {
        turns: Mutex<VecDeque<Vec<String>>>,
        last_body: Mutex<Option<String>>,
    }

    impl ScriptedTransport {
        /// The terminal event carries usage, so a pass can report what it cost.
        fn completed() -> String {
            serde_json::json!({
                "type": "response.completed",
                "response": {
                    "status": "completed",
                    "usage": {"input_tokens": 120, "output_tokens": 18, "total_tokens": 138},
                },
            })
            .to_string()
        }

        pub(crate) fn text(delta: &str) -> Vec<String> {
            vec![
                serde_json::json!({"type":"response.output_text.delta","delta":delta}).to_string(),
                Self::completed(),
            ]
        }

        pub(crate) fn call(call_id: &str, name: &str, arguments: serde_json::Value) -> Vec<String> {
            let item = serde_json::json!({
                "type": "function_call",
                "id": format!("item_{call_id}"),
                "call_id": call_id,
                "name": name,
                "arguments": serde_json::to_string(&arguments).unwrap(),
            });
            vec![
                serde_json::json!({"type":"response.output_item.added","item":item}).to_string(),
                serde_json::json!({"type":"response.output_item.done","item":item}).to_string(),
                Self::completed(),
            ]
        }

        fn new(turns: Vec<Vec<String>>) -> Arc<Self> {
            Arc::new(Self {
                turns: Mutex::new(turns.into_iter().collect()),
                last_body: Mutex::new(None),
            })
        }

        /// The most recent request body, so a test can assert what the pass was
        /// actually told rather than what it was supposed to be told.
        pub(crate) fn last_prompt(&self) -> String {
            self.last_body
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone()
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl HttpTransport for ScriptedTransport {
        async fn stream(&self, request: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
            *self
                .last_body
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(request.body);
            let events = self
                .turns
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .pop_front()
                .ok_or_else(|| CoreError::other("scripted model has no remaining turn"))?;
            for event in &events {
                if sink.on_event(event)? {
                    break;
                }
            }
            Ok(())
        }
    }

    /// A client whose transport refuses loudly: for tests that must prove the
    /// model is never consulted.
    pub(crate) fn unreachable_client() -> Arc<ModelClient> {
        struct Refusing;

        #[async_trait]
        impl HttpTransport for Refusing {
            async fn stream(
                &self,
                _request: TransportRequest,
                _sink: &mut dyn EventSink,
            ) -> Result<()> {
                Err(CoreError::other(
                    "the model must not be called for this compaction",
                ))
            }
        }

        Arc::new(ModelClient::new(
            Arc::new(Refusing),
            ModelConfig::deepseek("sk-synthetic-test-key"),
        ))
    }

    /// A model client replaying `turns` (synthetic key; nothing leaves the test).
    pub(crate) fn scripted_client(turns: Vec<Vec<String>>) -> Arc<ModelClient> {
        scripted_client_with_transport(turns).0
    }

    /// Same as [`scripted_client`] but also hands back the transport, so a test
    /// can inspect the prompt the pass sent.
    pub(crate) fn scripted_client_with_transport(
        turns: Vec<Vec<String>>,
    ) -> (Arc<ModelClient>, Arc<ScriptedTransport>) {
        let transport = ScriptedTransport::new(turns);
        let client = Arc::new(ModelClient::new(
            transport.clone(),
            ModelConfig::deepseek("sk-synthetic-test-key"),
        ));
        (client, transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use super::testing::{scripted_client, scripted_client_with_transport, ScriptedTransport};
    use async_trait::async_trait;
    use deepagent_core::id::EventId;
    use deepagent_core::message::ToolCall;

    fn ts(millis: i64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn state(tokens: u64) -> SessionMemoryState {
        SessionMemoryState {
            initialized: true,
            tokens_at_last_extraction: tokens,
            ..Default::default()
        }
    }

    fn tool_call(sequence: Sequence) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::new(),
            sequence,
            timestamp: ts(1_000 + sequence as i64),
            payload: EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: format!("call-{sequence}"),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({}),
                },
            },
        }
    }

    // ---- gate -------------------------------------------------------------

    #[test]
    fn no_note_yet_waits_until_the_context_is_big_enough() {
        let gate = ExtractionGate::default();
        assert_eq!(
            gate.decide(None, 9_999, 99, false),
            GateDecision::Wait,
            "tool calls alone must not create the note"
        );
        assert_eq!(gate.decide(None, 10_000, 0, true), GateDecision::Initialize);
    }

    #[test]
    fn token_growth_is_a_hard_prerequisite_for_an_update() {
        let gate = ExtractionGate::default();
        let note = state(20_000);
        // Plenty of tool calls but the context barely moved: still waiting.
        assert_eq!(
            gate.decide(Some(&note), 23_000, 12, false),
            GateDecision::Wait
        );
        // Enough growth AND enough tool calls.
        assert_eq!(
            gate.decide(Some(&note), 26_000, 3, true),
            GateDecision::Update
        );
        // Enough growth and the last turn opened no tool call: the second
        // branch of the reference rule.
        assert_eq!(
            gate.decide(Some(&note), 26_000, 1, false),
            GateDecision::Update
        );
        // Enough growth but the turn ended on a tool call and there are too few
        // of them: nothing worth folding in yet.
        assert_eq!(
            gate.decide(Some(&note), 26_000, 2, true),
            GateDecision::Wait
        );
    }

    #[test]
    fn cheap_pre_filter_follows_only_the_token_prerequisite() {
        // Hosts use this to decide whether reading the session log is worth it,
        // so it must be true whenever ANY branch of the real gate could fire.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let extractor =
            SessionMemoryExtractor::new(db.clone(), scripted_client(vec![]), "deepseek-chat");
        let session = SessionId::new();

        assert!(!extractor.might_be_due(session, 9_999).unwrap());
        assert!(extractor.might_be_due(session, 10_000).unwrap());

        store_state(&db, &session, &state(20_000));
        assert!(
            !extractor.might_be_due(session, 23_000).unwrap(),
            "3k of growth cannot open the gate for any tool-call count"
        );
        assert!(extractor.might_be_due(session, 30_000).unwrap());
    }

    fn store_state(db: &Arc<Database>, session: &SessionId, state: &SessionMemoryState) {
        SessionMemoryStore::new(db.clone())
            .save(*session, state)
            .unwrap();
    }

    // ---- cursors ----------------------------------------------------------
    #[test]
    fn cut_point_holds_while_the_tool_cursor_moves() {
        let note = SessionMemoryState {
            last_summarized_sequence: Some(10),
            tool_cursor_sequence: Some(10),
            ..SessionMemoryState::default()
        };
        let after = advance_cursors(note, 42, false, 30_000, ts(1));
        assert_eq!(after.last_summarized_sequence, Some(10));
        assert_eq!(after.tool_cursor_sequence, Some(42));
        assert_eq!(after.tokens_at_last_extraction, 30_000);
        assert!(after.initialized);

        let safe = advance_cursors(after, 50, true, 36_000, ts(2));
        assert_eq!(safe.last_summarized_sequence, Some(50));
        assert_eq!(safe.updated_at, 2);
    }

    #[test]
    fn unsafe_cut_point_survives_the_next_pass() {
        // First pass lands right after a tool call: the note exists but cannot
        // be used as a compaction boundary until a later safe pass.
        let first = advance_cursors(SessionMemoryState::default(), 7, false, 12_000, ts(1));
        assert_eq!(first.last_summarized_sequence, None);
        let second = advance_cursors(first, 9, true, 18_000, ts(2));
        assert_eq!(second.last_summarized_sequence, Some(9));
    }

    // ---- tail safety read -------------------------------------------------

    fn completed(sequence: Sequence, call_id: &str) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::new(),
            sequence,
            timestamp: ts(1_000 + sequence as i64),
            payload: EventPayload::ToolCallCompleted {
                call_id: call_id.to_string(),
                ok: true,
                output: serde_json::json!("done"),
                duration_ms: 1,
            },
        }
    }

    fn requested(sequence: Sequence, call_id: &str) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::new(),
            sequence,
            timestamp: ts(1_000 + sequence as i64),
            payload: EventPayload::ToolCallRequested {
                call: ToolCall {
                    id: call_id.to_string(),
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({}),
                },
            },
        }
    }

    #[test]
    fn tail_read_flags_a_tool_call_that_has_no_result_yet() {
        assert!(!pending_tool_call_at_tail(&[]));
        assert!(
            pending_tool_call_at_tail(&[requested(1, "call-1")]),
            "a run cancelled between call and result must not be cut there"
        );
        assert!(!pending_tool_call_at_tail(&[
            requested(1, "call-1"),
            completed(2, "call-1")
        ]));
        // An older orphan does not matter; only the newest call decides.
        assert!(!pending_tool_call_at_tail(&[
            requested(1, "call-1"),
            requested(2, "call-2"),
            completed(3, "call-2")
        ]));
    }

    #[test]
    fn one_pass_per_session_and_none_across_sessions() {
        let passes = SessionMemoryPasses::default();
        let first = SessionId::new();
        let second = SessionId::new();
        let guard = passes.try_begin(first).expect("first pass claims the slot");
        assert!(
            passes.try_begin(first).is_none(),
            "second pass must not queue"
        );
        assert!(
            passes.try_begin(second).is_some(),
            "another session is independent"
        );
        drop(guard);
        assert!(
            passes.try_begin(first).is_some(),
            "the slot is free again once the pass ends"
        );
    }

    /// The gate measures context growth, not new content. A note that already
    /// covers everything — the shape left behind by a rewind, or by a pass whose
    /// cut point sits past the surviving stream — must not buy a round trip, and
    /// must not even create the note file.
    #[tokio::test]
    async fn nothing_uncovered_buys_no_model_call_and_no_note() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        store
            .save(
                session,
                &SessionMemoryState {
                    summary: "- already folded".to_string(),
                    last_summarized_sequence: Some(9),
                    tool_cursor_sequence: Some(9),
                    tokens_at_last_extraction: 12_000,
                    initialized: true,
                    updated_at: 1,
                },
            )
            .unwrap();

        // Eight thousand tokens of growth, so the token condition is satisfied:
        // only the empty uncovered window can stop this pass.
        let events = vec![message_event(1, "decide the storage layout")];
        let outcome = SessionMemoryExtractor::new(
            db.clone(),
            testing::unreachable_client(),
            "model".to_string(),
        )
        .with_gate(ExtractionGate {
            init_tokens: 1,
            ..Default::default()
        })
        .run_if_due(
            session,
            &[Message::user("decide the storage layout")],
            &events,
            20_000,
            false,
        )
        .await
        .unwrap();

        assert!(
            !outcome.extracted,
            "nothing was left to fold in: {outcome:?}"
        );
        assert_eq!(outcome.reason, "nothing_uncovered");
        assert!(
            outcome.usage.is_none(),
            "no model call means nothing to bill: {outcome:?}"
        );
        assert!(
            !store.memory_path(session).exists(),
            "a pass that never started must not leave a note behind"
        );
    }

    /// An unfinished pass must leave no watermark behind. The cut point is what
    /// lets compaction remove transcript from the model view, so persisting it
    /// after a pass that ran out of steps would delete content the note never
    /// summarised — the same failure the unchanged-note guard closes.
    #[tokio::test]
    async fn an_abandoned_pass_saves_no_watermark_and_leaves_the_note_alone() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        let note_path = store.memory_path(session);
        std::fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        let template = ensure_note(&note_path).unwrap();

        // The scripted agent keeps reading the note and never answers, so the
        // loop exhausts its steps instead of completing.
        let stuck: Vec<Vec<String>> = (0..10)
            .map(|index| {
                ScriptedTransport::call(
                    &format!("c{index}"),
                    "read_file",
                    serde_json::json!({"path": note_path.to_string_lossy()}),
                )
            })
            .collect();
        let outcome =
            SessionMemoryExtractor::new(db.clone(), scripted_client(stuck), "model".to_string())
                .with_gate(ExtractionGate {
                    init_tokens: 1,
                    ..Default::default()
                })
                .run_if_due(
                    session,
                    &[Message::user("decide the storage layout")],
                    &[tool_call(1), tool_call(2), tool_call(3)],
                    12_000,
                    true,
                )
                .await
                .unwrap();

        assert!(
            !outcome.extracted,
            "an unfinished pass extracted nothing: {outcome:?}"
        );
        assert_eq!(outcome.reason, "pass_failed");
        assert_eq!(
            std::fs::read_to_string(&note_path).unwrap(),
            template,
            "the note must be exactly as it was"
        );
        assert!(
            store.load(session).unwrap().is_none(),
            "no cursors may be written for a pass that never completed"
        );
        assert!(
            outcome.usage.is_some(),
            "the abandoned calls still cost tokens and stay billable: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn wait_idle_reports_a_pass_that_is_still_running() {
        let passes = SessionMemoryPasses::default();
        let guard = passes
            .try_begin(SessionId::new())
            .expect("first pass claims the slot");
        assert!(
            !passes.wait_idle(std::time::Duration::from_millis(50)).await,
            "a pass still in flight must not be reported as drained"
        );
        drop(guard);
        assert!(
            passes
                .wait_idle(std::time::Duration::from_millis(500))
                .await,
            "the queue is empty once the guard is released"
        );
    }

    /// The drain is on the exit path of a one-shot process, so a pass that dies
    /// must not strand it: the guard releases its slot while unwinding.
    #[tokio::test]
    async fn wait_idle_is_not_stranded_by_a_panicking_pass() {
        let passes = SessionMemoryPasses::default();
        let guard = passes
            .try_begin(SessionId::new())
            .expect("first pass claims the slot");
        let panicker = tokio::spawn(async move {
            let _guard = guard;
            panic!("the extraction task died");
        });
        assert!(
            passes
                .wait_idle(std::time::Duration::from_millis(2_000))
                .await,
            "a crashed pass must not hold the drain open"
        );
        assert!(
            panicker.await.is_err(),
            "the task really did panic, otherwise this proves nothing"
        );
    }

    #[tokio::test]
    async fn wait_idle_wakes_when_a_background_pass_finishes() {
        let passes = SessionMemoryPasses::default();
        let guard = passes
            .try_begin(SessionId::new())
            .expect("first pass claims the slot");
        let finisher = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            drop(guard);
        });
        assert!(
            passes
                .wait_idle(std::time::Duration::from_millis(2_000))
                .await,
            "waiting must end when the pass releases its slot, not on the timeout"
        );
        finisher.await.expect("the pass task completes");
    }

    // ---- compaction hand-off ---------------------------------------------

    fn note_covering(db: &Arc<Database>, session: SessionId, covered: Sequence) {
        SessionMemoryStore::new(db.clone())
            .save(
                session,
                &SessionMemoryState {
                    summary: "- goal: one vault\n- files hold bodies".to_string(),
                    last_summarized_sequence: Some(covered),
                    tool_cursor_sequence: Some(covered),
                    tokens_at_last_extraction: 20_000,
                    initialized: true,
                    updated_at: 1,
                },
            )
            .unwrap();
    }

    fn three_turn_transcript() -> (Vec<Event>, Vec<Message>) {
        let messages = vec![
            Message::user("first decision: one vault"),
            Message::assistant("agreed"),
            Message::user("second decision: files hold bodies"),
        ];
        let events = vec![
            message_event(1, "first decision: one vault"),
            Event {
                id: EventId::new(),
                session_id: SessionId::new(),
                sequence: 2,
                timestamp: ts(1_002),
                payload: EventPayload::MessageAppended {
                    message: Message::assistant("agreed"),
                },
            },
            message_event(5, "second decision: files hold bodies"),
        ];
        (events, messages)
    }

    #[test]
    fn a_note_that_covers_the_prefix_becomes_the_compaction_summary() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        note_covering(&db, session, 2);
        let (events, history) = three_turn_transcript();
        let store = SessionMemoryStore::new(db.clone());

        let (split, block) = compaction_from_note(&store, session, &events, &history)
            .expect("the note covers the first two entries");
        assert_eq!(split, 2, "the tail starts after the covered prefix");
        assert!(block.contains("files hold bodies") || block.contains("one vault"));
    }

    #[test]
    fn a_note_is_refused_when_it_cannot_place_everything_it_drops() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        note_covering(&db, session, 99);
        let (events, history) = three_turn_transcript();
        // A message with no matching event may be newer than the note, so the
        // prefix cannot be proven covered and the summarizing call must stay.
        let mut unmappable = history.clone();
        unmappable.insert(0, Message::user("not in the event log"));
        let store = SessionMemoryStore::new(db.clone());
        assert!(compaction_from_note(&store, session, &events, &unmappable).is_none());
    }

    #[test]
    fn a_note_that_covers_nothing_droppable_is_refused() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        note_covering(&db, session, 0);
        let (events, history) = three_turn_transcript();
        let store = SessionMemoryStore::new(db.clone());
        assert!(
            compaction_from_note(&store, session, &events, &history).is_none(),
            "split 0 would add a summary without removing anything"
        );
    }

    #[test]
    fn no_note_means_no_compaction_shortcut() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        let (events, history) = three_turn_transcript();
        let store = SessionMemoryStore::new(db.clone());
        assert!(compaction_from_note(&store, session, &events, &history).is_none());
    }

    // ---- path pinning -----------------------------------------------------

    struct CountingTool {
        hits: Arc<Mutex<usize>>,
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                name: "edit_file".into(),
                description: "test".into(),
                parameters: serde_json::json!({"type":"object"}),
                risk: deepagent_tools::permission::RiskLevel::Low,
                required_permissions: PermissionSet::developer(),
            }
        }

        async fn invoke(&self, _args: serde_json::Value) -> Result<ToolOutput> {
            *self.hits.lock().unwrap() += 1;
            Ok(ToolOutput::success(serde_json::json!({"ok": true})))
        }
    }

    async fn refusal(pin: &PinnedFileTool, args: serde_json::Value) -> String {
        let output = pin.invoke(args).await.unwrap();
        assert!(!output.ok, "off-note access must be a tool failure");
        output
            .value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn pinned_tool_only_admits_the_note_path() {
        let note = Path::new("C:/data/files/sessions/ab/abc/session-memory.md");
        let hits = Arc::new(Mutex::new(0));
        let pin = PinnedFileTool::new(Arc::new(CountingTool { hits: hits.clone() }), note);

        for attempt in [
            note.parent().unwrap().join("other.md"),
            PathBuf::from("C:/Users/victim/.ssh/id_ed25519"),
            note.join("..").join("..").join("escape.md"),
        ] {
            let text = refusal(&pin, serde_json::json!({"path": attempt.to_string_lossy()})).await;
            assert!(
                text.contains("only") && text.contains("session-memory.md"),
                "off-note access must be refused: {text}"
            );
        }
        assert_eq!(
            *hits.lock().unwrap(),
            0,
            "refusals must not reach the inner tool"
        );

        // The exact path (absolute) and the same file addressed relative to the
        // note directory both pass, including a redundant `./` prefix.
        for path in [
            note.to_string_lossy().to_string(),
            "session-memory.md".to_string(),
            "./session-memory.md".to_string(),
        ] {
            pin.invoke(serde_json::json!({"path": path})).await.unwrap();
        }
        assert_eq!(*hits.lock().unwrap(), 3);

        // A call with no usable path is an error, not a silent pass.
        let error = pin.invoke(serde_json::json!({})).await.unwrap_err();
        assert!(matches!(error, CoreError::Invalid(_)), "{error:?}");
    }

    #[test]
    fn note_registry_exposes_exactly_the_pinned_pair() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("session-memory.md");
        let permissions = PermissionSet::developer();
        let registry = note_registry(&note, &permissions).unwrap();
        let mut names: Vec<String> = registry
            .visible_to(&permissions)
            .into_iter()
            .map(|descriptor| descriptor.name)
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["edit_file".to_string(), "read_file".to_string()]
        );

        // Read-only permissions cannot see the write tool, so the same registry
        // must fail to build rather than hand the pass a silently empty toolset.
        let err = note_registry(&note, &PermissionSet::read_only())
            .err()
            .unwrap();
        assert!(err.to_string().contains("session-note registry"));
    }

    // ---- note file --------------------------------------------------------

    #[test]
    fn ensure_note_creates_once_and_never_clobbers() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("nested").join("session-memory.md");
        assert_eq!(ensure_note(&note).unwrap(), NOTE_TEMPLATE);
        std::fs::write(&note, "# Session notes\n\nhandoff decided\n").unwrap();
        assert_eq!(
            ensure_note(&note).unwrap(),
            "# Session notes\n\nhandoff decided\n"
        );
    }

    #[test]
    fn extraction_prompt_pins_the_note_path_and_shows_the_note() {
        let note = Path::new("C:/data/sessions/abc/session-memory.md");
        let prompt = build_extraction_prompt(note, "old note", "user: pick Postgres");
        assert!(prompt.contains("C:/data/sessions/abc/session-memory.md"));
        assert!(prompt.contains("old note"));
        assert!(prompt.contains("pick Postgres"));
    }

    // ---- store ------------------------------------------------------------

    #[test]
    fn cursors_round_trip_through_the_documents_table() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        assert_eq!(store.load(session).unwrap(), None);

        let saved = SessionMemoryState {
            summary: "note text".into(),
            last_summarized_sequence: Some(11),
            tool_cursor_sequence: Some(13),
            tokens_at_last_extraction: 21_000,
            initialized: true,
            updated_at: 5,
        };
        store.save(session, &saved).unwrap();
        assert_eq!(store.load(session).unwrap(), Some(saved.clone()));

        // Update, not duplicate: one row per session.
        let updated = SessionMemoryState {
            tokens_at_last_extraction: 26_000,
            ..saved
        };
        store.save(session, &updated).unwrap();
        assert_eq!(store.load(session).unwrap(), Some(updated));
        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM documents WHERE collection = ?1",
                    rusqlite::params![SESSION_MEMORY_COLLECTION],
                    |row| row.get(0),
                )
                .map_err(|error| CoreError::Persistence(error.to_string()))
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    // ---- extraction pass (scenario) ---------------------------------------

    #[tokio::test]
    async fn pinned_pair_satisfies_read_before_edit_on_the_note() {
        // The seam this pass depends on: `edit_file` enforces read-before-edit
        // through a cache shared by `note_edit_tools`, and both tools must
        // resolve the note path to the same key through `PinnedFileTool`.
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("session-memory.md");
        ensure_note(&note).unwrap();
        let permissions = PermissionSet::developer();
        let registry = note_registry(&note, &permissions).unwrap();

        let read = registry
            .get("read_file")
            .expect("read_file registered")
            .tool
            .clone();
        let edit = registry
            .get("edit_file")
            .expect("edit_file registered")
            .tool
            .clone();

        let before = edit
            .invoke(serde_json::json!({
                "path": note.to_string_lossy(),
                "old": "(nothing recorded yet)",
                "new": "- decided: one vault",
            }))
            .await
            .unwrap();
        assert!(
            !before.ok,
            "editing before reading must be refused: {before:?}"
        );

        let read_output = read
            .invoke(serde_json::json!({"path": note.to_string_lossy()}))
            .await
            .unwrap();
        assert!(read_output.ok, "note must be readable: {read_output:?}");

        let after = edit
            .invoke(serde_json::json!({
                "path": note.to_string_lossy(),
                "old": "(nothing recorded yet)",
                "new": "- decided: one vault",
            }))
            .await
            .unwrap();
        assert!(
            after.ok,
            "read-then-edit must succeed on the same path: {after:?}"
        );
        let written = std::fs::read_to_string(&note).unwrap();
        assert!(written.contains("decided: one vault"), "{written}");
    }

    #[tokio::test]
    async fn the_pinned_write_tool_refuses_off_note_targets_and_writes_nothing() {
        // The whitelist claim is about the real `edit_file`, not a stand-in: a
        // hijacked pass must not write anywhere else, whether it names an
        // absolute sibling, spells a `..` escape, or hides behind a near-miss
        // filename. A refusal must leave the filesystem byte-for-byte alone.
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("session-memory.md");
        ensure_note(&note).unwrap();
        let sibling = dir.path().join("secret.md");
        std::fs::write(&sibling, "leave me alone").unwrap();
        let escape = dir.path().join("..").join("deepagent-note-pin-escape.md");

        let registry = note_registry(&note, &PermissionSet::developer()).unwrap();
        let edit = registry
            .get("edit_file")
            .expect("edit_file registered")
            .tool
            .clone();

        for path in [
            sibling.clone(),
            PathBuf::from("..").join("deepagent-note-pin-escape.md"),
            PathBuf::from("session-memory.md.bak"),
        ] {
            let output = edit
                .invoke(serde_json::json!({
                    "path": path.to_string_lossy(),
                    "old": "(nothing recorded yet)",
                    "new": "pwned",
                }))
                .await
                .unwrap();
            assert!(!output.ok, "off-note write must be refused: {output:?}");
            let error = output
                .value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            assert!(
                error.contains("session-memory.md"),
                "the refusal must name the one permitted file: {error}"
            );
        }

        assert_eq!(
            std::fs::read_to_string(&sibling).unwrap(),
            "leave me alone",
            "a refused edit must not touch the sibling"
        );
        assert_eq!(std::fs::read_to_string(&note).unwrap(), NOTE_TEMPLATE);
        assert!(
            !escape.exists(),
            "a refused write must not create its target"
        );
        assert!(!dir.path().join("session-memory.md.bak").exists());
    }

    async fn run_pass(
        turns: Vec<Vec<String>>,
        gate: ExtractionGate,
        total_tokens: u64,
        last_turn_has_tool_call: bool,
    ) -> (Arc<Database>, SessionId, ExtractionOutcome) {
        // `open_in_memory` keeps its own temp data root alive, which is where
        // the note file lands -- the same layout production uses.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        let client = scripted_client(turns);
        let extractor =
            SessionMemoryExtractor::new(db.clone(), client, "deepseek-chat").with_gate(gate);
        let history = vec![Message::user("decide the storage layout")];
        let events = vec![tool_call(1), tool_call(2), tool_call(3)];
        let outcome = extractor
            .run_if_due(
                session,
                &history,
                &events,
                total_tokens,
                last_turn_has_tool_call,
            )
            .await
            .unwrap();
        (db, session, outcome)
    }

    #[tokio::test]
    async fn gated_pass_edits_the_note_and_records_a_safe_cut_point() {
        let note_edit = |path: &Path| {
            ScriptedTransport::call(
                "c2",
                "edit_file",
                serde_json::json!({
                    "path": path.to_string_lossy(),
                    "old": "(nothing recorded yet)",
                    "new": "- goal: pick the storage layout",
                }),
            )
        };
        // The scripted model reads the note, edits it, then answers.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        let note_path = store.memory_path(session);
        std::fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        ensure_note(&note_path).unwrap();

        let client = scripted_client(vec![
            ScriptedTransport::call(
                "c1",
                "read_file",
                serde_json::json!({"path": note_path.to_string_lossy()}),
            ),
            note_edit(&note_path),
            ScriptedTransport::text("note updated"),
        ]);
        let extractor = SessionMemoryExtractor::new(db.clone(), client, "deepseek-chat").with_gate(
            ExtractionGate {
                init_tokens: 1,
                ..Default::default()
            },
        );
        let events = vec![tool_call(1), tool_call(2)];
        let outcome = extractor
            .run_if_due(
                session,
                &[Message::user("decide the storage layout")],
                &events,
                12_000,
                false,
            )
            .await
            .unwrap();

        assert!(outcome.extracted, "gate should have fired: {outcome:?}");
        assert_eq!(outcome.reason, "extracted");
        // Provider semantics: prompt tokens are the last request's context total,
        // completion tokens accumulate across the pass's three calls.
        let usage = outcome.usage.expect("the pass reports provider usage");
        assert_eq!(usage.prompt_tokens, 120);
        assert_eq!(usage.completion_tokens, 54);
        let note = std::fs::read_to_string(&note_path).unwrap();
        assert!(note.contains("goal: pick the storage layout"), "{note}");

        let saved = store.load(session).unwrap().unwrap();
        assert_eq!(saved.last_summarized_sequence, Some(2), "newest event");
        assert_eq!(saved.tool_cursor_sequence, Some(2));
        assert_eq!(saved.tokens_at_last_extraction, 12_000);
        assert_eq!(saved.summary, note);
    }

    /// Completion without an edit proves nothing. The cut point must not move —
    /// compaction trusts it to represent the transcript — while the paid-for
    /// attempt still advances the token watermark so the next turn does not buy
    /// the same window twice.
    #[tokio::test]
    async fn a_pass_that_leaves_the_note_unchanged_claims_no_coverage() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        let note_path = store.memory_path(session);
        std::fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        let template = ensure_note(&note_path).unwrap();

        let client = scripted_client(vec![
            ScriptedTransport::call(
                "c1",
                "read_file",
                serde_json::json!({"path": note_path.to_string_lossy()}),
            ),
            ScriptedTransport::text("nothing worth recording"),
        ]);
        let extractor = SessionMemoryExtractor::new(db.clone(), client, "deepseek-chat").with_gate(
            ExtractionGate {
                init_tokens: 1,
                ..Default::default()
            },
        );
        let outcome = extractor
            .run_if_due(
                session,
                &[Message::user("decide the storage layout")],
                &[tool_call(1), tool_call(2)],
                12_000,
                false,
            )
            .await
            .unwrap();

        assert!(
            !outcome.extracted,
            "an untouched note cannot count as extracted: {outcome:?}"
        );
        assert_eq!(outcome.reason, "note_unchanged");
        assert!(outcome.usage.is_some(), "the attempt still cost tokens");
        assert_eq!(std::fs::read_to_string(&note_path).unwrap(), template);

        let saved = store
            .load(session)
            .unwrap()
            .expect("the attempt is recorded");
        assert_eq!(
            saved.last_summarized_sequence, None,
            "compaction may not cut on a note that says nothing new"
        );
        assert_eq!(
            saved.tool_cursor_sequence,
            Some(2),
            "the calls themselves were still seen"
        );
        assert_eq!(
            saved.tokens_at_last_extraction, 12_000,
            "the window that was paid for is remembered"
        );
    }

    fn message_event(sequence: Sequence, content: &str) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::new(),
            sequence,
            timestamp: ts(1_000 + sequence as i64),
            payload: EventPayload::MessageAppended {
                message: Message::user(content),
            },
        }
    }

    #[tokio::test]
    async fn second_pass_folds_in_only_the_uncovered_tail() {
        // The note already covers the first message, so the pass must be shown
        // only what came after it. Re-sending the whole window would make the
        // note a re-derivation instead of an increment, and cost a context
        // window per pass.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let session = SessionId::new();
        let store = SessionMemoryStore::new(db.clone());
        store
            .save(
                session,
                &SessionMemoryState {
                    initialized: true,
                    last_summarized_sequence: Some(1),
                    tool_cursor_sequence: Some(1),
                    tokens_at_last_extraction: 20_000,
                    summary: "# Session notes\n\n- first decision recorded\n".to_string(),
                    ..Default::default()
                },
            )
            .unwrap();

        let events = vec![
            message_event(1, "first decision: one vault"),
            message_event(5, "second decision: files hold bodies"),
        ];
        let history = vec![
            Message::user("first decision: one vault"),
            Message::user("second decision: files hold bodies"),
        ];
        let (client, transport) = scripted_client_with_transport(vec![
            ScriptedTransport::call(
                "c1",
                "read_file",
                serde_json::json!({"path": "session-memory.md"}),
            ),
            ScriptedTransport::call(
                "c2",
                "edit_file",
                serde_json::json!({
                    "path": "session-memory.md",
                    "old": "(nothing recorded yet)",
                    "new": "- files hold bodies",
                }),
            ),
            ScriptedTransport::text("note updated"),
        ]);
        SessionMemoryExtractor::new(db.clone(), client, "deepseek-chat")
            .run_if_due(session, &history, &events, 30_000, false)
            .await
            .unwrap();

        let prompt = transport.last_prompt();
        assert!(
            prompt.contains("second decision: files hold bodies"),
            "the new turn must be folded in: {prompt}"
        );
        assert!(
            !prompt.contains("first decision: one vault"),
            "already-covered history must not be re-sent: {prompt}"
        );
        // The cut point moves to the newest event because the turn ended cleanly.
        let saved = store.load(session).unwrap().unwrap();
        assert_eq!(saved.last_summarized_sequence, Some(5));
    }

    #[tokio::test]
    async fn unsafe_last_turn_keeps_the_note_but_does_not_move_the_cut_point() {
        let (db, session, outcome) = run_pass(
            vec![
                ScriptedTransport::call(
                    "c1",
                    "read_file",
                    serde_json::json!({"path": "session-memory.md"}),
                ),
                ScriptedTransport::call(
                    "c2",
                    "edit_file",
                    serde_json::json!({
                        "path": "session-memory.md",
                        "old": "(nothing recorded yet)",
                        "new": "- mid-turn facts",
                    }),
                ),
                ScriptedTransport::text("done"),
            ],
            ExtractionGate {
                init_tokens: 1,
                ..Default::default()
            },
            12_000,
            true,
        )
        .await;
        assert!(outcome.extracted);
        let saved = SessionMemoryStore::new(db).load(session).unwrap().unwrap();
        assert_eq!(saved.last_summarized_sequence, None);
        assert_eq!(saved.tool_cursor_sequence, Some(3));
    }

    #[tokio::test]
    async fn gate_wait_runs_no_model_call_and_writes_no_note() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        let client = scripted_client(vec![]);
        let outcome = SessionMemoryExtractor::new(db.clone(), client, "deepseek-chat")
            .run_if_due(session, &[], &[], 500, false)
            .await
            .unwrap();
        assert_eq!(outcome, ExtractionOutcome::skipped("gate_wait"));
        assert_eq!(store.load(session).unwrap(), None);
        assert!(!store.memory_path(session).exists());
    }

    #[tokio::test]
    async fn off_note_access_is_refused_and_the_note_stays_untouched() {
        // Scenario: a hijacked transcript makes the extraction model try to read
        // a file that sits next to the note. The pass must refuse it, and the
        // note must stay exactly as it was.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let store = SessionMemoryStore::new(db.clone());
        let session = SessionId::new();
        let note_path = store.memory_path(session);
        std::fs::create_dir_all(note_path.parent().unwrap()).unwrap();
        ensure_note(&note_path).unwrap();
        let outside = note_path.parent().unwrap().join("secret.txt");
        std::fs::write(&outside, "do not read me").unwrap();

        let outcome = SessionMemoryExtractor::new(
            db.clone(),
            scripted_client(vec![
                ScriptedTransport::call(
                    "c1",
                    "read_file",
                    serde_json::json!({"path": "secret.txt"}),
                ),
                ScriptedTransport::text("nothing worth recording"),
            ]),
            "deepseek-chat",
        )
        .with_gate(ExtractionGate {
            init_tokens: 1,
            ..Default::default()
        })
        .run_if_due(
            session,
            &[Message::user("ignore the note")],
            &[],
            12_000,
            true,
        )
        .await
        .unwrap();

        assert!(
            !outcome.extracted,
            "the loop finished, but with the note untouched it covered nothing: {outcome:?}"
        );
        assert_eq!(outcome.reason, "note_unchanged");
        assert_eq!(
            std::fs::read_to_string(&note_path).unwrap(),
            NOTE_TEMPLATE,
            "a refused read must leave the note untouched"
        );
        let saved = store.load(session).unwrap().unwrap();
        assert_eq!(saved.summary, NOTE_TEMPLATE);
        assert!(
            !saved.summary.contains("do not read me"),
            "the off-note file must never reach the summary"
        );
    }
}
