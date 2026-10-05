//! The knowledge base façade: vaults (markdown truth) + a contextual-retrieval
//! index + capture/update logic.
//!
//! Retrieval reuses [`deepagent_memory::ContextualRetriever`] at the **chunk**
//! level (chunk → contextualize → embed → BM25 → RRF → rerank). The on-disk
//! `.md` files are the only source of truth; the index is a derived artifact
//! rebuilt from disk on load and after every mutation (cheap with the offline
//! [`deepagent_memory::HashingEmbedder`]).

use std::collections::{BTreeMap, BTreeSet};

use deepagent_core::error::{CoreError, Result};
use deepagent_memory::{
    cosine_similarity, ContextualRetriever, Embedder, HeadingContextualizer, RetrievedChunk,
    ScoreReranker,
};

use crate::entry::{EntryKind, KnowledgeEntry, Scope};
use crate::vault::Vault;

/// Tunables for passive injection and retrieval.
#[derive(Debug, Clone)]
pub struct KnowledgeConfig {
    /// Minimum normalized rerank score `[0,1]` for passive injection.
    pub min_score: f32,
    /// Maximum number of entries injected passively per turn.
    pub max_inject: usize,
    /// Approximate token budget for the injected block.
    pub max_inject_tokens: usize,
    /// Whether passive injection is enabled.
    pub passive_enabled: bool,
    /// Whether session auto-capture (recovery → active knowledge) is enabled.
    pub auto_capture_enabled: bool,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            min_score: 0.30,
            max_inject: 3,
            max_inject_tokens: 1200,
            passive_enabled: true,
            auto_capture_enabled: true,
        }
    }
}

/// One retrieval hit: the entry, its score, and the best-matching excerpt.
#[derive(Debug, Clone)]
pub struct KnowledgeHit {
    /// The matched entry.
    pub entry: KnowledgeEntry,
    /// Normalized rerank score in `[0,1]`.
    pub score: f32,
    /// The matching chunk text (for display / injection).
    pub excerpt: String,
}

/// A write request (from the `knowledge_write` tool or the UI).
#[derive(Debug, Clone)]
pub struct KnowledgeDraft {
    /// Entry title.
    pub title: String,
    /// Markdown body.
    pub body: String,
    /// Entry kind.
    pub kind: EntryKind,
    /// Tags.
    pub tags: Vec<String>,
    /// Originating session id, if any.
    pub source_session: Option<String>,
    /// Target vault scope (defaults to project at the call site).
    pub scope: Scope,
    /// Memory tier for [`EntryKind::Memory`] drafts; ordinary notes leave it
    /// `None` so their files stay byte-stable.
    pub tier: Option<String>,
    /// Baseline importance for recall ranking.
    pub importance: Option<f32>,
}

/// The composite, globally-unique id for an entry across vaults.
fn uid(scope: Scope, id: &str) -> String {
    format!("{}:{}", scope.label(), id)
}

/// Normalize a title for dedup comparison (case/space-insensitive).
fn norm_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Rough token estimate for budget control (chars/4, min 1).
fn estimate_tokens(s: &str) -> usize {
    (s.chars().count() / 4).max(1)
}

/// What one consolidation pass changed, in composite uids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsolidationReport {
    /// Entries moved to the vault's superseded archive.
    pub superseded: Vec<String>,
    /// Surviving entries rewritten with merged text or tags.
    pub rewritten: Vec<String>,
}

impl ConsolidationReport {
    /// Whether nothing changed, which is what makes a second pass a no-op.
    pub fn is_empty(&self) -> bool {
        self.superseded.is_empty() && self.rewritten.is_empty()
    }

    /// One line for a durable note or a log.
    pub fn summary(&self) -> String {
        format!(
            "memory consolidation: {} merged, {} rewritten",
            self.superseded.len(),
            self.rewritten.len()
        )
    }
}

/// Body overlap needed to call two entries the same decision.
///
/// Measured on this vault's own Chinese entries rather than picked: two records
/// of one decision written under reworded titles score 0.88, while two genuinely
/// different pitfalls that merely share technical wording score 0.02. The gap is
/// wide, so the bar sits at 0.75 — high enough that no amount of shared scaffolding
/// merges two decisions, low enough that a re-typed entry still folds in.
/// Title similarity is deliberately NOT a condition: the whole point is to catch
/// the same fact saved again under a different heading.
const DUPLICATE_SIMILARITY: f32 = 0.75;
/// Below this many shared bigrams the ratio is short-string noise.
const MIN_SHARED_BIGRAMS: usize = 12;

/// Character bigrams of a normalized text. Bigrams need no language-specific
/// tokenizer, which the vault cannot assume: entries here are mostly Chinese,
/// and splitting on whitespace turns a whole body into a single token.
fn bigrams(text: &str) -> BTreeSet<(char, char)> {
    let chars: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    chars
        .windows(2)
        .map(|window| (window[0], window[1]))
        .collect()
}

/// Jaccard overlap of two bigram sets.
fn overlap(left: &BTreeSet<(char, char)>, right: &BTreeSet<(char, char)>) -> f32 {
    let shared = left.iter().filter(|bigram| right.contains(bigram)).count();
    let union = left.len() + right.len() - shared;
    if union == 0 {
        return 0.0;
    }
    shared as f32 / union as f32
}

/// Whether two entries record the same thing: same scope and kind, and either the
/// same normalized title or a body that is nearly the text of the other.
fn is_duplicate(a: &KnowledgeEntry, b: &KnowledgeEntry) -> bool {
    if a.scope != b.scope || a.kind != b.kind {
        return false;
    }
    if norm_title(&a.title) == norm_title(&b.title) {
        return true;
    }
    let left = bigrams(&a.body);
    let right = bigrams(&b.body);
    let shared = left.iter().filter(|bigram| right.contains(bigram)).count();
    shared >= MIN_SHARED_BIGRAMS && overlap(&left, &right) >= DUPLICATE_SIMILARITY
}

/// The newest entry of a cluster: what a reader would have written last.
fn pick_survivor(group: &[KnowledgeEntry]) -> &KnowledgeEntry {
    group
        .iter()
        .min_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| {
                    b.importance
                        .unwrap_or_default()
                        .total_cmp(&a.importance.unwrap_or_default())
                })
                .then_with(|| a.id.cmp(&b.id))
        })
        .expect("a cluster is never empty")
}

/// Fold a cluster into `survivor`: union the tags, keep the strongest
/// importance, and append only lines the survivor does not already carry.
fn merge_into(mut survivor: KnowledgeEntry, group: &[KnowledgeEntry]) -> KnowledgeEntry {
    let mut seen: BTreeSet<String> = survivor
        .body
        .lines()
        .map(|line| line.trim().to_string())
        .collect();
    let mut tags: BTreeSet<String> = survivor.tags.iter().cloned().collect();
    let mut importance = survivor.importance;
    let mut created_at = survivor.created_at;
    for member in group {
        if member.id == survivor.id {
            continue;
        }
        for line in member.body.lines() {
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() || !seen.insert(trimmed.clone()) {
                continue;
            }
            survivor.body.push('\n');
            survivor.body.push_str(&trimmed);
        }
        tags.extend(member.tags.iter().cloned());
        if let Some(weight) = member.importance {
            importance = Some(importance.map_or(weight, |current| current.max(weight)));
        }
        created_at = created_at.min(member.created_at);
    }
    survivor.tags = tags.into_iter().collect();
    survivor.importance = importance;
    survivor.created_at = created_at;
    survivor
}

type Retriever<E> = ContextualRetriever<E, HeadingContextualizer, ScoreReranker>;

/// The knowledge base: one or more vaults plus a derived retrieval index.
pub struct KnowledgeBase<E: Embedder + Clone> {
    vaults: Vec<Vault>,
    config: KnowledgeConfig,
    embedder: E,
    /// uid (`scope:slug`) → entry.
    entries: BTreeMap<String, KnowledgeEntry>,
    /// uid (`scope:slug`) → pending draft. Kept entirely OUT of the retrieval
    /// index so drafts never leak into passive injection or search.
    drafts: BTreeMap<String, KnowledgeEntry>,
    retriever: Retriever<E>,
}

impl<E: Embedder + Clone> KnowledgeBase<E> {
    /// Build a knowledge base over `vaults` using `embedder` and `config`.
    /// Call [`KnowledgeBase::load_all`] to populate it from disk.
    pub fn new(vaults: Vec<Vault>, embedder: E, config: KnowledgeConfig) -> Self {
        let retriever = ContextualRetriever::new(embedder.clone(), HeadingContextualizer);
        Self {
            vaults,
            config,
            embedder,
            entries: BTreeMap::new(),
            drafts: BTreeMap::new(),
            retriever,
        }
    }

    /// Scan every vault and rebuild the index. Returns the number of active
    /// entries. Drafts are loaded into a separate map and are NOT indexed.
    pub fn load_all(&mut self) -> Result<usize> {
        let mut entries = BTreeMap::new();
        let mut drafts = BTreeMap::new();
        for vault in &self.vaults {
            for entry in vault.scan()? {
                entries.insert(uid(entry.scope, &entry.id), entry);
            }
            for mut draft in vault.scan_drafts()? {
                // Force draft status regardless of frontmatter, since location
                // (the `.drafts/` dir) is authoritative.
                draft.status = crate::entry::EntryStatus::Draft;
                drafts.insert(uid(draft.scope, &draft.id), draft);
            }
        }
        self.entries = entries;
        self.drafts = drafts;
        self.rebuild_index();
        Ok(self.entries.len())
    }

    /// Rebuild the retrieval index from the current entry set.
    fn rebuild_index(&mut self) {
        let mut retriever = ContextualRetriever::new(self.embedder.clone(), HeadingContextualizer);
        for (key, entry) in &self.entries {
            retriever.add_document(key, &entry.title, &entry.searchable_text());
        }
        self.retriever = retriever;
    }

    /// Retrieve entries for `query`, best first, at most `limit` distinct
    /// entries. Optionally filter by `kind`. Read-only and concurrency-safe.
    ///
    /// Candidate recall uses the contextual retriever (so BM25 exact-match
    /// tokens like command names / config keys still surface), but the returned
    /// `score` is an **absolute** query↔entry cosine similarity in `[0,1]`, so
    /// it is a valid threshold gate for passive injection (the retriever's own
    /// score is only relative within a single query and cannot be thresholded).
    pub fn search(&self, query: &str, kind: Option<EntryKind>, limit: usize) -> Vec<KnowledgeHit> {
        if limit == 0 || query.trim().is_empty() || self.entries.is_empty() {
            return Vec::new();
        }
        // Pull extra chunks so per-entry dedup still fills `limit` entries.
        let pool = (limit.saturating_mul(3)).max(10);
        let chunks = self.retriever.retrieve(query, pool);

        let query_vec = self.embedder.embed(query);
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut hits: Vec<KnowledgeHit> = Vec::new();
        for chunk in chunks {
            let key = &chunk.doc_id;
            if seen.contains(key) {
                continue; // keep only the top-scoring chunk per entry
            }
            let Some(entry) = self.entries.get(key) else {
                continue;
            };
            if let Some(k) = kind {
                if entry.kind != k {
                    continue;
                }
            }
            seen.insert(key.clone());
            // Absolute relevance: cosine of query vs the matched excerpt,
            // clamped to [0,1] (hashing embeddings are non-negative so cosine
            // is already in [0,1], but clamp defensively).
            let excerpt = excerpt_of(&chunk);
            let score =
                cosine_similarity(&query_vec, &self.embedder.embed(&excerpt)).clamp(0.0, 1.0);
            hits.push(KnowledgeHit {
                entry: entry.clone(),
                score,
                excerpt,
            });
        }
        // Order by absolute score, best first, then cap to `limit`.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        hits
    }

    /// Build the passive-injection block for `query`: retrieve, keep hits with
    /// `score >= min_score`, cap at `max_inject` and `max_inject_tokens`, and
    /// render a source-cited context block. Empty string when nothing qualifies
    /// or passive injection is disabled.
    pub fn passive_block(&self, query: &str) -> String {
        if !self.config.passive_enabled {
            return String::new();
        }
        let candidates = self.search(query, None, self.config.max_inject);
        let mut chosen: Vec<&KnowledgeHit> = Vec::new();
        let mut tokens = 0usize;
        for hit in &candidates {
            if hit.score < self.config.min_score {
                continue;
            }
            let cost = estimate_tokens(&hit.excerpt);
            if !chosen.is_empty() && tokens + cost > self.config.max_inject_tokens {
                break;
            }
            tokens += cost;
            chosen.push(hit);
            if chosen.len() >= self.config.max_inject {
                break;
            }
        }
        if chosen.is_empty() {
            return String::new();
        }

        let mut out = String::from("# 相关知识 (knowledge base, retrieved)\n");
        // Age caveat computed before `chosen` is consumed by the loop.
        let ages: Vec<i64> = chosen.iter().map(|hit| hit.entry.updated_at).collect();
        for hit in chosen {
            out.push_str(&format!(
                "\n[source: {} ({}) · {}]\n{}\n",
                hit.entry.title,
                hit.entry.scope.label(),
                hit.entry.kind.label(),
                hit.excerpt.trim()
            ));
        }
        // One shared staleness rule for every injection path (see entry.rs).
        let now_ms = {
            use deepagent_core::clock::Clock as _;
            deepagent_core::clock::SystemClock.now().as_millis()
        };
        let note = crate::entry::block_staleness_note(&ages, now_ms);
        if !note.is_empty() {
            out.push('\n');
            out.push_str(&note);
            out.push('\n');
        }
        out.trim_end().to_string()
    }

    /// Create a new entry or update an existing one (same scope + normalized
    /// title). Writes the `.md` file and rebuilds the index. Returns the entry.
    pub fn write(&mut self, draft: KnowledgeDraft, now_ms: i64) -> Result<KnowledgeEntry> {
        if draft.title.trim().is_empty() {
            return Err(CoreError::invalid("knowledge entry title is empty"));
        }
        let vault = self.vault_for(draft.scope).ok_or_else(|| {
            CoreError::invalid(format!(
                "no vault configured for scope {}",
                draft.scope.label()
            ))
        })?;

        // Dedup: an existing entry in the same scope with an equal normalized
        // title is updated in place.
        let target = norm_title(&draft.title);
        let existing_id = self
            .entries
            .values()
            .find(|e| e.scope == draft.scope && norm_title(&e.title) == target)
            .map(|e| e.id.clone());

        let entry = if let Some(id) = existing_id {
            let created = self
                .entries
                .get(&uid(draft.scope, &id))
                .map(|e| e.created_at)
                .unwrap_or(now_ms);
            KnowledgeEntry {
                id,
                title: draft.title,
                kind: draft.kind,
                tags: draft.tags,
                created_at: created,
                updated_at: now_ms,
                source_session: draft.source_session,
                scope: draft.scope,
                status: crate::entry::EntryStatus::Active,
                tier: draft.tier,
                importance: draft.importance,
                body: draft.body,
            }
        } else {
            let existing_ids: BTreeSet<String> = self
                .entries
                .values()
                .filter(|e| e.scope == draft.scope)
                .map(|e| e.id.clone())
                .collect();
            let id = Vault::unique_slug(&draft.title, &existing_ids);
            KnowledgeEntry {
                id,
                title: draft.title,
                kind: draft.kind,
                tags: draft.tags,
                created_at: now_ms,
                updated_at: now_ms,
                source_session: draft.source_session,
                scope: draft.scope,
                status: crate::entry::EntryStatus::Active,
                tier: draft.tier,
                importance: draft.importance,
                body: draft.body,
            }
        };

        vault.write(&entry)?;
        self.entries
            .insert(uid(entry.scope, &entry.id), entry.clone());
        self.rebuild_index();
        Ok(entry)
    }

    /// Fold duplicate long-term entries into one and archive the rest.
    ///
    /// Deterministic on purpose: this runs unattended after a turn, so every
    /// merged line is text that was already on disk, and nothing is deleted —
    /// the losers move to the vault's superseded archive and a human can put one
    /// back by hand. `exclude_session` keeps the entries the just-finished run
    /// wrote out of a pass that has no context for them.
    ///
    /// Clustering is single-linkage within one `(scope, kind)`: a group grows by
    /// any member matching. Transitive chains therefore merge as one decision,
    /// and the survivor is the newest entry, so the merged file is the one a
    /// reader would have written last.
    pub fn consolidate(
        &mut self,
        exclude_session: Option<&str>,
        now_ms: i64,
    ) -> Result<ConsolidationReport> {
        let mut report = ConsolidationReport::default();
        let candidates: Vec<KnowledgeEntry> = self
            .entries
            .values()
            .filter(|entry| match (&entry.source_session, exclude_session) {
                (Some(source), Some(excluded)) => source != excluded,
                _ => true,
            })
            .cloned()
            .collect();

        let mut groups: Vec<Vec<KnowledgeEntry>> = Vec::new();
        for entry in candidates {
            match groups
                .iter_mut()
                .find(|group| group.iter().any(|member| is_duplicate(member, &entry)))
            {
                Some(group) => group.push(entry),
                None => groups.push(vec![entry]),
            }
        }

        for group in groups.into_iter().filter(|group| group.len() > 1) {
            let scope = group[0].scope;
            let vault = self
                .vault_for(scope)
                .cloned()
                .ok_or_else(|| CoreError::invalid("no vault for the consolidation scope"))?;
            let survivor = pick_survivor(&group).clone();
            let mut merged = merge_into(survivor.clone(), &group);
            let rewritten = merged.body != survivor.body || merged.tags != survivor.tags;
            if rewritten {
                merged.updated_at = now_ms;
            }

            // One lock for the whole cluster: the rewrite and the archival moves
            // must not interleave with another writer.
            let lock = vault.lock()?;
            if rewritten {
                lock.write(&vault, &merged)?;
                report.rewritten.push(uid(scope, &merged.id));
            }
            for member in &group {
                if member.id == survivor.id {
                    continue;
                }
                let member_uid = uid(scope, &member.id);
                if lock.archive(&vault, &member.id)? {
                    report.superseded.push(member_uid.clone());
                }
                self.entries.remove(&member_uid);
            }
            drop(lock);
            self.entries.insert(uid(scope, &merged.id), merged);
        }

        if !report.is_empty() {
            self.rebuild_index();
        }
        Ok(report)
    }

    /// All entries, ordered by uid.
    pub fn list(&self) -> Vec<KnowledgeEntry> {
        self.entries.values().cloned().collect()
    }

    /// Look up an entry by its composite uid (`scope:slug`).
    pub fn get(&self, uid_key: &str) -> Option<&KnowledgeEntry> {
        self.entries.get(uid_key)
    }

    /// Delete an entry by composite uid. Removes the file and the index entry.
    pub fn delete(&mut self, uid_key: &str) -> Result<bool> {
        let Some(entry) = self.entries.get(uid_key).cloned() else {
            return Ok(false);
        };
        if let Some(vault) = self.vault_for(entry.scope) {
            vault.delete(&entry.id)?;
        }
        self.entries.remove(uid_key);
        self.rebuild_index();
        Ok(true)
    }

    /// Add a pending draft (from session auto-capture). Written to the vault's
    /// `.drafts/` subdir and kept OUT of the retrieval index. Returns the draft.
    pub fn add_draft(&mut self, draft: KnowledgeDraft, now_ms: i64) -> Result<KnowledgeEntry> {
        if draft.title.trim().is_empty() {
            return Err(CoreError::invalid("knowledge draft title is empty"));
        }
        let vault = self.vault_for(draft.scope).ok_or_else(|| {
            CoreError::invalid(format!(
                "no vault configured for scope {}",
                draft.scope.label()
            ))
        })?;
        // Unique id across both active entries and existing drafts in scope.
        let mut existing_ids: BTreeSet<String> = self
            .entries
            .values()
            .filter(|e| e.scope == draft.scope)
            .map(|e| e.id.clone())
            .collect();
        existing_ids.extend(
            self.drafts
                .values()
                .filter(|e| e.scope == draft.scope)
                .map(|e| e.id.clone()),
        );
        let id = Vault::unique_slug(&draft.title, &existing_ids);
        let entry = KnowledgeEntry {
            id,
            title: draft.title,
            kind: draft.kind,
            tags: draft.tags,
            created_at: now_ms,
            updated_at: now_ms,
            source_session: draft.source_session,
            scope: draft.scope,
            status: crate::entry::EntryStatus::Draft,
            tier: draft.tier,
            importance: draft.importance,
            body: draft.body,
        };
        vault.write_draft(&entry)?;
        self.drafts
            .insert(uid(entry.scope, &entry.id), entry.clone());
        // No index rebuild: drafts are never indexed.
        Ok(entry)
    }

    /// All pending drafts, ordered by uid.
    pub fn list_drafts(&self) -> Vec<KnowledgeEntry> {
        self.drafts.values().cloned().collect()
    }

    /// Accept a draft by composite uid: remove it from the drafts area and
    /// persist it as a normal active entry (entering the index, honoring the
    /// same same-title dedup as `write`). Returns the promoted entry.
    pub fn accept_draft(&mut self, uid_key: &str, now_ms: i64) -> Result<KnowledgeEntry> {
        let Some(draft) = self.drafts.get(uid_key).cloned() else {
            return Err(CoreError::not_found(format!("draft not found: {uid_key}")));
        };
        // Remove the draft file first so the slug is free for the active write.
        if let Some(vault) = self.vault_for(draft.scope) {
            vault.delete_draft(&draft.id)?;
        }
        self.drafts.remove(uid_key);
        // Promote via the normal write path (dedup + index update).
        self.write(
            KnowledgeDraft {
                title: draft.title,
                body: draft.body,
                kind: draft.kind,
                tags: draft.tags,
                source_session: draft.source_session,
                scope: draft.scope,
                tier: draft.tier,
                importance: draft.importance,
            },
            now_ms,
        )
    }

    /// Discard a draft by composite uid: delete its file and drop it. Returns
    /// whether a draft existed.
    pub fn discard_draft(&mut self, uid_key: &str) -> Result<bool> {
        let Some(draft) = self.drafts.get(uid_key).cloned() else {
            return Ok(false);
        };
        if let Some(vault) = self.vault_for(draft.scope) {
            vault.delete_draft(&draft.id)?;
        }
        self.drafts.remove(uid_key);
        Ok(true)
    }

    /// Number of pending drafts.
    pub fn draft_count(&self) -> usize {
        self.drafts.len()
    }

    /// The retrieval/injection config.
    pub fn config(&self) -> &KnowledgeConfig {
        &self.config
    }

    /// Replace the config.
    pub fn set_config(&mut self, config: KnowledgeConfig) {
        self.config = config;
    }

    /// Toggle passive injection.
    pub fn set_passive_enabled(&mut self, on: bool) {
        self.config.passive_enabled = on;
    }

    /// Toggle session auto-capture.
    pub fn set_auto_capture_enabled(&mut self, on: bool) {
        self.config.auto_capture_enabled = on;
    }

    /// Number of loaded entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the base has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn vault_for(&self, scope: Scope) -> Option<&Vault> {
        self.vaults.iter().find(|v| v.scope() == scope)
    }
}

/// The excerpt for a hit: the chunk text, trimmed.
fn excerpt_of(chunk: &RetrievedChunk) -> String {
    chunk.text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_memory::HashingEmbedder;

    fn base(tmp: &std::path::Path) -> KnowledgeBase<HashingEmbedder> {
        let project = Vault::new(tmp.join("proj"), Scope::Project);
        let global = Vault::new(tmp.join("glob"), Scope::Global);
        KnowledgeBase::new(
            vec![project, global],
            HashingEmbedder::default(),
            KnowledgeConfig::default(),
        )
    }

    fn draft(title: &str, body: &str, kind: EntryKind, scope: Scope) -> KnowledgeDraft {
        KnowledgeDraft {
            title: title.to_string(),
            body: body.to_string(),
            kind,
            tags: vec![],
            source_session: None,
            scope,
            tier: None,
            importance: None,
        }
    }

    #[test]
    fn write_then_search_finds_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft(
                "PowerShell pipe interrupted",
                "cargo test piped to Select-String often exits -1; redirect to a file instead.",
                EntryKind::Pitfall,
                Scope::Project,
            ),
            1000,
        )
        .unwrap();

        let hits = kb.search("powershell select-string exit", None, 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.title, "PowerShell pipe interrupted");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn bm25_finds_exact_command_token() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft(
                "Offline workspace tests",
                "Run the backend suite with the exact command cargo test --workspace --offline.",
                EntryKind::Command,
                Scope::Project,
            ),
            1,
        )
        .unwrap();
        kb.write(
            draft(
                "Frontend styling",
                "Tailwind classes control button colors in the composer.",
                EntryKind::Note,
                Scope::Project,
            ),
            1,
        )
        .unwrap();

        let hits = kb.search("--offline", None, 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].entry.title, "Offline workspace tests");
    }

    #[test]
    fn kind_filter_restricts_results() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft(
                "A command",
                "use git status often",
                EntryKind::Command,
                Scope::Project,
            ),
            1,
        )
        .unwrap();
        kb.write(
            draft(
                "A pitfall",
                "git status can be slow on huge repos",
                EntryKind::Pitfall,
                Scope::Project,
            ),
            1,
        )
        .unwrap();

        let only_cmd = kb.search("git status", Some(EntryKind::Command), 5);
        assert_eq!(only_cmd.len(), 1);
        assert_eq!(only_cmd[0].entry.kind, EntryKind::Command);
    }

    fn real_now_ms() -> i64 {
        use deepagent_core::clock::Clock as _;
        deepagent_core::clock::SystemClock.now().as_millis()
    }

    #[test]
    fn passive_block_carries_the_staleness_caveat_for_old_entries() {
        // A memory about code state is a point-in-time observation. Without the
        // caveat the model asserts it as fact — the failure Claude Code documented
        // after users reported stale file:line claims being trusted *more*
        // because they looked precise.
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let forty_seven_days = 47 * 86_400_000;
        kb.write(
            draft(
                "Build gate",
                "cargo test hangs without --offline behind the proxy",
                EntryKind::Pitfall,
                Scope::Project,
            ),
            real_now_ms() - forty_seven_days,
        )
        .unwrap();

        let block = kb.passive_block("cargo test hangs offline proxy");
        assert!(block.contains("cargo test hangs"), "{block}");
        assert!(
            block.contains("47 天") && block.contains("断言前先核对当前代码"),
            "stale entry must carry the caveat with a relative age: {block}"
        );
        assert!(
            !block.contains("1970") && !block.contains("2026-"),
            "the caveat uses a relative day count, not an absolute timestamp: {block}"
        );
    }

    #[test]
    fn passive_block_omits_the_caveat_for_fresh_entries() {
        // ≤1 day is noise, not information.
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft(
                "Build gate",
                "cargo test hangs without --offline behind the proxy",
                EntryKind::Pitfall,
                Scope::Project,
            ),
            real_now_ms() - 3_600_000,
        )
        .unwrap();

        let block = kb.passive_block("cargo test hangs offline proxy");
        assert!(block.contains("cargo test hangs"), "{block}");
        assert!(!block.contains("断言前先核对当前代码"), "{block}");
    }

    #[test]
    fn passive_block_threshold_filters_low_scores() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.set_config(KnowledgeConfig {
            min_score: 0.99, // impossibly high → nothing qualifies
            ..KnowledgeConfig::default()
        });
        kb.write(
            draft(
                "Note",
                "some content about deployment",
                EntryKind::Note,
                Scope::Project,
            ),
            1,
        )
        .unwrap();
        assert!(kb.passive_block("deployment").is_empty());
    }

    #[test]
    fn passive_block_renders_when_relevant() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.set_config(KnowledgeConfig {
            min_score: 0.0,
            ..KnowledgeConfig::default()
        });
        kb.write(
            draft(
                "Keyring backend fix",
                "On Windows the credential is stored under service deepagent-studio.",
                EntryKind::Solution,
                Scope::Project,
            ),
            1,
        )
        .unwrap();
        let block = kb.passive_block("windows keyring credential");
        assert!(block.contains("相关知识"));
        assert!(block.contains("[source: Keyring backend fix"));
    }

    #[test]
    fn passive_block_caps_at_max_inject() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.set_config(KnowledgeConfig {
            min_score: 0.0,
            max_inject: 2,
            ..KnowledgeConfig::default()
        });
        for i in 0..5 {
            kb.write(
                draft(
                    &format!("Deploy note {i}"),
                    "deployment pipeline notes and steps",
                    EntryKind::Note,
                    Scope::Project,
                ),
                1,
            )
            .unwrap();
        }
        let block = kb.passive_block("deployment pipeline");
        assert!(block.matches("[source:").count() <= 2);
    }

    #[test]
    fn write_dedup_is_idempotent_on_title() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let first = kb
            .write(
                draft("Same Title", "v1", EntryKind::Note, Scope::Project),
                1000,
            )
            .unwrap();
        let second = kb
            .write(
                draft(
                    "same title",
                    "v2 updated",
                    EntryKind::Solution,
                    Scope::Project,
                ),
                2000,
            )
            .unwrap();
        // Same file id, created_at preserved, updated_at refreshed.
        assert_eq!(first.id, second.id);
        assert_eq!(second.created_at, 1000);
        assert_eq!(second.updated_at, 2000);
        assert_eq!(kb.len(), 1);
        // Disk reflects exactly one file.
        let scoped: Vec<_> = Vault::new(tmp.path().join("proj"), Scope::Project)
            .scan()
            .unwrap();
        assert_eq!(scoped.len(), 1);
        assert!(scoped[0].body.contains("v2 updated"));
    }

    #[test]
    fn delete_removes_from_disk_and_index() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let e = kb
            .write(
                draft("To Delete", "body", EntryKind::Note, Scope::Project),
                1,
            )
            .unwrap();
        let key = format!("project:{}", e.id);
        assert!(kb.delete(&key).unwrap());
        assert!(kb.is_empty());
        assert!(kb.search("body", None, 5).is_empty());
        assert!(!kb.delete(&key).unwrap());
    }

    #[test]
    fn load_all_counts_both_vaults() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft("P", "project body", EntryKind::Note, Scope::Project),
            1,
        )
        .unwrap();
        kb.write(draft("G", "global body", EntryKind::Note, Scope::Global), 1)
            .unwrap();
        // Fresh base reloads from disk.
        let mut kb2 = base(tmp.path());
        let n = kb2.load_all().unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn empty_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let kb = base(tmp.path());
        assert!(kb.search("anything", None, 5).is_empty());
        assert!(kb.search("", None, 5).is_empty());
        assert!(kb.passive_block("anything").is_empty());
        assert!(kb.list().is_empty());
    }

    #[test]
    fn same_slug_across_scopes_coexist() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft("Shared", "project version", EntryKind::Note, Scope::Project),
            1,
        )
        .unwrap();
        kb.write(
            draft("Shared", "global version", EntryKind::Note, Scope::Global),
            1,
        )
        .unwrap();
        assert_eq!(kb.len(), 2);
        assert!(kb.get("project:shared").is_some());
        assert!(kb.get("global:shared").is_some());
    }

    #[test]
    fn drafts_excluded_from_search_and_passive() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.set_config(KnowledgeConfig {
            min_score: 0.0,
            ..KnowledgeConfig::default()
        });
        kb.add_draft(
            draft(
                "Draft about deployment pipeline",
                "deployment pipeline release steps and notes",
                EntryKind::Note,
                Scope::Project,
            ),
            1,
        )
        .unwrap();
        // Draft is listed as a draft...
        assert_eq!(kb.list_drafts().len(), 1);
        assert_eq!(kb.draft_count(), 1);
        // ...but never appears in search or passive injection (Property 10).
        assert!(kb.search("deployment pipeline release", None, 5).is_empty());
        assert!(kb.passive_block("deployment pipeline release").is_empty());
        assert!(kb.is_empty()); // no active entries
    }

    #[test]
    fn accept_draft_promotes_to_active_with_source() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let mut d = draft(
            "Recovered fix",
            "the fix that worked after retries",
            EntryKind::Solution,
            Scope::Project,
        );
        d.source_session = Some("ses_abc".into());
        let added = kb.add_draft(d, 1000).unwrap();
        let key = format!("project:{}", added.id);

        let promoted = kb.accept_draft(&key, 2000).unwrap();
        // Draft area emptied, active +1 (Property 13).
        assert_eq!(kb.draft_count(), 0);
        assert_eq!(kb.len(), 1);
        assert_eq!(promoted.source_session.as_deref(), Some("ses_abc"));
        // Now retrievable.
        kb.set_config(KnowledgeConfig {
            min_score: 0.0,
            ..KnowledgeConfig::default()
        });
        assert!(!kb.search("fix that worked retries", None, 5).is_empty());
        // Draft file is gone from disk.
        let drafts_on_disk = Vault::new(tmp.path().join("proj"), Scope::Project)
            .scan_drafts()
            .unwrap();
        assert!(drafts_on_disk.is_empty());
    }

    #[test]
    fn discard_draft_removes_without_promoting() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let added = kb
            .add_draft(
                draft("Throwaway", "not useful", EntryKind::Note, Scope::Project),
                1,
            )
            .unwrap();
        let key = format!("project:{}", added.id);
        assert!(kb.discard_draft(&key).unwrap());
        assert_eq!(kb.draft_count(), 0);
        assert!(kb.is_empty());
        assert!(!kb.discard_draft(&key).unwrap());
    }

    #[test]
    fn drafts_reload_from_disk_separately() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        kb.write(
            draft("Active", "active body", EntryKind::Note, Scope::Project),
            1,
        )
        .unwrap();
        kb.add_draft(
            draft("Pending", "pending body", EntryKind::Note, Scope::Project),
            1,
        )
        .unwrap();
        // Fresh base over the same dirs: 1 active indexed, 1 draft isolated.
        let mut kb2 = base(tmp.path());
        let active_count = kb2.load_all().unwrap();
        assert_eq!(active_count, 1);
        assert_eq!(kb2.draft_count(), 1);
    }

    /// The duplicate rule, pinned on real entries: one decision written twice
    /// under reworded titles must fold, and two different pitfalls that share
    /// technical wording must not. These are the cases the threshold was measured
    /// against, so they are the ones that have to keep behaving.
    #[test]
    fn duplicate_rule_folds_reworded_copies_and_keeps_distinct_pitfalls() {
        let entry = |title: &str, body: &str| KnowledgeEntry {
            id: Vault::slug(title),
            title: title.to_string(),
            kind: EntryKind::Pitfall,
            tags: vec![],
            created_at: 1,
            updated_at: 1,
            source_session: None,
            scope: Scope::Project,
            status: crate::entry::EntryStatus::Active,
            tier: None,
            importance: None,
            body: body.to_string(),
        };
        let cargo_a = entry(
            "cargo 并行构建产生假错误",
            "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误。",
        );
        let cargo_b = entry(
            "cargo 并行构建会假报错",
            "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误，别再回退。",
        );
        let powershell = entry(
            "PowerShell 管道把退出码吞掉",
            "长命令重定向到文件再看，管道到 Select-String 会把崩溃报成退出码 0。",
        );
        assert!(
            is_duplicate(&cargo_a, &cargo_b),
            "one decision written twice must fold"
        );
        assert!(
            !is_duplicate(&cargo_a, &powershell),
            "shared technical wording is not the same decision"
        );
    }

    #[test]
    fn consolidation_merges_duplicates_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let mut older = draft(
            "cargo 并行构建产生假错误",
            "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误。",
            EntryKind::Pitfall,
            Scope::Project,
        );
        older.tags = vec!["cargo".to_string()];
        kb.write(older, 1_000).unwrap();
        let mut newer = draft(
            "cargo 并行构建会假报错",
            "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误，别再回退。",
            EntryKind::Pitfall,
            Scope::Project,
        );
        newer.tags = vec!["构建".to_string()];
        kb.write(newer, 2_000).unwrap();
        // An unrelated decision must not be folded in.
        kb.write(
            draft(
                "登录态验证",
                "绕不过守卫时用构建加 DOM 探针取证，并声明未目视的屏。",
                EntryKind::Pitfall,
                Scope::Project,
            ),
            3_000,
        )
        .unwrap();

        let report = kb.consolidate(None, 4_000).unwrap();
        assert_eq!(
            report.superseded.len(),
            1,
            "one duplicate folded: {report:?}"
        );
        assert_eq!(report.rewritten.len(), 1, "the survivor carries the merge");
        assert_eq!(kb.list().len(), 2, "the unrelated entry survives");

        let survivor = kb
            .list()
            .into_iter()
            .find(|entry| entry.title.contains("假报错"))
            .expect("the newest wording is the one that stays");
        assert!(
            survivor.tags.contains(&"cargo".to_string())
                && survivor.tags.contains(&"构建".to_string()),
            "tags from both copies must be kept: {:?}",
            survivor.tags
        );
        assert!(
            survivor.body.contains("别再回退"),
            "the survivor's own text must not be rewritten away: {}",
            survivor.body
        );
        assert_eq!(survivor.updated_at, 4_000);

        // The loser is archived rather than deleted, so the pass stays reversible.
        let archived = tmp.path().join("proj").join(".superseded");
        let files: Vec<_> = std::fs::read_dir(&archived)
            .expect("the archive directory exists")
            .filter_map(|entry| entry.ok())
            .collect();
        assert_eq!(files.len(), 1, "the duplicate is recoverable: {report:?}");

        let again = kb.consolidate(None, 5_000).unwrap();
        assert!(
            again.is_empty(),
            "a second pass must change nothing: {again:?}"
        );
    }

    #[test]
    fn consolidation_leaves_the_active_session_and_other_scopes_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let mut kb = base(tmp.path());
        let body = "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误。";
        let mut just_written = draft("cargo 假错误", body, EntryKind::Pitfall, Scope::Project);
        just_written.source_session = Some("ses-active".to_string());
        kb.write(just_written, 1_000).unwrap();
        let older = draft(
            "cargo 并行构建会假报错",
            body,
            EntryKind::Pitfall,
            Scope::Global,
        );
        kb.write(older, 2_000).unwrap();

        // Same text, different scope: never one decision written twice.
        let report = kb.consolidate(Some("ses-active"), 3_000).unwrap();
        assert!(
            report.is_empty(),
            "scope isolation and the active session both hold entries back: {report:?}"
        );
        assert_eq!(kb.list().len(), 2);
    }
}
