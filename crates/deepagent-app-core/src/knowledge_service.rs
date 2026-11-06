//! Knowledge base management for the UI and the chat runtime.
//!
//! Wraps [`deepagent_knowledge::KnowledgeBase`] behind a `Mutex` (same pattern
//! as [`crate::skills_service::SkillsService`]) so it can be shared via `Arc`
//! between the Tauri command layer and the [`crate::chat_service::ChatService`]
//! (which uses it for passive injection and the active knowledge tools).
//!
//! It loads two vaults — project-local and user-global — into one index, and
//! exposes serializable DTOs the desktop app consumes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::{CoreError, Result};
use deepagent_core::event::Event;
use deepagent_knowledge::{
    capture, EntryKind, HashingEmbedder, KnowledgeBase, KnowledgeConfig, KnowledgeDraft,
    KnowledgeEntry, Scope, Vault,
};
use deepagent_models::chat::ResponseRequest;
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
    /// Memory tier for `kind == "memory"` entries; `None` for ordinary notes.
    #[serde(default)]
    pub tier: Option<String>,
    /// Baseline importance used by the `memory_recall` ranking.
    #[serde(default)]
    pub importance: Option<f32>,
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
            tier: entry.tier.clone(),
            importance: entry.importance,
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
    /// Entry last-update time (Unix ms), carried so the injection path can add
    /// the shared staleness caveat without re-reading the vault.
    #[serde(default)]
    pub updated_at: i64,
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
    /// Target scope label (`project` or `global`); defaults to global so
    /// knowledge follows the user across projects unless explicitly scoped.
    #[serde(default)]
    pub scope: Option<String>,
    /// Originating session id, if any.
    #[serde(default)]
    pub source_session: Option<String>,
    /// Memory tier (only meaningful for `kind == "memory"`).
    #[serde(default)]
    pub tier: Option<String>,
    /// Baseline importance for the recall ranking.
    #[serde(default)]
    pub importance: Option<f32>,
}

/// Resolve the user-global vault root for hosts that have no platform
/// app-data directory (headless CLI, app-server). The desktop passes the Tauri
/// resolver's directory instead, so both hosts read and write the *same* vault
/// a memory was written to — a memory stored per-workspace would be invisible to
/// every other project, which is exactly the split this layer just removed.
///
/// `DEEPAGENT_HOME` wins; otherwise `$HOME` (POSIX) or `%USERPROFILE%` (Windows)
/// with `.deepagent` appended. Resolution is a pure function of the environment
/// lookup so it can be tested without mutating process globals.
pub fn global_vault_root() -> Result<std::path::PathBuf> {
    global_vault_root_from(|name| std::env::var_os(name))
}

fn global_vault_root_from(
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(root) = lookup("DEEPAGENT_HOME") {
        let root = PathBuf::from(root);
        if !root.as_os_str().is_empty() {
            return Ok(root);
        }
    }
    for name in ["HOME", "USERPROFILE"] {
        if let Some(home) = lookup(name) {
            let home = PathBuf::from(home);
            if !home.as_os_str().is_empty() {
                return Ok(home.join(".deepagent"));
            }
        }
    }
    Err(CoreError::invalid(
        "cannot resolve a user-global knowledge vault: set DEEPAGENT_HOME or HOME",
    ))
}

/// Parse a scope label, defaulting to [`Scope::Global`] so saved knowledge is
/// shared across every project unless the caller explicitly opts into the
/// project-local vault.
fn parse_scope(s: Option<&str>) -> Scope {
    match s.map(|x| x.trim().to_lowercase()).as_deref() {
        Some("project") => Scope::Project,
        _ => Scope::Global,
    }
}

/// UI selection and stable runtime views of the same vault-backed service.
///
/// Local design: one service mutex owns every cached project index. Bound handles
/// never consult the mutable UI selection. Mutations publish the shared global
/// scope to the other derived indexes before releasing that lock, including
/// partial changes when a vault operation returns an error. No model await is
/// performed while holding it; independent hosts remain a separate boundary.
pub struct KnowledgeService {
    inner: Arc<Mutex<KnowledgeState>>,
    project: Option<u64>,
}

struct KnowledgeState {
    active: u64,
    bases: BTreeMap<u64, ProjectKnowledge>,
    // IDs, unlike path names, cannot be reused by a different physical root.
    // Merged pending handles point directly to a surviving cache, never chains.
    aliases: BTreeMap<u64, u64>,
    next_project: u64,
    // Custom from_base wiring is already fixed and has no retargetable roots.
    global_root: Option<PathBuf>,
}

struct ProjectKnowledge {
    root: PathBuf,
    materialized: bool,
    base: KnowledgeBase<HashingEmbedder>,
}

fn knowledge_root(path: &Path) -> Result<(PathBuf, bool)> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CoreError::Persistence(format!("resolve knowledge root: {error}")))?
            .join(path)
    };
    let resolved = deepagent_persistence::fs_commit::canonicalize_with_missing_suffix(&absolute)
        .map_err(|error| CoreError::Persistence(format!("resolve knowledge root: {error}")))?;
    // Windows may report a suffix below a regular file as NotFound. The shared
    // path helper resolves identity, not the directory contract of a vault.
    for (depth, ancestor) in resolved.ancestors().enumerate() {
        match std::fs::metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => return Ok((resolved, depth == 0)),
            Ok(_) => return Err(CoreError::invalid("knowledge root must be a directory")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CoreError::Persistence(format!(
                    "inspect knowledge root: {error}"
                )))
            }
        }
    }
    Err(CoreError::invalid(
        "knowledge root has no existing directory ancestor",
    ))
}

impl KnowledgeService {
    /// Open project + global vaults and load them into one index.
    ///
    /// The project vault lives at `<project_root>/.deepagent/knowledge/`; the
    /// global vault at `<global_root>/knowledge/`. Neither needs to exist yet.
    pub fn open(project_root: &Path, global_root: &Path) -> Result<Self> {
        let (project_root, materialized) = knowledge_root(project_root)?;
        let (global_root, _) = knowledge_root(global_root)?;
        let base = Self::build_base(&project_root, &global_root, KnowledgeConfig::default())?;
        Ok(Self {
            inner: Arc::new(Mutex::new(KnowledgeState {
                active: 0,
                bases: BTreeMap::from([(
                    0,
                    ProjectKnowledge {
                        root: project_root,
                        materialized,
                        base,
                    },
                )]),
                aliases: BTreeMap::new(),
                next_project: 1,
                global_root: Some(global_root),
            })),
            project: None,
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
            inner: Arc::new(Mutex::new(KnowledgeState {
                active: 0,
                bases: BTreeMap::from([(
                    0,
                    ProjectKnowledge {
                        root: PathBuf::new(),
                        materialized: true,
                        base,
                    },
                )]),
                aliases: BTreeMap::new(),
                next_project: 1,
                global_root: None,
            })),
            project: Some(0),
        }
    }

    /// Select the UI's project without retargeting any existing runtime handle.
    /// Bound views cannot change selection. Closing a sidebar project does not
    /// invalidate a live run's cache or turn its writes into another project.
    pub fn activate_project(&self, project_root: &Path) -> Result<()> {
        if self.project.is_some() {
            return Err(CoreError::invalid(
                "a bound knowledge view cannot activate a project",
            ));
        }
        let (project, materialized) = knowledge_root(project_root)?;
        let mut state = self.lock();
        let (project, created) = Self::ensure_project(&mut state, &project, materialized)?;
        if state.active != project && !created {
            // Preserve the UI's existing rescan-on-switch behavior, but update
            // the shared view rather than replacing any run's project identity.
            Self::mutate_project(&mut state, project, KnowledgeBase::load_all)?;
        }
        state.active = Self::project_key(&state, project);
        Ok(())
    }

    /// Bind an explicit run project, independently of the sidebar selection.
    /// A custom `from_base` service is already bound by its caller and is reused
    /// without guessing vault paths. Opening a new view can fail; reads never
    /// hide reload errors or silently fall back to the currently active project.
    pub fn for_project(&self, project_root: &Path) -> Result<Arc<Self>> {
        let mut state = self.lock();
        let project = if state.global_root.is_none() {
            state.active
        } else {
            let (project, materialized) = knowledge_root(project_root)?;
            Self::ensure_project(&mut state, &project, materialized)?.0
        };
        Ok(Arc::new(Self {
            inner: self.inner.clone(),
            project: Some(project),
        }))
    }

    /// Capture the current view without filesystem I/O. Runtime adapters use
    /// this at construction, not on every later call or after a model await.
    pub fn bind_current(&self) -> Arc<Self> {
        let state = self.lock();
        Arc::new(Self {
            inner: self.inner.clone(),
            project: Some(Self::project_key(
                &state,
                self.project.unwrap_or(state.active),
            )),
        })
    }

    fn project_key(state: &KnowledgeState, project: u64) -> u64 {
        state.aliases.get(&project).copied().unwrap_or(project)
    }

    /// Resolve only an as-yet-missing operation root and its physical peers.
    /// Filesystem identity (not case-folding) decides which caches can merge.
    /// Rebuild the vault at its physical path as well as redirecting old keys:
    /// retaining a logical symlink in the vault would still retarget late I/O.
    /// Unrelated invalid pending roots cannot block a healthy project's work.
    fn reconcile_project(state: &mut KnowledgeState, project: u64) -> Result<u64> {
        let project = Self::project_key(state, project);
        let cached = &state.bases[&project];
        if cached.materialized {
            return Ok(project);
        }
        let (physical, materialized) = knowledge_root(&cached.root)?;
        if !materialized {
            return Ok(project);
        }
        let mut peers = Vec::new();
        for (&id, pending) in state
            .bases
            .iter()
            .filter(|(_, cached)| !cached.materialized)
        {
            match knowledge_root(&pending.root) {
                Ok((path, true)) if path == physical => peers.push(id),
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    project = %pending.root.display(), %error,
                    "cannot resolve unrelated pending knowledge project"
                ),
            }
        }
        let target = state
            .bases
            .iter()
            .find(|(_, cached)| cached.materialized && cached.root == physical)
            .map(|(&id, _)| id)
            .unwrap_or(project);
        let global = state
            .global_root
            .clone()
            .ok_or_else(|| CoreError::invalid("custom knowledge view has no roots"))?;
        Self::mutate_base(state, target, |base| {
            // Materializing a path changes its physical binding, not what this
            // host has observed. An implicit reload here would authorize a stale
            // same-title replacement of another host's first write (H-52).
            let project = base.scope_snapshot(Scope::Project);
            let global_snapshot = base.scope_snapshot(Scope::Global);
            let mut rebound = Self::build_base(&physical, &global, base.config().clone())?;
            rebound.apply_scope_snapshot(&project);
            rebound.apply_scope_snapshot(&global_snapshot);
            *base = rebound;
            Ok(())
        })?;
        let cached = state.bases.get_mut(&target).expect("project cache exists");
        cached.root = physical;
        cached.materialized = true;
        for alias in state.aliases.values_mut() {
            if peers.contains(alias) {
                *alias = target;
            }
        }
        if peers.contains(&state.active) {
            state.active = target;
        }
        for peer in peers {
            if peer != target {
                state.bases.remove(&peer);
                state.aliases.insert(peer, target);
            }
        }
        Ok(target)
    }

    fn ensure_project(
        state: &mut KnowledgeState,
        project: &Path,
        materialized: bool,
    ) -> Result<(u64, bool)> {
        // Only pending roots resolving to the requested physical project take
        // part. Other unreadable projects must not poison a healthy binding.
        let pending: Vec<u64> = state.bases.iter()
            .filter(|(_, cached)| !cached.materialized)
            .filter(|(_, cached)| matches!(knowledge_root(&cached.root), Ok((path, true)) if path == project))
            .map(|(&id, _)| id)
            .collect();
        for id in pending {
            Self::reconcile_project(state, id)?;
        }
        if let Some((&id, _)) = state
            .bases
            .iter()
            .find(|(_, cached)| cached.root == project)
        {
            return Ok((id, false));
        }
        let current = &state.bases[&state.active].base;
        let base = Self::build_base(
            project,
            state
                .global_root
                .as_deref()
                .ok_or_else(|| CoreError::invalid("custom knowledge view has no roots"))?,
            current.config().clone(),
        )?;
        let id = state.next_project;
        state.next_project = id
            .checked_add(1)
            .ok_or_else(|| CoreError::other("knowledge project identity exhausted"))?;
        // Markdown remains the truth. A successful disk scan publishes its
        // global projection to every already-open view under the same lock.
        let global = base.scope_snapshot(Scope::Global);
        for cached in state.bases.values_mut() {
            cached.base.apply_scope_snapshot(&global);
        }
        state.bases.insert(
            id,
            ProjectKnowledge {
                root: project.to_path_buf(),
                materialized,
                base,
            },
        );
        Ok((id, true))
    }

    fn read<T>(&self, read: impl FnOnce(&KnowledgeBase<HashingEmbedder>) -> T) -> T {
        let state = self.lock();
        let project = Self::project_key(&state, self.project.unwrap_or(state.active));
        read(&state.bases[&project].base)
    }

    fn mutate<T>(
        &self,
        mutate: impl FnOnce(&mut KnowledgeBase<HashingEmbedder>) -> Result<T>,
    ) -> Result<T> {
        let mut state = self.lock();
        let project = self.project.unwrap_or(state.active);
        Self::mutate_project(&mut state, project, mutate)
    }

    fn mutate_project<T>(
        state: &mut KnowledgeState,
        project: u64,
        mutate: impl FnOnce(&mut KnowledgeBase<HashingEmbedder>) -> Result<T>,
    ) -> Result<T> {
        let project = Self::reconcile_project(state, project)?;
        let result = Self::mutate_base(state, project, mutate);
        match (result, Self::reconcile_project(state, project)) {
            (result, Ok(_)) => result,
            (Ok(_), Err(error)) => Err(error),
            (Err(operation), Err(identity)) => Err(CoreError::other(format!(
                "{operation}; refresh knowledge project identity: {identity}"
            ))),
        }
    }

    /// The sole cached-base mutation/publication boundary. Identity merging
    /// uses the same boundary for its disk reload, without recursing into it.
    fn mutate_base<T>(
        state: &mut KnowledgeState,
        project: u64,
        mutate: impl FnOnce(&mut KnowledgeBase<HashingEmbedder>) -> Result<T>,
    ) -> Result<T> {
        let base = state
            .bases
            .get_mut(&project)
            .map(|cached| &mut cached.base)
            .expect("bound knowledge view exists");
        let before = base.scope_snapshot(Scope::Global);
        let result = mutate(base);
        let after = base.scope_snapshot(Scope::Global);
        if before != after {
            for (key, cached) in &mut state.bases {
                if *key != project {
                    cached.base.apply_scope_snapshot(&after);
                }
            }
        }
        result
    }

    /// Re-scan all vaults from disk and rebuild the index. Returns entry count.
    pub fn reload(&self) -> Result<usize> {
        self.mutate(KnowledgeBase::load_all)
    }

    /// List all entries as DTOs.
    pub fn list(&self) -> Vec<KnowledgeDto> {
        self.read(|base| base.list().iter().map(KnowledgeDto::from_entry).collect())
    }

    /// Get one entry by composite id (`scope:slug`).
    pub fn get(&self, id: &str) -> Option<KnowledgeDto> {
        self.read(|base| base.get(id).map(KnowledgeDto::from_entry))
    }

    /// Search the knowledge base, returning hit DTOs (best first).
    pub fn search(&self, query: &str, kind: Option<&str>, limit: usize) -> Vec<KnowledgeHitDto> {
        let kind = kind.filter(|s| !s.trim().is_empty()).map(EntryKind::parse);
        self.read(|base| {
            base.search(query, kind, limit)
                .into_iter()
                .map(|hit| KnowledgeHitDto {
                    id: format!("{}:{}", hit.entry.scope.label(), hit.entry.id),
                    title: hit.entry.title,
                    kind: hit.entry.kind.label().to_string(),
                    scope: hit.entry.scope.label().to_string(),
                    score: hit.score,
                    excerpt: hit.excerpt,
                    updated_at: hit.entry.updated_at,
                })
                .collect()
        })
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
            tier: draft.tier,
            importance: draft.importance,
        };
        let entry = self.mutate(|base| base.write(kdraft, now_ms))?;
        Ok(KnowledgeDto::from_entry(&entry))
    }

    /// Delete an entry by composite id. Returns whether it existed.
    pub fn delete(&self, id: &str) -> Result<bool> {
        self.mutate(|base| base.delete(id))
    }

    /// How many entries both vaults hold, which is what the consolidation gate
    /// measures before spending a scan.
    pub fn entry_count(&self) -> usize {
        self.read(KnowledgeBase::len)
    }

    /// Fold duplicate entries across both vaults and archive the losers.
    ///
    /// Synchronous by design: the merge is local file work with no model call, so
    /// it holds the service mutex only while it rewrites, and there is no
    /// background task to drain on shutdown.
    pub fn consolidate(
        &self,
        exclude_session: Option<&str>,
        now_millis: i64,
    ) -> Result<deepagent_knowledge::ConsolidationReport> {
        self.mutate(|base| base.consolidate(exclude_session, now_millis))
    }

    /// The passive-injection block for `query` (empty when nothing qualifies or
    /// passive injection is disabled).
    pub fn passive_block(&self, query: &str) -> String {
        self.read(|base| base.passive_block(query))
    }

    /// Toggle passive injection.
    pub fn set_passive_enabled(&self, on: bool) {
        for cached in self.lock().bases.values_mut() {
            cached.base.set_passive_enabled(on);
        }
    }

    /// Whether passive injection is currently enabled.
    pub fn passive_enabled(&self) -> bool {
        self.read(|base| base.config().passive_enabled)
    }

    /// Toggle session auto-capture (recovery → active knowledge).
    pub fn set_auto_capture(&self, on: bool) {
        for cached in self.lock().bases.values_mut() {
            cached.base.set_auto_capture_enabled(on);
        }
    }

    /// Whether session auto-capture is currently enabled.
    pub fn auto_capture_enabled(&self) -> bool {
        self.read(|base| base.config().auto_capture_enabled)
    }

    /// List pending drafts as DTOs.
    pub fn list_drafts(&self) -> Vec<KnowledgeDto> {
        self.read(|base| {
            base.list_drafts()
                .iter()
                .map(KnowledgeDto::from_entry)
                .collect()
        })
    }

    /// Accept a draft by composite id: promote it to an active entry.
    pub fn accept_draft(&self, id: &str) -> Result<KnowledgeDto> {
        let now_ms = SystemClock.now().as_millis();
        let entry = self.mutate(|base| base.accept_draft(id, now_ms))?;
        Ok(KnowledgeDto::from_entry(&entry))
    }

    /// Discard a draft by composite id.
    pub fn discard_draft(&self, id: &str) -> Result<bool> {
        self.mutate(|base| base.discard_draft(id))
    }

    /// Add a pending draft directly. Returns the draft.
    #[cfg(test)]
    fn add_draft(&self, draft: KnowledgeDraft) -> Result<KnowledgeDto> {
        let now_ms = SystemClock.now().as_millis();
        let entry = self.mutate(|base| base.add_draft(draft, now_ms))?;
        Ok(KnowledgeDto::from_entry(&entry))
    }

    /// Auto-capture a worthwhile session as an active knowledge entry so the
    /// lesson can be passively injected next time. Two paths run on every
    /// completed session, in order:
    ///
    /// 1. **Recovery arc** — if the run had a tool failure that was overcome,
    ///    summarize that arc (with a deterministic fallback if the model
    ///    declines). Operationally important; never silently dropped.
    /// 2. **Generic session digest** — for any other substantive run, ask the
    ///    model whether anything is worth saving. The model's `worth_saving`
    ///    reply is the only gate; trivial chats are filtered locally before
    ///    any model call to avoid spend.
    ///
    /// Returns `None` (silently, never erroring) when auto-capture is off, the
    /// session is not substantive, or both paths declined.
    ///
    /// The `Mutex` is only held for the brief insert step, never across the
    /// `await` of a model call.
    pub async fn capture_from_session(
        &self,
        client: Arc<ModelClient>,
        model: String,
        events: &[Event],
        session_id: &str,
    ) -> Option<KnowledgeDto> {
        let bound = self.bind_current();
        let service = bound.as_ref();
        // Cheap gate before any model call; the lock is released immediately.
        if !service.auto_capture_enabled() {
            return None;
        }

        // Path 1: recovery arc. Strong signal with a deterministic fallback so
        // a recovered failure is never lost even when the model declines.
        let signal = capture::detect_recovery(events);
        if capture::is_worth_capturing(&signal) {
            let draft = summarize_recovery(&client, &model, &signal, session_id)
                .await
                .unwrap_or_else(|| fallback_recovery_draft(&signal, session_id));
            return service.persist_capture(draft);
        }

        // Path 2: generic session digest. Local substantiveness gate filters
        // out trivial chatter before any network call; the model's
        // `worth_saving` reply is the only quality gate (no fallback so the
        // vault stays clean).
        let digest = capture::detect_session_digest(events);
        if !capture::is_session_substantive(&digest) {
            return None;
        }
        let draft = summarize_session_digest(&client, &model, &digest, session_id).await?;
        service.persist_capture(draft)
    }

    /// Persist an auto-capture draft into the vault. Returns the persisted
    /// entry DTO, or `None` (with a warning log) on a write error.
    fn persist_capture(&self, draft: KnowledgeDraft) -> Option<KnowledgeDto> {
        let now_ms = SystemClock.now().as_millis();
        match self.mutate(|base| base.write(draft, now_ms)) {
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
        let n = self.read(KnowledgeBase::len);
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
        // Auto-captured recovery experience is reusable across every project,
        // so it lands in the global vault.
        scope: Scope::Global,
        tier: None,
        importance: None,
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
    let request =
        ResponseRequest::with_instructions_and_user_input(model, CAPTURE_SYSTEM_PROMPT, &user)
            .with_temperature(0.2)
            .with_max_output_tokens(600);

    let response = match client.stream_response(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "auto-capture summarization call failed");
            return None;
        }
    };

    let reply = parse_capture_reply(&response.output_text_projection())?;
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
        // Lessons distilled from a recovery arc are reusable across every
        // project; they belong in the global vault.
        scope: Scope::Global,
        tier: None,
        importance: None,
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

/// Generic session-digest summarization prompt. Used by the second auto-capture
/// path that runs on every substantive session, regardless of whether failures
/// happened. The model is biased toward NOT saving and decides via the
/// `worth_saving` field; the bar is concrete, transferable knowledge.
const DIGEST_SYSTEM_PROMPT: &str = r#"You distill a coding session into AT MOST ONE reusable knowledge note for a knowledge base. Decide whether anything in this session is genuinely worth saving for future reuse — a non-obvious pitfall, a fix that worked, a useful command, an important config, or a notable design decision. Mundane chatter, one-off questions, and things easily rediscovered are NOT worth saving.

Bias toward NOT saving. Only save when there is concrete, transferable knowledge that a future session would benefit from seeing automatically.

Respond with ONLY a single JSON object, no prose, no code fences:
{"worth_saving": true|false, "title": "...", "kind": "pitfall|solution|command|config|note", "tags": ["..."], "body": "..."}

- If not worth saving, return {"worth_saving": false} and nothing else.
- title: specific and searchable; never generic ("session summary", "knowledge", etc.).
- body: concise Markdown — the situation, the fact/fix, and how to apply it. Self-contained.
- Keep it short. Do not invent details that are not in the session."#;

/// Run the generic session-digest summarization model call. Returns `None` on
/// any failure or when the model declines (`worth_saving: false`). Never
/// fabricates a fallback — the model is the only quality gate for this path.
async fn summarize_session_digest(
    client: &ModelClient,
    model: &str,
    digest: &capture::SessionDigest,
    session_id: &str,
) -> Option<KnowledgeDraft> {
    let user = format!(
        "Here is the session digest:\n\n{}\n\nReturn the JSON now.",
        digest.transcript_digest
    );
    let request =
        ResponseRequest::with_instructions_and_user_input(model, DIGEST_SYSTEM_PROMPT, &user)
            .with_temperature(0.2)
            .with_max_output_tokens(600);

    let response = match client.stream_response(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "session-digest summarization call failed");
            return None;
        }
    };

    let reply = parse_capture_reply(&response.output_text_projection())?;
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
        // Distilled session knowledge is reusable across projects — global vault.
        scope: Scope::Global,
        tier: None,
        importance: None,
    })
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
        Self {
            service: service.bind_current(),
        }
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
            scope: None, // None → parse_scope defaults to Global, so tool-captured knowledge follows the user across projects
            source_session: None,
            tier: None,
            importance: None,
        })?;
        Ok(dto.id)
    }
}

/// Adapts [`KnowledgeService`] to the workflow's [`KnowledgeRetriever`] trait so
/// knowledge retrieval nodes can query the real vault. The service methods are
/// synchronous (Mutex-backed); the trait is async, so each call simply forwards
/// to the sync method.
pub struct WorkflowKnowledgeRetriever {
    service: std::sync::Arc<KnowledgeService>,
}

impl WorkflowKnowledgeRetriever {
    /// Wrap a shared knowledge service.
    pub fn new(service: std::sync::Arc<KnowledgeService>) -> Self {
        Self {
            service: service.bind_current(),
        }
    }
}

#[async_trait::async_trait]
impl deepagent_runtime::workflow::KnowledgeRetriever for WorkflowKnowledgeRetriever {
    async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> deepagent_core::error::Result<Vec<deepagent_runtime::workflow::KnowledgeDocument>> {
        let hits = self.service.search(query, None, limit);
        Ok(hits
            .into_iter()
            .map(|h| deepagent_runtime::workflow::KnowledgeDocument {
                id: h.id,
                title: h.title,
                content: h.excerpt,
                score: h.score,
            })
            .collect())
    }
}

/// Minimum relevance for a background-prefetched memory to surface. Set above
/// the passive-injection threshold (0.30): the background channel supplements
/// the seeded passive block, so only clearly-relevant entries are worth a
/// mid-run `<system-reminder>`.
const RELEVANT_MEMORY_MIN_SCORE: f32 = 0.35;
/// How many entries to surface per background prefetch round.
const RELEVANT_MEMORY_LIMIT: usize = 3;

/// Entry kinds that document *how to use* something rather than what went
/// wrong. These are the ones worth suppressing while the model is already
/// exercising the tool, because the transcript contains working usage.
const REFERENCE_DOC_KINDS: [&str; 3] = ["command", "config", "note"];

/// True when `hit` is reference material for a tool the run is actively using.
///
/// Local design, same intent as Claude Code's selection rule ("do not select
/// memories that are usage reference or API documentation for those tools —
/// they are already being exercised. DO still select warnings, gotchas or known
/// issues about those tools"). Ours is deterministic: the vault has no per-entry
/// tool tag, so a hit counts as reference material when its kind is a
/// how-to kind *and* its title/excerpt names a recently-used tool. Pitfalls,
/// solutions and memories are never dropped here — active use is exactly when
/// those matter, and keyword overlap alone is what makes an unguarded match
/// wrong (a query mentioning "search" otherwise pulls in `web_search` docs).
fn is_reference_doc_for_active_tool(hit: &KnowledgeHitDto, recent_tools: &[String]) -> bool {
    if recent_tools.is_empty() || !REFERENCE_DOC_KINDS.contains(&hit.kind.as_str()) {
        return false;
    }
    let haystack = format!("{} {} {}", hit.title, hit.scope, hit.excerpt).to_lowercase();
    recent_tools.iter().any(|tool| {
        let lowered = tool.to_lowercase();
        !lowered.is_empty()
            && (haystack.contains(&lowered) || haystack.contains(&lowered.replace('_', " ")))
    })
}

/// Adapts [`KnowledgeService`] to the runtime's background relevant-memory
/// prefetch (§3.2). Reuses the existing retrieval stack; the runtime drives it
/// off the critical path and injects settled results as a `<system-reminder>`.
pub struct KnowledgeMemoryProvider {
    service: std::sync::Arc<KnowledgeService>,
}

impl KnowledgeMemoryProvider {
    /// Wrap a shared knowledge service.
    pub fn new(service: std::sync::Arc<KnowledgeService>) -> Self {
        Self {
            service: service.bind_current(),
        }
    }
}

#[async_trait::async_trait]
impl deepagent_runtime::RelevantMemoryProvider for KnowledgeMemoryProvider {
    async fn fetch_relevant(
        &self,
        query: &str,
        already_surfaced: &[String],
        recent_tools: &[String],
    ) -> Result<Vec<deepagent_runtime::RelevantMemory>> {
        // Pull extra candidates so the surfaced-id filter still fills the limit.
        let hits = self
            .service
            .search(query, None, RELEVANT_MEMORY_LIMIT + already_surfaced.len());
        let now_ms = SystemClock.now().as_millis();
        let out = hits
            .into_iter()
            .filter(|hit| hit.score >= RELEVANT_MEMORY_MIN_SCORE)
            .filter(|hit| !already_surfaced.contains(&hit.id))
            .filter(|hit| !is_reference_doc_for_active_tool(hit, recent_tools))
            .take(RELEVANT_MEMORY_LIMIT)
            .map(|hit| {
                let mut block = format!(
                    "[source: {} ({}) · {}]\n{}",
                    hit.title,
                    hit.scope,
                    hit.kind,
                    hit.excerpt.trim()
                );
                // Same rule as the passive knowledge block: an entry older than
                // one day carries a verify-first caveat, because a stale
                // file:line claim reads more authoritative, not less.
                let note = deepagent_knowledge::entry::staleness_note(hit.updated_at, now_ms);
                if !note.is_empty() {
                    block.push('\n');
                    block.push_str(&note);
                }
                deepagent_runtime::RelevantMemory { block, id: hit.id }
            })
            .collect();
        Ok(out)
    }
}

#[cfg(test)]
#[path = "knowledge_project_tests.rs"]
mod project_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::message::Message;

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
            tier: None,
            importance: None,
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
            "Encrypted credential storage",
            "Credential ciphertext is stored in SQLite under a separate logical field.",
            "config",
            "project",
        ))
        .unwrap();
        let hits = svc.search("credential ciphertext sqlite", None, 5);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].title, "Encrypted credential storage");
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
    fn audit_independent_vault_writers_preserve_distinct_notes() {
        let mut outcomes = Vec::new();
        for refresh_second in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let project = tmp.path().join("project");
            let global = tmp.path().join("global");
            // Same disk roots, different host caches. Both load before the
            // first write; the OS lock serializes their actual file writes.
            let first = KnowledgeService::open(&project, &global).unwrap();
            let second = KnowledgeService::open(&project, &global).unwrap();
            let first_note = first
                .save(draft(
                    "Audit retry: policy",
                    "AUDIT_NOTE_A is a distinct deployment retry policy.",
                    "note",
                    "global",
                ))
                .unwrap();
            if refresh_second {
                second.reload().unwrap();
            }
            let second_note = second
                .save(draft(
                    "Audit retry policy",
                    "AUDIT_NOTE_B is a separate application retry policy.",
                    "note",
                    "global",
                ))
                .unwrap();
            assert_ne!(first_note.title, second_note.title);
            let reopened = KnowledgeService::open(&project, &global).unwrap();
            let entries = reopened.list();
            let a_on_disk = entries.iter().any(|entry| entry.body == first_note.body);
            let b_on_disk = entries.iter().any(|entry| entry.body == second_note.body);
            let first_cache_matches_disk =
                first.get(&first_note.id) == reopened.get(&first_note.id);
            eprintln!("vault writer audit: refreshed={refresh_second} same_id={} disk_entries={} note_a={a_on_disk} note_b={b_on_disk} first_cache_matches_disk={first_cache_matches_disk}",
                first_note.id == second_note.id, entries.len());
            outcomes.push(a_on_disk && b_on_disk && first_note.id != second_note.id);
        }
        assert!(
            outcomes.iter().all(|preserved| *preserved),
            "successful saves of distinct titles must survive independent host caches"
        );
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

    #[tokio::test]
    async fn audit_existing_memory_handles_keep_their_project_after_activation() {
        use deepagent_builtins::{MemoryRecallTool, MemoryWriteTool};
        use deepagent_runtime::RelevantMemoryProvider;
        use deepagent_tools::{PermissionSet, ToolRegistry};

        let mut checks = Vec::new();
        for switch_project in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let project_a = tmp.path().join("a");
            let project_b = tmp.path().join("b");
            let global = tmp.path().join("global");
            let query = "audit deployment workspace routing memory";
            for (root, owner) in [(&project_a, "PROJECT_A"), (&project_b, "PROJECT_B")] {
                let seed = KnowledgeService::open(root, &global).unwrap();
                let mut entry = draft(
                    "Audit deployment workspace routing memory",
                    &format!("{query} owner={owner}"),
                    "memory",
                    "project",
                );
                entry.tier = Some("workspace".into());
                seed.save(entry).unwrap();
            }

            // The production assembler retains these wrappers over one shared
            // service. Capture them while A is active, before the UI or another
            // run activates B. No fake memory backend or retrieval is involved.
            let knowledge = Arc::new(KnowledgeService::open(&project_a, &global).unwrap());
            let memory = Arc::new(crate::memory_service::MemoryService::new(knowledge.clone()));
            let provider = KnowledgeMemoryProvider::new(knowledge.clone());
            let mut registry = ToolRegistry::new();
            registry
                .register(Arc::new(MemoryRecallTool::new(memory.clone())))
                .unwrap();
            registry
                .register(Arc::new(MemoryWriteTool::new(memory)))
                .unwrap();
            if switch_project {
                knowledge.activate_project(&project_b).unwrap();
            }

            let recall = registry
                .invoke(
                    "memory_recall",
                    serde_json::json!({"tier":"workspace"}),
                    &PermissionSet::developer(),
                    false,
                )
                .await
                .unwrap();
            assert!(recall.ok);
            let recall_text = recall.value.to_string();
            let fetched = provider.fetch_relevant(query, &[], &[]).await.unwrap();
            let read_a = recall_text.contains("PROJECT_A");
            let read_b = recall_text.contains("PROJECT_B");
            let fetched_a = fetched.iter().any(|item| item.block.contains("PROJECT_A"));
            let fetched_b = fetched.iter().any(|item| item.block.contains("PROJECT_B"));

            let content = "Audit durable write from the already running project A";
            let written = registry
                .invoke(
                    "memory_write",
                    serde_json::json!({
                        "tier":"workspace", "content":content, "scope":"project"
                    }),
                    &PermissionSet::developer(),
                    false,
                )
                .await
                .unwrap();
            assert!(written.ok);
            let saved_a = KnowledgeService::open(&project_a, &global)
                .unwrap()
                .list()
                .iter()
                .any(|entry| entry.body == content);
            let saved_b = KnowledgeService::open(&project_b, &global)
                .unwrap()
                .list()
                .iter()
                .any(|entry| entry.body == content);
            eprintln!("project memory audit: switched={switch_project} recall_a={read_a} recall_b={read_b} fetched_a={fetched_a} fetched_b={fetched_b} saved_a={saved_a} saved_b={saved_b}");
            checks.push(read_a && !read_b && fetched_a && !fetched_b && saved_a && !saved_b);
        }
        assert!(
            checks.iter().all(|check| *check),
            "handles retained by a running project A must not read or write project B"
        );
    }

    // ---- auto-capture --------------------------------------------------

    use deepagent_core::clock::Timestamp;
    use deepagent_core::event::{Event, EventPayload};
    use deepagent_core::id::{EventId, SessionId, TaskId};
    use deepagent_core::task::TaskState;
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
        let payload = serde_json::json!({
            "type": "response.output_text.delta", "delta": content_json
        })
        .to_string();
        let transport = Arc::new(crate::test_sequence::mock([
            payload,
            r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#.to_string(),
        ]));
        Arc::new(ModelClient::new(
            transport,
            ModelConfig::deepseek("sk-test"),
        ))
    }

    #[tokio::test]
    async fn capture_wait_does_not_lock_selection_and_publishes_to_existing_global_views() {
        use deepagent_models::transport::{EventSink, HttpTransport, TransportRequest};
        #[derive(Default)]
        struct DelayedCapture {
            entered: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        #[async_trait::async_trait]
        impl HttpTransport for DelayedCapture {
            async fn stream(&self, _: TransportRequest, sink: &mut dyn EventSink) -> Result<()> {
                let sink: &mut dyn deepagent_models::transport::EventSink =
                    &mut crate::test_sequence::sink(sink);
                self.entered.notify_one();
                self.release.notified().await;
                sink.on_event(&serde_json::json!({"type":"response.output_text.delta", "delta":
                    r#"{"worth_saving":true,"title":"Late shared capture","body":"late deployment evidence"}"#}).to_string())?;
                sink.on_event(
                    r#"{"type":"response.completed","response":{"id":"fixture-response","object":"response","created_at":1,"model":"deepseek-flash","output":[],"status":"completed"}}"#,
                )?;
                Ok(())
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let svc = Arc::new(service(tmp.path()));
        let alpha = svc.bind_current();
        let model = Arc::new(DelayedCapture::default());
        let client = Arc::new(ModelClient::new(
            model.clone(),
            ModelConfig::deepseek("offline-fixture-placeholder"),
        ));
        let pending = svc.clone();
        let capture = tokio::spawn(async move {
            pending
                .capture_from_session(client, "deepseek-v4-flash".into(), &recovery_events(), "s")
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), model.entered.notified())
            .await
            .unwrap();
        svc.activate_project(&tmp.path().join("beta")).unwrap();
        let beta = svc.bind_current();
        model.release.notify_one();
        let entry = tokio::time::timeout(std::time::Duration::from_secs(5), capture)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(entry.scope, "global");
        assert_eq!(alpha.get(&entry.id), Some(entry.clone()));
        assert_eq!(beta.get(&entry.id), Some(entry));
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

    /// A non-recovery (no failure) but substantive session with a tool call.
    /// Should reach the digest path; if the model says `worth_saving: true`,
    /// an active entry is persisted.
    fn substantive_no_failure_events() -> Vec<Event> {
        use deepagent_core::message::ToolCall;
        vec![
            ev(
                0,
                EventPayload::MessageAppended {
                    message: Message::user(
                        "fix tauri exe lock by closing the desktop window before rebuild",
                    ),
                },
            ),
            ev(
                1,
                EventPayload::ToolCallRequested {
                    call: ToolCall {
                        id: "c1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({}),
                    },
                },
            ),
            ev(
                2,
                EventPayload::ToolCallCompleted {
                    call_id: "c1".into(),
                    ok: true,
                    output: serde_json::json!({"content": "..."}),
                    duration_ms: 4,
                },
            ),
            ev(
                3,
                EventPayload::MessageAppended {
                    message: Message::assistant(
                        "Documented: close the running window before `pnpm tauri dev` to avoid os error 5.",
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

    #[tokio::test]
    async fn digest_path_saves_when_model_says_worth_saving() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        let client = client_streaming(
            r#"{"worth_saving": true, "title": "Tauri exe lock workaround", "kind": "pitfall", "tags": ["tauri","windows"], "body": "Close the running window before `pnpm tauri dev`."}"#,
        );
        let dto = svc
            .capture_from_session(
                client,
                "deepseek-v4-flash".into(),
                &substantive_no_failure_events(),
                "ses_digest",
            )
            .await;
        assert!(dto.is_some());
        let dto = dto.unwrap();
        assert_eq!(dto.title, "Tauri exe lock workaround");
        assert_eq!(dto.scope, "global");
        assert_eq!(dto.source_session.as_deref(), Some("ses_digest"));
        assert_eq!(svc.list().len(), 1);
        assert!(svc.list_drafts().is_empty());
    }

    #[tokio::test]
    async fn digest_path_skips_when_model_declines() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = service(tmp.path());
        // Model returns worth_saving: false → no fallback in the digest path.
        let client = client_streaming(r#"{"worth_saving": false}"#);
        let dto = svc
            .capture_from_session(
                client,
                "deepseek-v4-flash".into(),
                &substantive_no_failure_events(),
                "ses_digest",
            )
            .await;
        assert!(dto.is_none());
        assert!(svc.list().is_empty());
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
                tier: None,
                importance: None,
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
                tier: None,
                importance: None,
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

    #[test]
    fn global_vault_root_prefers_deepagent_home() {
        use std::ffi::OsString;
        let root = global_vault_root_from(|name| match name {
            "DEEPAGENT_HOME" => Some(OsString::from("/srv/deepagent")),
            "HOME" => Some(OsString::from("/home/dev")),
            _ => None,
        })
        .unwrap();
        assert_eq!(root, std::path::PathBuf::from("/srv/deepagent"));

        // Otherwise the user's home, with the same `.deepagent` folder name the
        // project-local vault uses.
        let root = global_vault_root_from(|name| match name {
            "HOME" => Some(OsString::from("/home/dev")),
            _ => None,
        })
        .unwrap();
        assert_eq!(root, std::path::PathBuf::from("/home/dev/.deepagent"));

        // Windows hosts.
        let root = global_vault_root_from(|name| match name {
            "USERPROFILE" => Some(OsString::from("C:\\Users\\dev")),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            root,
            std::path::PathBuf::from("C:\\Users\\dev").join(".deepagent")
        );
    }

    #[test]
    fn global_vault_root_fails_loudly_instead_of_guessing() {
        use std::ffi::OsString;
        // An empty DEEPAGENT_HOME must not become a relative "" vault.
        let error = global_vault_root_from(|name| match name {
            "DEEPAGENT_HOME" => Some(OsString::from("")),
            _ => None,
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("DEEPAGENT_HOME"),
            "the error must name what to set: {error}"
        );
    }

    #[tokio::test]
    async fn memory_provider_retrieves_and_dedups() {
        use deepagent_runtime::RelevantMemoryProvider;

        let tmp = tempfile::tempdir().unwrap();
        let svc = std::sync::Arc::new(service(tmp.path()));
        let saved = svc
            .save(draft(
                "Sandboxie output relay pitfall",
                "Sandboxie Start.exe does not relay sandboxed stdout; use the \
                 workspace redirect readback fix. Do not revert this.",
                "pitfall",
                "project",
            ))
            .unwrap();

        let provider = KnowledgeMemoryProvider::new(svc.clone());
        let hits = provider
            .fetch_relevant("sandboxie stdout output not showing", &[], &[])
            .await
            .unwrap();
        assert!(
            hits.iter().any(|m| m.id == saved.id),
            "seeded pitfall should surface for a relevant query; got {hits:?}"
        );
        assert!(hits
            .iter()
            .any(|m| m.block.contains("Sandboxie") && m.block.contains("redirect")));

        // De-dup: excluding the surfaced id yields no repeat of it.
        let again = provider
            .fetch_relevant(
                "sandboxie stdout output not showing",
                std::slice::from_ref(&saved.id),
                &[],
            )
            .await
            .unwrap();
        assert!(again.iter().all(|m| m.id != saved.id));
    }

    #[tokio::test]
    async fn reference_docs_for_a_tool_in_use_are_dropped_but_its_pitfalls_are_kept() {
        use deepagent_runtime::RelevantMemoryProvider;

        // Scenario: the run is actively calling `web_search`. Surfacing its
        // how-to note is noise (the transcript already shows working usage) and
        // it eats the 3-slot injection budget, while a gotcha about the same
        // tool is exactly what active use needs. A keyword match alone cannot
        // tell these apart, so the kind is the discriminator.
        let tmp = tempfile::tempdir().unwrap();
        let svc = Arc::new(service(tmp.path()));
        let how_to = svc
            .save(draft(
                "web_search usage reference",
                "Call web_search with a query string and optional allowed_domains.",
                "command",
                "global",
            ))
            .unwrap();
        let gotcha = svc
            .save(draft(
                "web_search returns stale results",
                "web_search results can lag the index by weeks; verify before citing.",
                "pitfall",
                "global",
            ))
            .unwrap();

        let provider = KnowledgeMemoryProvider::new(svc.clone());
        // Without the hint both surface (the query names the tool).
        let baseline = provider
            .fetch_relevant("web_search results and usage", &[], &[])
            .await
            .unwrap();
        let baseline_ids: Vec<&str> = baseline.iter().map(|m| m.id.as_str()).collect();
        assert!(
            baseline_ids.contains(&how_to.id.as_str()),
            "reference doc is injectable when the tool is not in use: {baseline_ids:?}"
        );

        let with_hint = provider
            .fetch_relevant(
                "web_search results and usage",
                &[],
                &["web_search".to_string()],
            )
            .await
            .unwrap();
        let kept: Vec<&str> = with_hint.iter().map(|m| m.id.as_str()).collect();
        assert!(
            !kept.contains(&how_to.id.as_str()),
            "usage reference for an actively used tool must be suppressed: {kept:?}"
        );
        assert!(
            kept.contains(&gotcha.id.as_str()),
            "the pitfall about the same tool must still surface: {kept:?}"
        );
    }
}
