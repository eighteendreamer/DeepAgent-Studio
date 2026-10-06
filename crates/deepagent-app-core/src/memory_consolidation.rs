//! Lazy long-term memory consolidation (底层优化方案 Phase 3 §6.3).
//!
//! The vault is the single source of truth for cross-session memory, and it only
//! grows: the same decision gets written again under a slightly different title,
//! and retrieval then surfaces three near-identical entries as if they were three
//! facts. This pass folds those clusters back into one entry.
//!
//! Three deliberate limits, because this runs unattended after a turn:
//!
//! - **Deterministic.** The merged text is only ever a union of lines that were
//!   already on disk. No model call, so the pass is free, cannot invent a memory
//!   that never happened, and cannot hang a background task. Rewriting wording is
//!   left to whoever reads the entry.
//! - **Reversible.** Superseded entries move to the vault's `.superseded/`
//!   archive instead of being deleted, so a wrong merge is one file move from
//!   being undone.
//! - **Lazy.** A watermark in the `documents` table gates the pass by elapsed
//!   time and finished runs, so an idle install never pays for a scan and there
//!   is no polling service to run.

use serde::{Deserialize, Serialize};

use deepagent_core::clock::{Clock, SystemClock};
use deepagent_core::error::CoreError;
use deepagent_core::Result;
use deepagent_persistence::Database;
use rusqlite::OptionalExtension;

use crate::knowledge_service::KnowledgeService;

/// Vault-wide watermark in the `documents` table.
const STATE_COLLECTION: &str = "memory_consolidation";
/// One vault pair per install, so one row.
const STATE_ID: &str = "vault";

/// When a pass is allowed to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConsolidationGate {
    /// Below this many entries a cluster is unlikely to exist, and the pairwise
    /// scan is the pass's only real cost.
    pub min_entries: usize,
    /// Minimum wall-clock gap between two passes.
    pub min_interval: std::time::Duration,
    /// Minimum finished runs since the last pass, so one long session does not
    /// re-scan the same vault every turn.
    pub min_runs_between: u64,
}

impl Default for ConsolidationGate {
    fn default() -> Self {
        Self {
            min_entries: 24,
            min_interval: std::time::Duration::from_secs(6 * 60 * 60),
            min_runs_between: 5,
        }
    }
}

/// Persisted watermark: when the pass last ran and how many runs finished since.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConsolidationState {
    /// Millisecond timestamp of the last pass that changed the vault, `0` when
    /// there has never been one.
    #[serde(default)]
    pub last_run_millis: i64,
    /// Finished runs counted since that pass.
    #[serde(default)]
    pub runs_since: u64,
    /// Lifetime count of archived duplicates: the audit trail.
    #[serde(default)]
    pub superseded_total: u64,
    /// Lifetime count of rewritten survivors.
    #[serde(default)]
    pub rewritten_total: u64,
}

impl ConsolidationGate {
    /// Whether the watermark allows a pass now.
    ///
    /// A vault that has never been consolidated satisfies the interval by
    /// definition, but still has to wait for `min_runs_between` finished runs:
    /// the first pass on a fresh install has nothing to gain from running on the
    /// first turn.
    fn allows(&self, state: &ConsolidationState, now_millis: i64) -> bool {
        let elapsed = if state.last_run_millis == 0 {
            u64::MAX
        } else {
            now_millis
                .saturating_sub(state.last_run_millis)
                .max(0)
                .try_into()
                .unwrap_or(u64::MAX)
        };
        elapsed >= self.min_interval.as_millis() as u64 && state.runs_since >= self.min_runs_between
    }
}

/// Load the watermark. Absent row means this install has never run a pass.
fn load_state(db: &Database) -> Result<ConsolidationState> {
    db.with_conn(|conn| {
        let body: Option<String> = conn
            .query_row(
                "SELECT body FROM documents WHERE collection = ?1 AND id = ?2",
                rusqlite::params![STATE_COLLECTION, STATE_ID],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| CoreError::Persistence(error.to_string()))?;
        match body {
            None => Ok(ConsolidationState::default()),
            Some(body) => serde_json::from_str(&body).map_err(|error| {
                CoreError::Persistence(format!("parse consolidation state: {error}"))
            }),
        }
    })
}

fn save_state(db: &Database, state: &ConsolidationState) -> Result<()> {
    let body = serde_json::to_string(state)
        .map_err(|error| CoreError::Persistence(format!("encode consolidation state: {error}")))?;
    let now = SystemClock.now().as_millis();
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO documents (id, collection, body, embedding, created_at, updated_at)
             VALUES (?2, ?1, ?3, NULL, ?4, ?4)
             ON CONFLICT(collection, id) DO UPDATE SET body = excluded.body,
                                                        updated_at = excluded.updated_at",
            rusqlite::params![STATE_COLLECTION, STATE_ID, body, now],
        )
        .map_err(|error| CoreError::Persistence(error.to_string()))?;
        Ok(())
    })
}

/// Run one gated consolidation pass after a successful run.
///
/// Counts the finished run and, when the gate opens, folds duplicate entries and
/// returns the note to record in that run's event log. `exclude_session` keeps the
/// entries this very run wrote out of reach: they are the newest facts on disk and
/// the pass has no context to judge them.
pub fn consolidate_if_due(
    db: &Database,
    knowledge: &KnowledgeService,
    exclude_session: &str,
    clock: &dyn Clock,
    gate: &ConsolidationGate,
) -> Result<Option<String>> {
    let now_millis = clock.now().as_millis();
    let mut state = load_state(db)?;
    state.runs_since = state.runs_since.saturating_add(1);
    if !gate.allows(&state, now_millis) || knowledge.entry_count() < gate.min_entries {
        // A pass that did not really run must not reset the clock: the vault may
        // gain its first duplicate on the very next turn.
        save_state(db, &state)?;
        return Ok(None);
    }

    let report = knowledge.consolidate(Some(exclude_session), now_millis)?;
    if report.is_empty() {
        save_state(db, &state)?;
        return Ok(None);
    }
    state.last_run_millis = now_millis;
    state.runs_since = 0;
    state.superseded_total = state
        .superseded_total
        .saturating_add(report.superseded.len() as u64);
    state.rewritten_total = state
        .rewritten_total
        .saturating_add(report.rewritten.len() as u64);
    save_state(db, &state)?;
    tracing::info!(
        superseded = report.superseded.len(),
        rewritten = report.rewritten.len(),
        "memory consolidation: pass finished"
    );
    Ok(Some(format!(
        "{} · 累计合并 {} 条",
        report.summary(),
        state.superseded_total
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_core::clock::FixedClock;
    use deepagent_knowledge::{
        EntryKind, KnowledgeBase, KnowledgeConfig, KnowledgeDraft, Scope, Vault,
    };
    use deepagent_memory::HashingEmbedder;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("deepagent-consol-{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn vault_root(&self) -> std::path::PathBuf {
            self.0.join("proj")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn draft(title: &str, body: &str, kind: EntryKind, session: Option<&str>) -> KnowledgeDraft {
        KnowledgeDraft {
            title: title.to_string(),
            body: body.to_string(),
            kind,
            tags: vec![],
            source_session: session.map(|s| s.to_string()),
            scope: Scope::Project,
            tier: None,
            importance: None,
        }
    }

    /// `clusters` decisions, each written down twice, plus `filler` unique
    /// entries. Repeated wording inside a cluster must merge; different clusters
    /// are about different things and must never merge with each other.
    fn knowledge(scratch: &Scratch, clusters: usize, filler: usize) -> KnowledgeService {
        let project = Vault::new(scratch.vault_root(), Scope::Project);
        let global = Vault::new(scratch.0.join("glob"), Scope::Global);
        let mut base = KnowledgeBase::new(
            vec![project, global],
            HashingEmbedder::default(),
            KnowledgeConfig::default(),
        );
        let mut written = 0i64;
        for (title_a, title_b, body) in [
            (
                "cargo 并行构建产生假错误",
                "cargo 并行构建会假报错",
                "本机 cargo 必须 -j1，并行构建会报出成百 prelude 假错误。",
            ),
            (
                "PowerShell 管道把退出码吞掉",
                "PowerShell 管道截断退出码",
                "长命令重定向到文件再看，管道到 Select-String 会把崩溃报成退出码 0。",
            ),
        ]
        .into_iter()
        .take(clusters)
        {
            for title in [title_a, title_b] {
                written += 1;
                base.write(
                    draft(title, body, EntryKind::Pitfall, Some("ses-other")),
                    written,
                )
                .unwrap();
            }
        }
        // Each filler is a different decision in its own words. Fillers that only
        // differ by a counter would be duplicates by definition, which is exactly
        // the over-merge the threshold has to avoid.
        for (title, body) in [
            (
                "登录态界面自验",
                "绕不过守卫时用构建产物加 DOM 探针取证，并声明未目视的屏。",
            ),
            (
                "线上排障顺序",
                "先比对部署产物特征码，再深入查渲染与代码链路。",
            ),
        ]
        .into_iter()
        .take(filler)
        {
            written += 1;
            base.write(
                draft(title, body, EntryKind::Note, Some("ses-other")),
                written,
            )
            .unwrap();
        }
        KnowledgeService::from_base(base)
    }

    fn gate() -> ConsolidationGate {
        ConsolidationGate {
            min_entries: 4,
            min_interval: std::time::Duration::from_millis(1_000),
            min_runs_between: 1,
        }
    }

    #[test]
    fn a_pass_folds_duplicates_and_records_a_note() {
        let scratch = Scratch::new("run");
        let db = Database::open_in_memory().unwrap();
        let knowledge = knowledge(&scratch, 2, 2);
        assert_eq!(knowledge.entry_count(), 6);

        let clock = FixedClock::new(10_000);
        let note = consolidate_if_due(&db, &knowledge, "ses-active", &clock, &gate())
            .unwrap()
            .expect("two duplicate clusters must be folded");
        assert!(note.contains('2'), "the note counts the merges: {note}");
        assert_eq!(knowledge.entry_count(), 4, "one entry per cluster");

        let archived = std::fs::read_dir(scratch.vault_root().join(".superseded"))
            .expect("the archive directory exists")
            .filter_map(|entry| entry.ok())
            .count();
        assert_eq!(archived, 2, "superseded entries stay recoverable");

        let state = load_state(&db).unwrap();
        assert_eq!(state.last_run_millis, 10_000);
        assert_eq!(state.superseded_total, 2);
        assert_eq!(state.runs_since, 0, "the counter restarts after a pass");
    }

    /// DoD §6 "跑两次不重复改": the gate would hold the second call back on its
    /// interval, so the fold is re-run with the gate taken out of the way. What
    /// must be idempotent is the fold, not the counter.
    #[test]
    fn the_fold_changes_nothing_on_a_second_pass() {
        let scratch = Scratch::new("idempotent");
        let db = Database::open_in_memory().unwrap();
        let knowledge = knowledge(&scratch, 2, 2);
        let clock = FixedClock::new(10_000);
        consolidate_if_due(&db, &knowledge, "ses-active", &clock, &gate())
            .unwrap()
            .expect("precondition: the first pass folds");
        let folded = knowledge.entry_count();

        let report = knowledge
            .consolidate(Some("ses-active"), 10_000)
            .expect("second fold");
        assert!(
            report.is_empty(),
            "a second pass changed the vault: {}",
            report.summary()
        );
        assert_eq!(knowledge.entry_count(), folded);
        assert_eq!(
            std::fs::read_dir(scratch.vault_root().join(".superseded"))
                .expect("the archive directory exists")
                .filter_map(|entry| entry.ok())
                .count(),
            2,
            "a no-op pass must not archive anything again"
        );
    }

    #[test]
    fn the_gate_holds_a_pass_back_until_enough_runs_accumulate() {
        let scratch = Scratch::new("gate");
        let db = Database::open_in_memory().unwrap();
        let knowledge = knowledge(&scratch, 2, 2);
        let strict = ConsolidationGate {
            min_runs_between: 3,
            ..gate()
        };
        let clock = FixedClock::new(10_000);

        for expected in 1..3u64 {
            assert!(
                consolidate_if_due(&db, &knowledge, "ses-active", &clock, &strict)
                    .unwrap()
                    .is_none(),
                "run {expected} may not open a pass"
            );
        }
        assert_eq!(knowledge.entry_count(), 6, "nothing merged yet");
        assert_eq!(load_state(&db).unwrap().runs_since, 2);

        assert!(
            consolidate_if_due(&db, &knowledge, "ses-active", &clock, &strict)
                .unwrap()
                .is_some(),
            "the third finished run opens the gate"
        );
        assert_eq!(knowledge.entry_count(), 4);

        assert!(
            consolidate_if_due(&db, &knowledge, "ses-active", &clock, &strict)
                .unwrap()
                .is_none(),
            "two passes in the same instant would re-scan a vault that just settled"
        );
    }

    #[test]
    fn a_small_vault_never_pays_for_a_scan() {
        let scratch = Scratch::new("small");
        let db = Database::open_in_memory().unwrap();
        let knowledge = knowledge(&scratch, 1, 0);
        let clock = FixedClock::new(10_000);

        assert!(
            consolidate_if_due(&db, &knowledge, "ses-active", &clock, &gate())
                .unwrap()
                .is_none(),
            "below the size floor nothing is scanned"
        );
        assert_eq!(knowledge.entry_count(), 2, "the duplicates are left alone");
        assert_eq!(
            load_state(&db).unwrap().runs_since,
            1,
            "the run was counted"
        );
        assert_eq!(
            load_state(&db).unwrap().last_run_millis,
            0,
            "a skipped pass must not claim to have run"
        );
    }
}
