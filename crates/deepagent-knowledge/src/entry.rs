//! Knowledge entry model: a single `.md` note plus its typed metadata.
//!
//! An entry is the unit of the knowledge base — one Markdown file in a
//! [`crate::vault::Vault`]. It carries a small YAML frontmatter block (title,
//! kind, tags, timestamps, source session) followed by the freeform Markdown
//! body. The frontmatter is parsed with the dependency-light splitter from
//! `deepagent-skills` (no external YAML crate), and parsing is **tolerant**:
//! a missing/garbled frontmatter still yields a usable entry whose title is
//! derived from the file id and whose kind defaults to [`EntryKind::Note`].

use serde::{Deserialize, Serialize};

use deepagent_memory::ObservationType;
use deepagent_skills::frontmatter;

/// The category of a knowledge entry. Drives classification and the UI badge,
/// and maps onto the memory tier system via [`EntryKind::observation_type`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A pitfall / gotcha to avoid repeating.
    Pitfall,
    /// A solution / fix that worked.
    Solution,
    /// A frequently used command.
    Command,
    /// An important configuration.
    Config,
    /// A general note.
    Note,
    /// A cross-session long-term memory written by `memory_write`. Kept in the
    /// same vault as notes so the retrieval track that injects `relevant
    /// memories` can actually see it (one store, one truth).
    Memory,
}

impl EntryKind {
    /// The canonical lowercase label (also what is written to frontmatter).
    pub fn label(&self) -> &'static str {
        match self {
            EntryKind::Pitfall => "pitfall",
            EntryKind::Solution => "solution",
            EntryKind::Command => "command",
            EntryKind::Config => "config",
            EntryKind::Note => "note",
            EntryKind::Memory => "memory",
        }
    }

    /// Parse a label into an [`EntryKind`], tolerating case/whitespace and
    /// common synonyms. Unknown values fall back to [`EntryKind::Note`].
    pub fn parse(s: &str) -> EntryKind {
        match s.trim().to_lowercase().as_str() {
            "pitfall" | "gotcha" | "trap" | "坑" => EntryKind::Pitfall,
            "solution" | "fix" | "解决" | "解决方法" => EntryKind::Solution,
            "command" | "cmd" | "命令" => EntryKind::Command,
            "config" | "configuration" | "配置" => EntryKind::Config,
            "memory" | "记忆" => EntryKind::Memory,
            _ => EntryKind::Note,
        }
    }

    /// Map onto the existing memory [`ObservationType`] so entries can reuse the
    /// tier/ranking machinery if needed.
    pub fn observation_type(&self) -> ObservationType {
        match self {
            EntryKind::Pitfall => ObservationType::Failure,
            EntryKind::Solution => ObservationType::BugFix,
            EntryKind::Command | EntryKind::Config => ObservationType::Knowledge,
            EntryKind::Note | EntryKind::Memory => ObservationType::Knowledge,
        }
    }
}

/// The vault a knowledge entry lives in. Carried back on every hit so callers
/// can tell project-local knowledge from global/user knowledge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Project-local knowledge (`<project>/.deepagent/knowledge/`).
    Project,
    /// User-global knowledge (`~/.deepagent/knowledge/` or app data dir).
    Global,
}

impl Scope {
    /// Canonical lowercase label.
    pub fn label(&self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Global => "global",
        }
    }
}

/// Whether an entry is a confirmed, retrievable note or a pending draft awaiting
/// user confirmation. Drafts are produced by session auto-capture and are kept
/// out of the retrieval index until accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    /// A confirmed entry — indexed and retrievable.
    Active,
    /// A pending draft — stored but excluded from retrieval until accepted.
    Draft,
}

impl EntryStatus {
    /// Canonical lowercase label (also written to frontmatter).
    pub fn label(&self) -> &'static str {
        match self {
            EntryStatus::Active => "active",
            EntryStatus::Draft => "draft",
        }
    }

    /// Parse a label; unknown/missing values default to [`EntryStatus::Active`]
    /// (so pre-existing files without a `status` field stay active).
    pub fn parse(s: &str) -> EntryStatus {
        match s.trim().to_lowercase().as_str() {
            "draft" => EntryStatus::Draft,
            _ => EntryStatus::Active,
        }
    }
}

/// One knowledge entry — the in-memory form of a single `.md` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    /// Stable id (equals the file-name slug, without the `.md` extension).
    pub id: String,
    /// Human title.
    pub title: String,
    /// The entry kind.
    pub kind: EntryKind,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Creation time (Unix ms).
    pub created_at: i64,
    /// Last-update time (Unix ms).
    pub updated_at: i64,
    /// The session that produced this entry, if any.
    #[serde(default)]
    pub source_session: Option<String>,
    /// Which vault this entry belongs to.
    pub scope: Scope,
    /// Whether this is a confirmed entry or a pending draft.
    #[serde(default = "default_status")]
    pub status: EntryStatus,
    /// Memory tier for [`EntryKind::Memory`] entries (`semantic` / `episodic`
    /// / `procedural` / `workspace` / `failure`). Absent on ordinary notes.
    #[serde(default)]
    pub tier: Option<String>,
    /// Baseline importance in `[0,1]` used by the recall ranking. Absent when
    /// the writer did not supply one.
    #[serde(default)]
    pub importance: Option<f32>,
    /// The Markdown body (everything after the frontmatter).
    pub body: String,
}

/// Default status for deserialization / legacy files: confirmed/active.
fn default_status() -> EntryStatus {
    EntryStatus::Active
}

impl KnowledgeEntry {
    /// Render the entry as a complete `.md` file: YAML frontmatter + body.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&format!("title: {}\n", yaml_scalar(&self.title)));
        out.push_str(&format!("kind: {}\n", self.kind.label()));
        out.push_str(&format!("tags: {}\n", yaml_tags(&self.tags)));
        out.push_str(&format!("created_at: {}\n", self.created_at));
        out.push_str(&format!("updated_at: {}\n", self.updated_at));
        if let Some(src) = &self.source_session {
            out.push_str(&format!("source_session: {}\n", yaml_scalar(src)));
        }
        // Only emit a status line for drafts; active is the implicit default,
        // keeping pre-existing files byte-stable.
        if self.status == EntryStatus::Draft {
            out.push_str("status: draft\n");
        }
        // Memory attributes are emitted only when present, so ordinary note
        // files stay byte-stable across upgrades.
        if let Some(tier) = &self.tier {
            out.push_str(&format!("tier: {}\n", yaml_scalar(tier)));
        }
        if let Some(importance) = self.importance {
            out.push_str(&format!("importance: {importance:.3}\n"));
        }
        out.push_str("---\n\n");
        out.push_str(self.body.trim_end());
        out.push('\n');
        out
    }

    /// Parse an entry from raw `.md` text. Tolerant: missing frontmatter →
    /// title derived from `id`, kind = [`EntryKind::Note`]; bad `kind` →
    /// [`EntryKind::Note`]; non-numeric timestamps → 0.
    pub fn from_markdown(id: &str, scope: Scope, raw: &str) -> KnowledgeEntry {
        let fm = frontmatter::parse(raw);

        let title = fm
            .get("title")
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| title_from_id(id));

        let kind = fm
            .get("kind")
            .map(EntryKind::parse)
            .unwrap_or(EntryKind::Note);

        let tags = fm.get("tags").map(parse_tags).unwrap_or_default();

        let created_at = fm.get("created_at").and_then(parse_millis).unwrap_or(0);
        let updated_at = fm
            .get("updated_at")
            .and_then(parse_millis)
            .unwrap_or(created_at);

        let source_session = fm
            .get("source_session")
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());

        let status = fm
            .get("status")
            .map(EntryStatus::parse)
            .unwrap_or(EntryStatus::Active);

        let tier = fm
            .get("tier")
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty());
        let importance = fm.get("importance").and_then(parse_float);

        // When there is no frontmatter at all, `frontmatter::parse` returns the
        // whole input as the body — exactly the tolerant behavior we want.
        let body = fm.body.trim().to_string();

        KnowledgeEntry {
            id: id.to_string(),
            title,
            kind,
            tags,
            created_at,
            updated_at,
            source_session,
            scope,
            status,
            tier,
            importance,
            body,
        }
    }

    /// Plain searchable text (title + tags + body), fed to the retriever.
    pub fn searchable_text(&self) -> String {
        let mut parts = vec![self.title.clone()];
        if !self.tags.is_empty() {
            parts.push(self.tags.join(" "));
        }
        parts.push(self.body.clone());
        parts.join("\n\n")
    }
}

/// Days elapsed since `updated_at_ms`, floor-rounded (0 today, 1 yesterday).
/// Negative inputs (clock skew, future mtime) clamp to 0.
pub fn memory_age_days(updated_at_ms: i64, now_ms: i64) -> i64 {
    ((now_ms - updated_at_ms).max(0)) / 86_400_000
}

/// Staleness caveat for an injected memory/knowledge entry. Returns an empty
/// string for entries ≤1 day old — warning there is noise.
///
/// Mirrors the reason Claude Code carries this: memories are point-in-time
/// observations, and one carrying a `file:line` claim reads *more* authoritative
/// when it is stale, not less. Phrased as "N 天" rather than an ISO timestamp on
/// purpose: models do not do date arithmetic, so a raw timestamp fails to
/// trigger the verify-before-asserting behavior the note exists to cause.
///
/// Single implementation shared by the passive knowledge block and the
/// background `relevant memories` injection so the two can never disagree.
pub fn staleness_note(updated_at_ms: i64, now_ms: i64) -> String {
    let days = memory_age_days(updated_at_ms, now_ms);
    if days <= 1 {
        return String::new();
    }
    format!(
        "本条记录已 {days} 天。记忆是时点观测，不是实时状态：其中关于代码行为、\
         文件位置或行号的断言可能已过期，断言前先核对当前代码。"
    )
}

/// The oldest age among `updated_at_ms` values, as a caveat string prefixed for
/// a block that injects several entries at once. Empty when all are fresh.
pub fn block_staleness_note(updated_at_ms: &[i64], now_ms: i64) -> String {
    let Some(oldest) = updated_at_ms.iter().min().copied() else {
        return String::new();
    };
    let days = memory_age_days(oldest, now_ms);
    if days <= 1 {
        return String::new();
    }
    format!(
        "以上记录中最旧的一条已 {days} 天。记忆是时点观测，不是实时状态：其中关于代码\
         行为、文件位置或行号的断言可能已过期，断言前先核对当前代码。"
    )
}

/// Turn a slug id back into a human-ish title (`my-note` -> `my note`).
fn title_from_id(id: &str) -> String {
    let t = id.replace(['-', '_'], " ");
    let t = t.trim();
    if t.is_empty() {
        "Untitled".to_string()
    } else {
        t.to_string()
    }
}

/// Parse a YAML-ish tags value: either `[a, b]` flow style or `a, b` plain.
fn parse_tags(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(trimmed);
    inner
        .split(',')
        .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\'').trim())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a millisecond timestamp; tolerate quotes/whitespace.
fn parse_millis(raw: &str) -> Option<i64> {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .parse::<i64>()
        .ok()
}

/// Tolerant float parse for the `importance` frontmatter key. A garbled value
/// yields `None` (recall then falls back to the writer's default) rather than
/// failing the whole entry, matching the parsing discipline above.
fn parse_float(raw: &str) -> Option<f32> {
    raw.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .parse::<f32>()
        .ok()
}

/// Quote a scalar for frontmatter if it contains characters that would confuse
/// the line parser (`:` or leading/trailing spaces). Otherwise emit bare.
fn yaml_scalar(s: &str) -> String {
    let needs_quote = s.contains(':') || s.contains('#') || s != s.trim() || s.is_empty();
    if needs_quote {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// Render tags as a YAML flow sequence: `[a, b, c]` (or `[]`).
fn yaml_tags(tags: &[String]) -> String {
    let inner = tags
        .iter()
        .map(|t| yaml_scalar(t))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_kind_parse_is_tolerant() {
        assert_eq!(EntryKind::parse("Pitfall"), EntryKind::Pitfall);
        assert_eq!(EntryKind::parse("  SOLUTION "), EntryKind::Solution);
        assert_eq!(EntryKind::parse("cmd"), EntryKind::Command);
        assert_eq!(EntryKind::parse("配置"), EntryKind::Config);
        // Unknown → Note.
        assert_eq!(EntryKind::parse("whatever"), EntryKind::Note);
        assert_eq!(EntryKind::parse(""), EntryKind::Note);
    }

    #[test]
    fn entry_kind_label_roundtrips() {
        for k in [
            EntryKind::Pitfall,
            EntryKind::Solution,
            EntryKind::Command,
            EntryKind::Config,
            EntryKind::Note,
        ] {
            assert_eq!(EntryKind::parse(k.label()), k);
        }
    }

    #[test]
    fn observation_type_mapping() {
        assert_eq!(
            EntryKind::Pitfall.observation_type(),
            ObservationType::Failure
        );
        assert_eq!(
            EntryKind::Solution.observation_type(),
            ObservationType::BugFix
        );
        assert_eq!(
            EntryKind::Command.observation_type(),
            ObservationType::Knowledge
        );
    }

    fn sample() -> KnowledgeEntry {
        KnowledgeEntry {
            id: "powershell-pipe-interrupt".to_string(),
            title: "PowerShell 管道命令被 ^C 中断".to_string(),
            kind: EntryKind::Pitfall,
            tags: vec!["windows".into(), "powershell".into()],
            created_at: 1_730_000_000_000,
            updated_at: 1_730_000_001_000,
            source_session: Some("ses_019e7b".into()),
            scope: Scope::Project,
            status: EntryStatus::Active,
            tier: None,
            importance: None,
            body: "## 现象\n`cargo test | Select-String` 经常 exit -1。\n\n## 解决\n改用 `> out.txt 2>&1` 重定向。".to_string(),
        }
    }

    #[test]
    fn markdown_roundtrip_preserves_fields() {
        let entry = sample();
        let md = entry.to_markdown();
        let back = KnowledgeEntry::from_markdown(&entry.id, Scope::Project, &md);
        assert_eq!(back.title, entry.title);
        assert_eq!(back.kind, entry.kind);
        assert_eq!(back.tags, entry.tags);
        assert_eq!(back.created_at, entry.created_at);
        assert_eq!(back.updated_at, entry.updated_at);
        assert_eq!(back.source_session, entry.source_session);
        assert_eq!(back.body.trim(), entry.body.trim());
    }

    #[test]
    fn tolerant_parse_without_frontmatter() {
        let raw = "# Just a note\n\nSome body text without frontmatter.";
        let entry = KnowledgeEntry::from_markdown("my-loose-note", Scope::Global, raw);
        assert_eq!(entry.title, "my loose note");
        assert_eq!(entry.kind, EntryKind::Note);
        assert!(entry.tags.is_empty());
        assert!(entry.body.contains("Some body text"));
        assert_eq!(entry.scope, Scope::Global);
    }

    #[test]
    fn tolerant_parse_with_bad_kind() {
        let raw = "---\ntitle: X\nkind: bogus\n---\nbody";
        let entry = KnowledgeEntry::from_markdown("x", Scope::Project, raw);
        assert_eq!(entry.kind, EntryKind::Note);
        assert_eq!(entry.title, "X");
    }

    #[test]
    fn title_with_colon_survives_roundtrip() {
        let mut entry = sample();
        entry.title = "fix: the thing".to_string();
        let md = entry.to_markdown();
        let back = KnowledgeEntry::from_markdown(&entry.id, Scope::Project, &md);
        assert_eq!(back.title, "fix: the thing");
    }

    #[test]
    fn searchable_text_includes_title_tags_body() {
        let entry = sample();
        let text = entry.searchable_text();
        assert!(text.contains("PowerShell"));
        assert!(text.contains("windows"));
        assert!(text.contains("重定向"));
    }

    #[test]
    fn status_roundtrips_and_defaults_active() {
        // Active entries omit the status line; parse back as active.
        let active = sample();
        assert_eq!(active.status, EntryStatus::Active);
        let md = active.to_markdown();
        assert!(!md.contains("status:"));
        let back = KnowledgeEntry::from_markdown(&active.id, Scope::Project, &md);
        assert_eq!(back.status, EntryStatus::Active);

        // Draft entries emit and round-trip the status.
        let mut draft = sample();
        draft.status = EntryStatus::Draft;
        let dmd = draft.to_markdown();
        assert!(dmd.contains("status: draft"));
        let dback = KnowledgeEntry::from_markdown(&draft.id, Scope::Project, &dmd);
        assert_eq!(dback.status, EntryStatus::Draft);
    }

    #[test]
    fn legacy_file_without_status_is_active() {
        let raw = "---\ntitle: Legacy\nkind: note\n---\nbody";
        let entry = KnowledgeEntry::from_markdown("legacy", Scope::Project, raw);
        assert_eq!(entry.status, EntryStatus::Active);
    }

    #[test]
    fn staleness_note_only_fires_past_one_day() {
        let day = 86_400_000;
        let now = 1_730_000_000_000;
        assert_eq!(memory_age_days(now, now), 0);
        assert_eq!(memory_age_days(now - day + 1, now), 0);
        assert_eq!(memory_age_days(now - day, now), 1);
        assert_eq!(memory_age_days(now - 47 * day, now), 47);
        // Clock skew / future mtime must not produce a negative age.
        assert_eq!(memory_age_days(now + day, now), 0);

        assert!(staleness_note(now - day, now).is_empty(), "1 day is noise");
        assert!(staleness_note(now, now).is_empty());
        let two = staleness_note(now - 2 * day, now);
        assert!(
            two.contains("2 天") && two.contains("断言前先核对当前代码"),
            "{two}"
        );
        assert!(staleness_note(now - 47 * day, now).contains("47 天"));
    }

    #[test]
    fn block_note_reports_the_oldest_entry_and_stays_quiet_when_fresh() {
        let day = 86_400_000;
        let now = 1_730_000_000_000;
        assert!(block_staleness_note(&[now, now - day], now).is_empty());
        let note = block_staleness_note(&[now - 3 * day, now - 40 * day, now], now);
        assert!(
            note.contains("最旧的一条已 40 天"),
            "one caveat per block, sized by the oldest entry: {note}"
        );
        assert_eq!(block_staleness_note(&[], now), "");
    }
}
