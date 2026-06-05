//! Knowledge base management for the UI and the chat runtime.
//!
//! Wraps [`deepagent_knowledge::KnowledgeBase`] behind a `Mutex` (same pattern
//! as [`crate::skills_service::SkillsService`]) so it can be shared via `Arc`
//! between the Tauri command layer and the [`crate::chat_service::ChatService`]
//! (which uses it for passive injection and the active knowledge tools).
//!
//! It loads two vaults — project-local and user-global — into one index, and
//! exposes serializable DTOs the desktop app consumes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::Event;
use deepagent_core::message::Message;
use deepagent_knowledge::{
    capture, EntryKind, HashingEmbedder, KnowledgeBase, KnowledgeConfig, KnowledgeDraft,
    KnowledgeEntry, Scope, Vault,
};
use deepagent_models::chat::ChatRequest;
use deepagent_models::ModelClient;
use serde::{Deserialize, Serialize};

/// A serializable view of a knowledge entry for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDto {
    /// Composite id (`scope:slug`), stable across reloads.
    pub id: String,
    /// Entry title.
    pub title: String,
    /// Entry kind label (pitfall/solution/command/config/note).
    pub kind: String,
    /// Tags.
    pub tags: Vec<String>,
    /// Vault scope label (project/global).
    pub scope: String,
    /// Creation time (Unix ms).
    pub created_at: i64,
    /// Last-update time (Unix ms).
    pub updated_at: i64,
    /// Originating session id, if any.
    pub source_session: Option<String>,
    /// Markdown body.
    pub body: String,
}

impl KnowledgeDto {
    fn from_entry(entry: &KnowledgeEntry) -> Self {
        Self {
            id: format!("{}:{}", entry.scope.label(), entry.id),
            title: entry.title.clone(),
            kind: entry.kind.label().to_string(),
            tags: entry.tags.clone(),
            scope: entry.scope.label().to_string(),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            source_session: entry.source_session.clone(),
            body: entry.body.clone(),
        }
    }
}

/// A serializable search/tool hit for the UI and the active tool channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeHitDto {
    /// Composite id (`scope:slug`).
    pub id: String,
    /// Entry title.
    pub title: String,
    /// Entry kind label.
    pub kind: String,
    /// Vault scope label.
    pub scope: String,
    /// Relevance score in `[0,1]`.
    pub score: f32,
    /// Matching excerpt.
    pub excerpt: String,
}

/// A draft to create or update an entry (from the UI or a tool).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDraftDto {
    /// Entry title (dedup key within a scope).
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Kind label; unknown/empty defaults to `note`.
    #[serde(default)]
    pub kind: Option<String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Target scope label (`project` or `global`); defaults to project.
    #[serde(default)]
    pub scope: Option<String>,
    /// Originating session id, if any.
    #[serde(default)]
    pub source_session: Option<String>,
}

/// Parse a scope label, defaulting to [`Scope::Project`].
fn parse_scope(s: Option<&str>) -> Scope {
    match s.map(|x| x.trim().to_lowercase()).as_deref() {
        Some("global") => Scope::Global,
        _ => Scope::Project,
    }
}

/// UI- and runtime-facing knowledge base service.
pub struct KnowledgeService {
    inner: Mutex<KnowledgeState>,
    global_root: PathBuf,
}

struct KnowledgeState {
    project_root: PathBuf,
    base: KnowledgeBase<HashingEmbedder>,
}

impl KnowledgeService {
    /// Open project + global vaults and load them into one index.
    ///
    /// The project vault lives at `<project_root>/.deepagent/knowledge/`; the
    /// global vault at `<global_root>/knowledge/`. Neither needs to exist yet.
    pub fn open(project_root: &Path, global_root: &Path) -> Result<Self> {
        let base = Self::build_base(project_root, global_root, KnowledgeConfig::default())?;
        Ok(Self {
            inner: Mutex::new(KnowledgeState {
                project_root: project_root.to_path_buf(),
                base,
            }),
            global_root: global_root.to_path_buf(),
        })
    }

    fn build_base(
        project_root: &Path,
        global_root: &Path,
        config: KnowledgeConfig,
    ) -> Result<KnowledgeBase<HashingEmbedder>> {
        let project = Vault::new(
            project_root.join(".deepagent").join("knowledge"),
            Scope::Project,
        );
        let global = Vault::new(global_root.join("knowledge"), Scope::Global);
        let mut base =
            KnowledgeBase::new(vec![project, global], HashingEmbedder::default(), config);
        base.load_all()?;
        Ok(base)
    }

    /// Build over an already-constructed base (for tests / custom wiring).
    pub fn from_base(base: KnowledgeBase<HashingEmbedder>) -> Self {
        Self {
            inner: Mutex::new(KnowledgeState {
                project_root: PathBuf::new(),
                base,
            }),
            global_root: PathBuf::new(),
        }
    }

    /// Switch the project-local vault to `project_root`, preserving the global
    /// vault and runtime toggles. This keeps project-scoped knowledge aligned
    /// with the active project folder instead of the app launch directory.
    pub fn activate_project(&self, project_root: &Path) -> Result<()> {
        let mut state = self.lock();
        if state.project_root == project_root {
            return Ok(());
        }
        let config = state.base.config().clone();
        let base = Self::build_base(project_root, &self.global_root, config)?;
        *state = KnowledgeState {
            project_root: project_root.to_path_buf(),
            base,
        };
        Ok(())
    }

    /// Re-scan all vaults from disk and rebuild the index. Returns entry count.
    pub fn reload(&self) -> Result<usize> {
        let mut base = self.lock();
        base.base.load_all()
    }

    /// List all entries as DTOs.
    pub fn list(&self) -> Vec<KnowledgeDto> {
        let base = self.lock();
        base.base
            .list()
            .iter()
            .map(KnowledgeDto::from_entry)
            .collect()
    }

    /// Get one entry by composite id (`scope:slug`).
    pub fn get(&self, id: &str) -> Option<KnowledgeDto> {
        let base = self.lock();
        base.base.get(id).map(KnowledgeDto::from_entry)
    }

    /// Search the knowledge base, returning hit DTOs (best first).
    pub fn search(&self, query: &str, kind: Option<&str>, limit: usize) -> Vec<KnowledgeHitDto> {
        let kind = kind.filter(|s| !s.trim().is_empty()).map(EntryKind::parse);
        let base = self.lock();
        base.base
            .search(query, kind, limit)
            .into_iter()
            .map(|hit| KnowledgeHitDto {
                id: format!("{}:{}", hit.entry.scope.label(), hit.entry.id),
                title: hit.entry.title,
                kind: hit.entry.kind.label().to_string(),
                scope: hit.entry.scope.label().to_string(),
                score: hit.score,
                excerpt: hit.excerpt,
            })
            .collect()
    }

    /// Create or update an entry from a draft. Returns the persisted entry.
    pub fn save(&self, draft: KnowledgeDraftDto) -> Result<KnowledgeDto> {
        let kind = draft
            .kind
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(EntryKind::parse)
            .unwrap_or(EntryKind::Note);
        let scope = parse_scope(draft.scope.as_deref());
        let now_ms = SystemClock.now().as_millis();
        let kdraft = KnowledgeDraft {
            title: draft.title,
            body: draft.body,
            kind,
            tags: draft.tags,
            source_session: draft.source_session,
            scope,
        };
        let mut base = self.lock();
        let entry = base.base.write(kdraft, now_ms)?;
        Ok(KnowledgeDto::from_entry(&entry))
    }

    /// Delete an entry by composite id. Returns whether it existed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut base = self.lock();
        base.base.delete(id)
    }

    /// The passive-injection block for `query` (empty when nothing qualifies or
    /// passive injection is disabled).
    pub fn passive_block(&self, query: &str) -> String {
        let base = self.lock();
        base.base.passive_block(query)
    }

    /// Toggle passive injection.
    pub fn set_passive_enabled(&self, on: bool) {
        let mut base = self.lock();
        base.base.set_passive_enabled(on);
    }

    /// Whether passive injection is currently enabled.
    pub fn passive_enabled(&self) -> bool {
        let base = self.lock();
        base.base.config().passive_enabled
    }

    /// Toggle session auto-capture (recovery → active knowledge).
    pub fn set_auto_capture(&self, on: bool) {
        let mut base = self.lock();
        base.base.set_auto_capture_enabled(on);
    }

    /// Whether session auto-capture is currently enabled.
    pub fn auto_capture_enabled(&self) -> bool {
        let base = self.lock();
        base.base.config().auto_capture_enabled
    }

    /// List pending drafts as DTOs.
    pub fn list_drafts(&self) -> Vec<KnowledgeDto> {
        let base = self.lock();
        base.base
            .list_drafts()
            .iter()
            .map(KnowledgeDto::from_entry)
            .collect()
    }

    /// Accept a draft by composite id: promote it to an active entry.
    pub fn accept_draft(&self, id: &str) -> Result<KnowledgeDto> {
        let now_ms = SystemClock.now().as_millis();
        let mut base = self.lock();
        let entry = base.base.accept_draft(id, now_ms)?;
        Ok(KnowledgeDto::from_entry(&entry))
    }

    /// Discard a draft by composite id.
    pub fn discard_draft(&self, id: &str) -> Result<bool> {
        let mut base = self.lock();
        base.base.discard_draft(id)
    }

    /// Add a pending draft directly. Returns the draft.
    #[cfg(test)]
    fn add_draft(&self, draft: KnowledgeDraft) -> Result<KnowledgeDto> {
        let now_ms = SystemClock.now().as_millis();
        let mut base = self.lock();
        let entry = base.base.add_draft(draft, now_ms)?;
        Ok(KnowledgeDto::from_entry(&entry))
    }

    /// Auto-capture a worthwhile recovery from a finished session as an active
    /// knowledge entry, so the lesson can be passively injected on the next
    /// relevant turn. Returns `None` — silently, never erroring — when
    /// auto-capture is disabled or no failure/recovery was detected. If the
    /// summarizer fails or declines, a conservative fallback note is still
    /// persisted; a recovered tool failure is useful even when the model cannot
    /// summarize it cleanly.
    ///
    /// The `Mutex` is only held for the brief detect/insert steps, never across
    /// the `await` of the model call.
    pub async fn capture_from_session(
        &self,
        client: Arc<ModelClient>,
        model: String,
        events: &[Event],
        session_id: &str,
    ) -> Option<KnowledgeDto> {
        // Cheap, lock-free gate first.
        if !self.auto_capture_enabled() {
            return None;
        }
        let signal = capture::detect_recovery(events);
        if !capture::is_worth_capturing(&signal) {
            return None;
        }

        // Summarize via a small, non-streaming model call (no lock held). If it
        // fails or declines, keep the operational lesson with a deterministic
        // fallback so failures are not rediscovered from scratch next time.
        let draft = summarize_recovery(&client, &model, &signal, session_id)
            .await
            .unwrap_or_else(|| fallback_recovery_draft(&signal, session_id));

        let now_ms = SystemClock.now().as_millis();
        let mut base = self.lock();
        match base.base.write(draft, now_ms) {
            Ok(entry) => Some(KnowledgeDto::from_entry(&entry)),
            Err(e) => {
                tracing::warn!(error = %e, "failed to persist auto-capture knowledge");
                None
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, KnowledgeState> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl std::fmt::Debug for KnowledgeService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.inner.lock().map(|s| s.base.len()).unwrap_or(0);
        f.debug_struct("KnowledgeService")
            .field("entries", &n)
            .finish()
    }
}

/// Map a [`CoreError`] for callers that need a string message.
pub fn err_msg(e: &CoreError) -> String {
    e.to_string()
}

/// The summarization prompt instructs the model to either return a structured
/// draft or explicitly decline. Kept terse to keep the call cheap.
const CAPTURE_SYSTEM_PROMPT: &str = r#"You distill a coding session into at most ONE reusable knowledge note for a knowledge base. The session hit at least one tool failure and then recovered. Decide whether the experience is worth saving for the future (a non-obvious pitfall, a fix that worked, a useful command, or an important config). Mundane or one-off things are NOT worth saving.

Respond with ONLY a single JSON object, no prose, no code fences:
{"worth_saving": true|false, "title": "...", "kind": "pitfall|solution|command|config|note", "tags": ["..."], "body": "..."}

- If not worth saving, return {"worth_saving": false} and nothing else.
- title: specific and searchable.
- body: concise Markdown — the symptom and the resolution, self-contained.
- Keep it short. Do not invent details that are not in the session."#;

/// The parsed shape of the summarizer's JSON reply.
#[derive(Debug, Deserialize)]
struct CaptureReply {
    #[serde(default)]
    worth_saving: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    body: String,
}

fn fallback_recovery_draft(signal: &capture::RecoverySignal, session_id: &str) -> KnowledgeDraft {
    let tools = if signal.failed_tools.is_empty() {
        "tool".to_string()
    } else {
        signal.failed_tools.join(", ")
    };
    let mut tags = vec!["auto-captured".to_string(), "failure-recovery".to_string()];
    tags.extend(
        signal
            .failed_tools
            .iter()
            .map(|tool| tool.replace([' ', '_'], "-").to_lowercase()),
    );

    let mut body = format!(
        "## Symptom\nA previous run hit failed tool calls while handling this request:\n\n{}\n\n## Failed tools\n{}\n\n## What worked\nThe run recovered and produced a final answer. On a similar failure, do not stop at the first failed tool call; inspect the error, try an alternate query/source/tool, and preserve the workaround for reuse.",
        signal.user_goal.trim(),
        tools
    );

    if signal.failed_tools.iter().any(|tool| tool == "web_search") {
        body.push_str(
            "\n\nFor `web_search` failures such as unparseable results or provider markup changes, retry with a different query/source or use `web_fetch` against a known authoritative URL.",
        );
    }
    if !signal.final_answer.trim().is_empty() {
        body.push_str("\n\n## Recovered answer\n");
        body.push_str(signal.final_answer.trim());
    }
    if !signal.transcript_digest.trim().is_empty() {
        body.push_str("\n\n## Evidence\n");
        body.push_str(signal.transcript_digest.trim());
    }

    KnowledgeDraft {
        title: format!("Recovered from {tools} failure"),
        body,
        kind: EntryKind::Pitfall,
        tags,
        source_session: Some(session_id.to_string()),
        scope: Scope::Project,
    }
}

/// Run the summarization model call and parse a draft, or `None` on any failure
/// (model error, decline, empty, or unparseable). Never panics.
async fn summarize_recovery(
    client: &ModelClient,
    model: &str,
    signal: &capture::RecoverySignal,
    session_id: &str,
) -> Option<KnowledgeDraft> {
    let user = format!(
        "Here is the session digest:\n\n{}\n\nFailed tools: {}.\n\nReturn the JSON now.",
        signal.transcript_digest,
        signal.failed_tools.join(", ")
    );
    let request = ChatRequest::new(
        model,
        vec![Message::system(CAPTURE_SYSTEM_PROMPT), Message::user(&user)],
    )
    .with_temperature(0.2)
    .with_max_tokens(600);

    let response = match client.stream_chat(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "auto-capture summarization call failed");
            return None;
        }
    };

    let reply = parse_capture_reply(&response.message.content)?;
    if !reply.worth_saving || reply.title.trim().is_empty() || reply.body.trim().is_empty() {
        return None;
    }
    let kind = reply
        .kind
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(EntryKind::parse)
        .unwrap_or(EntryKind::Note);
    Some(KnowledgeDraft {
        title: reply.title,
        body: reply.body,
        kind,
        tags: reply.tags,
        source_session: Some(session_id.to_string()),
        scope: Scope::Project,
    })
}

/// Extract the JSON object from a model reply, tolerating code fences or
/// surrounding prose by slicing the outermost `{...}`.
fn parse_capture_reply(raw: &str) -> Option<CaptureReply> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end < start {
        return None;
    }
    let json = &raw[start..=end];
    serde_json::from_str::<CaptureReply>(json).ok()
}

/// Adapts an [`Arc<KnowledgeService>`] to the built-in [`KnowledgeBackend`]
/// trait so the `knowledge_search` / `knowledge_write` tools can reach the real
/// vault. The service methods are synchronous (Mutex-backed); the trait is
/// async, so each call simply forwards to the sync method.
#[derive(Clone)]
pub struct KnowledgeServiceBackend {
    service: std::sync::Arc<KnowledgeService>,
}

impl KnowledgeServiceBackend {
    /// Wrap a shared knowledge service.
    pub fn new(service: std::sync::Arc<KnowledgeService>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl deepagent_builtins::KnowledgeBackend for KnowledgeServiceBackend {
    async fn search(
        &self,
        query: &str,
        kind: Option<String>,
        limit: usize,
    ) -> Result<Vec<deepagent_builtins::KnowledgeToolHit>> {
        let hits = self.service.search(query, kind.as_deref(), limit);
        Ok(hits
            .into_iter()
            .map(|h| deepagent_builtins::KnowledgeToolHit {
                id: h.id,
                title: h.title,
                kind: h.kind,
                scope: h.scope,
                score: h.score,
                excerpt: h.excerpt,
            })
            .collect())
    }

    async fn write(&self, draft: deepagent_builtins::KnowledgeToolDraft) -> Result<String> {
        let dto = self.service.save(KnowledgeDraftDto {
            title: draft.title,
            body: draft.content,
            kind: draft.kind,
            tags: draft.tags,
            scope: None, // tool-captured knowledge lands in the project vault
            source_session: None,
        })?;
        Ok(dto.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(tmp: &Path) -> KnowledgeService {
        KnowledgeService::open(&tmp.join("proj"), &tmp.join("glob")).unwrap()
    }

    fn draft(title: &str, body: &str, kind: &str, scope: &str) -> KnowledgeDraftDto {
        KnowledgeDraftDto {
            title: title.to_string(),
            body: body.to_string(),
            kind: Some(kind.to_string()),
            tags: vec!["t".into()],
            scope: Some(scope.to_string()),
            source_session: None,
        }
    }

    #[test]
    fn open_save_list_get() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        assert!(svc.list().is_empty());

        let saved = svc
            .save(draft(
                "Cargo offline tests",
                "Run cargo test --workspace --offline to test the backend.",
                "command",
                "project",
            ))
            .unwrap();
        assert_eq!(saved.kind, "command");
        assert_eq!(saved.scope, "project");
        assert!(saved.id.starts_with("project:"));

        let list = svc.list();
        assert_eq!(list.len(), 1);
        let got = svc.get(&saved.id).unwrap();
        assert_eq!(got.title, "Cargo offline tests");
    }

    #[test]
    fn save_updates_existing_on_same_title() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        let first = svc
            .save(draft("Same", "v1 body", "note", "project"))
            .unwrap();
        let second = svc
            .save(draft("same", "v2 body changed", "solution", "project"))
            .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(svc.list().len(), 1);
        assert!(second.body.contains("v2 body changed"));
    }

    #[test]
    fn search_returns_hits() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        svc.save(draft(
            "Keyring on Windows",
            "Credential stored under service deepagent-studio in Credential Manager.",
            "config",
            "project",
        ))
        .unwrap();
        let hits = svc.search("windows credential manager", None, 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].title, "Keyring on Windows");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn delete_removes_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        let e = svc.save(draft("Gone", "body", "note", "project")).unwrap();
        assert!(svc.delete(&e.id).unwrap());
        assert!(svc.list().is_empty());
        assert!(!svc.delete(&e.id).unwrap());
    }

    #[test]
    fn passive_toggle_controls_block() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        svc.save(draft(
            "Deployment steps",
            "deploy by running the pipeline with the release profile",
            "note",
            "project",
        ))
        .unwrap();
        // Enabled (default): a relevant query yields a block.
        let on = svc.passive_block("deployment pipeline release");
        assert!(!on.is_empty());
        // Disabled: always empty.
        svc.set_passive_enabled(false);
        assert!(!svc.passive_enabled());
        assert!(svc.passive_block("deployment pipeline release").is_empty());
    }

    #[test]
    fn reload_reads_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        svc.save(draft("Persisted", "body", "note", "global"))
            .unwrap();
        // A second service over the same dirs sees the persisted entry.
        let svc2 = service(tmp.path());
        assert_eq!(svc2.list().len(), 1);
        assert_eq!(svc2.reload().unwrap(), 1);
    }

    #[test]
    fn activate_project_switches_project_vault_but_keeps_global() {
        let tmp = tempfile::tempdir().unwrap();
        let global_root = tmp.path().join("glob");
        let project_a = tmp.path().join("a");
        let project_b = tmp.path().join("b");
        let svc = KnowledgeService::open(&project_a, &global_root).unwrap();
        svc.save(draft(
            "Project A only",
            "This note belongs to project A",
            "note",
            "project",
        ))
        .unwrap();
        svc.save(draft(
            "Global note",
            "This note should follow every project",
            "note",
            "global",
        ))
        .unwrap();

        svc.activate_project(&project_b).unwrap();
        let titles: Vec<_> = svc.list().into_iter().map(|e| e.title).collect();
        assert!(!titles.iter().any(|t| t == "Project A only"));
        assert!(titles.iter().any(|t| t == "Global note"));
    }

    // ---- auto-capture --------------------------------------------------

    use deepagent_core::clock::Timestamp;
    use deepagent_core::event::{Event, EventPayload};
    use deepagent_core::id::{EventId, SessionId, TaskId};
    use deepagent_core::task::TaskState;
    use deepagent_models::transport::MockTransport;
    use deepagent_models::ModelConfig;

    fn ev(seq: u64, payload: EventPayload) -> Event {
        Event {
            id: EventId::new(),
            session_id: SessionId::new(),
            sequence: seq,
            timestamp: Timestamp::from_millis(seq as i64),
            payload,
        }
    }

    /// A run with a tool failure followed by completion (a recovery arc).
    fn recovery_events() -> Vec<Event> {
        use deepagent_core::message::ToolCall;
        vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("fix the os error 5 when rebuilding tauri"),
                },
            ),
            ev(
                1,
                EventPayload::ToolCallRequested {
                    call: ToolCall {
                        id: "c1".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({}),
                    },
                },
            ),
            ev(
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: false,
                    output: serde_json::json!({"error": "os error 5: access denied"}),
                    duration_ms: 3,
                },
            ),
            ev(
                3,
                EventPayload::MessageAppended {
                    message: Message::assistant(
                        "Close the desktop window first; it locks the exe.",
                    ),
                },
            ),
            ev(
                4,
                EventPayload::TaskStateChanged {
                    task_id: TaskId::new(),
                    from: TaskState::Running,
                    to: TaskState::Completed,
                },
            ),
        ]
    }

    fn client_streaming(content_json: &str) -> Arc<ModelClient> {
        // Stream the JSON reply as one content delta then [DONE].
        let payload = serde_json::json!({
            "choices": [{"delta": {"content": content_json}, "finish_reason": "stop"}]
        })
        .to_string();
        let transport = Arc::new(MockTransport::new([payload, "[DONE]".to_string()]));
        Arc::new(ModelClient::new(
            transport,
            ModelConfig::deepseek("sk-test"),
        ))
    }

    #[tokio::test]
    async fn capture_creates_active_knowledge_on_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        let client = client_streaming(
            r#"{"worth_saving": true, "title": "Tauri exe lock on rebuild", "kind": "pitfall", "tags": ["tauri","windows"], "body": "Close the window before rebuilding to avoid os error 5."}"#,
        );
        let events = recovery_events();
        let dto = svc
            .capture_from_session(client, "deepseek-v4-flash".into(), &events, "ses_xyz")
            .await;
        assert!(dto.is_some());
        let dto = dto.unwrap();
        assert_eq!(dto.title, "Tauri exe lock on rebuild");
        assert_eq!(dto.source_session.as_deref(), Some("ses_xyz"));
        // Auto-captured knowledge is active so passive injection can reuse it
        // on the next relevant turn.
        assert_eq!(svc.list().len(), 1);
        assert!(svc.list_drafts().is_empty());
    }

    #[tokio::test]
    async fn capture_falls_back_when_model_declines() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        let client = client_streaming(r#"{"worth_saving": false}"#);
        let dto = svc
            .capture_from_session(client, "deepseek-v4-flash".into(), &recovery_events(), "s")
            .await;
        assert!(dto.is_some());
        let dto = dto.unwrap();
        assert_eq!(dto.title, "Recovered from bash failure");
        assert_eq!(dto.kind, "pitfall");
        assert!(dto.body.contains("os error 5"));
        assert_eq!(svc.list().len(), 1);
        assert!(svc.list_drafts().is_empty());
    }

    #[tokio::test]
    async fn capture_skips_trivial_run_without_calling_model() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        // A trivial run: no tool failure. Model would never be consulted; use a
        // transport that returns nothing useful to prove it isn't needed.
        let client = client_streaming(r#"{"worth_saving": true, "title": "x", "body": "y"}"#);
        let events = vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user("hi"),
                },
            ),
            ev(
                1,
                EventPayload::MessageAppended {
                    message: Message::assistant("hello"),
                },
            ),
            ev(
                2,
                EventPayload::TaskStateChanged {
                    task_id: TaskId::new(),
                    from: TaskState::Running,
                    to: TaskState::Completed,
                },
            ),
        ];
        let dto = svc
            .capture_from_session(client, "deepseek-v4-flash".into(), &events, "s")
            .await;
        assert!(dto.is_none());
        assert!(svc.list_drafts().is_empty());
    }

    #[tokio::test]
    async fn capture_skips_when_disabled() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        svc.set_auto_capture(false);
        assert!(!svc.auto_capture_enabled());
        let client = client_streaming(r#"{"worth_saving": true, "title": "x", "body": "y"}"#);
        let dto = svc
            .capture_from_session(client, "deepseek-v4-flash".into(), &recovery_events(), "s")
            .await;
        assert!(dto.is_none());
    }

    #[test]
    fn accept_and_discard_drafts() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        // Seed two drafts directly via the private helper through add_draft path:
        // use save() is for active; emulate via capture path is async, so test
        // accept/discard against drafts produced by add_draft.
        let d1 = svc
            .add_draft(KnowledgeDraft {
                title: "Draft A".into(),
                body: "body a".into(),
                kind: EntryKind::Note,
                tags: vec![],
                source_session: Some("ses1".into()),
                scope: Scope::Project,
            })
            .unwrap();
        let d2 = svc
            .add_draft(KnowledgeDraft {
                title: "Draft B".into(),
                body: "body b".into(),
                kind: EntryKind::Note,
                tags: vec![],
                source_session: None,
                scope: Scope::Project,
            })
            .unwrap();
        assert_eq!(svc.list_drafts().len(), 2);

        // Accept one → becomes active, leaves drafts.
        let accepted = svc.accept_draft(&d1.id).unwrap();
        assert_eq!(accepted.source_session.as_deref(), Some("ses1"));
        assert_eq!(svc.list().len(), 1);
        assert_eq!(svc.list_drafts().len(), 1);

        // Discard the other → gone, no active added.
        assert!(svc.discard_draft(&d2.id).unwrap());
        assert!(svc.list_drafts().is_empty());
        assert_eq!(svc.list().len(), 1);
    }
}
