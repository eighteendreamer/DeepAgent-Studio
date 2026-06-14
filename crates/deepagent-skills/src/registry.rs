//! The [`SkillRegistry`] — dynamic registration + progressive disclosure.
//!
//! The registry holds every known skill's metadata (Level 1, always resident)
//! and supports:
//! - **dynamic registration / unregistration** (install/uninstall at runtime),
//! - **passive activation** — [`SkillRegistry::match_query`] scores skills
//!   against a user query using their trigger phrases (model-driven use),
//! - **active activation** — [`SkillRegistry::activate`] by id (explicit use),
//! - **context injection** — an activated skill's Level-2 body becomes a
//!   [`PromptFragment`] for the context pipeline, while Level-1 metadata is
//!   exposed as a compact catalog blurb.

use std::collections::BTreeMap;

use deepagent_context::{PromptFragment, PromptSource};

use crate::skill::{Skill, SkillMeta, SkillOrigin, SkillToolOutput};

/// A scored passive-activation match.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillMatch {
    /// The matched skill id.
    pub id: String,
    /// Match score (higher = stronger). Sums matched trigger weights.
    pub score: f32,
    /// The trigger phrases that matched.
    pub matched_triggers: Vec<String>,
}

/// Priority assigned to an activated skill's body fragment (high — it is
/// procedural guidance the agent specifically pulled in).
const SKILL_BODY_PRIORITY: u8 = 160;

/// Maximum length, in characters, of one entry's description in the
/// `<available-skills>` catalog reminder. Longer descriptions are truncated
/// with a trailing ellipsis. Mirrors Claude Code's `MAX_LISTING_DESC_CHARS`
/// (design.md §Auto-Activation §通道 A.格式).
pub const MAX_LISTING_DESC_CHARS: usize = 250;

/// Below this per-skill description budget, non-built-in entries degrade to
/// names-only (`- {id}`) in the catalog reminder. Built-in skills are always
/// kept full. Mirrors Claude Code's `MIN_DESC_LENGTH`
/// (design.md §Auto-Activation §通道 A.字符预算).
pub const MIN_DESC_LENGTH: usize = 20;

/// Header text injected at the top of each `<available-skills>` reminder. The
/// short instruction tells the model how to act on the catalog without
/// derailing the surrounding turn (design.md §Auto-Activation §通道 A.格式
/// example).
const CATALOG_HEADER: &str = "The following skills are available. When a user request matches a skill's\npurpose, invoke the `skill` tool with its id (do NOT mention the skill in\nyour reply without invoking it).";

/// Errors returned by [`SkillRegistry::body_for_invoke`].
///
/// The `skill` tool (channel B) maps these directly to a tool-error result.
/// `NotFound` carries up to 5 fuzzy-matched candidate ids so the model can
/// retry with a corrected id. `DisabledForModel` is the user-only opt-out
/// for skills that carry `disable-model-invocation: true` in their
/// frontmatter (design.md §Auto-Activation §通道 B).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyForInvokeError {
    /// The requested skill id is not registered. `suggestions` carries the
    /// closest registered ids (descending similarity, max 5) for retry.
    NotFound {
        /// The id the caller asked for.
        id: String,
        /// Up to 5 fuzzy-matched ids, descending by similarity.
        suggestions: Vec<String>,
    },
    /// The skill exists but its frontmatter sets
    /// `disable-model-invocation: true`. The model must not invoke it; the
    /// user can still trigger it via slash command / UI.
    DisabledForModel {
        /// The id of the user-only skill.
        id: String,
    },
}

impl std::fmt::Display for BodyForInvokeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyForInvokeError::NotFound { id, suggestions } => {
                write!(f, "skill '{id}' not found")?;
                if !suggestions.is_empty() {
                    write!(f, " (did you mean: {})", suggestions.join(", "))?;
                }
                Ok(())
            }
            BodyForInvokeError::DisabledForModel { id } => {
                write!(f, "skill '{id}' is user-only, ask the user to invoke /{id}")
            }
        }
    }
}

impl std::error::Error for BodyForInvokeError {}

/// Registry of skills keyed by id.
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    skills: BTreeMap<String, Skill>,
}

impl SkillRegistry {
    /// New empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (install) a skill, replacing any existing one with the same id.
    /// Returns the previous skill if one was replaced.
    pub fn register(&mut self, skill: Skill) -> Option<Skill> {
        tracing::debug!(id = %skill.meta.id, origin = skill.meta.origin.label(), "registering skill");
        self.skills.insert(skill.meta.id.clone(), skill)
    }

    /// Unregister (uninstall) a skill by id. Returns it if present.
    pub fn unregister(&mut self, id: &str) -> Option<Skill> {
        tracing::debug!(id, "unregistering skill");
        self.skills.remove(id)
    }

    /// Whether a skill with `id` is registered.
    pub fn contains(&self, id: &str) -> bool {
        self.skills.contains_key(id)
    }

    /// Look up a skill by id.
    pub fn get(&self, id: &str) -> Option<&Skill> {
        self.skills.get(id)
    }

    /// Number of registered skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// All skill metadata (Level 1), sorted by id.
    pub fn catalog(&self) -> Vec<&SkillMeta> {
        self.skills.values().map(|s| &s.meta).collect()
    }

    /// The always-resident Level-1 catalog blurb: one line per skill. This is
    /// what stays in the system prompt so the model knows which skills exist.
    pub fn catalog_blurb(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut out = String::from("Available skills (invoke when relevant):\n");
        for skill in self.skills.values() {
            out.push_str(&skill.meta.blurb());
            out.push('\n');
        }
        out.trim_end().to_string()
    }

    /// Passive activation: score every skill against `query`, returning matches
    /// sorted by descending score.
    ///
    /// Two signals combine (exact-phrase dominates):
    /// - **Exact phrase** — the lower-cased query contains a (lower-cased)
    ///   trigger phrase. Strong weight, scaled by phrase length.
    /// - **Word overlap** — significant words of a trigger phrase that appear as
    ///   whole words in the query. A weak fallback so natural phrasings
    ///   ("review this pull request") still match triggers ("review a pull
    ///   request") without an exact substring hit.
    pub fn match_query(&self, query: &str) -> Vec<SkillMatch> {
        let q = query.to_lowercase();
        let q_tokens = tokenize(&q);
        let mut matches: Vec<SkillMatch> = Vec::new();

        for skill in self.skills.values() {
            let mut score = 0.0_f32;
            let mut matched = Vec::new();
            for trigger in &skill.triggers {
                if q.contains(trigger.as_str()) {
                    // Strong: exact phrase containment.
                    let words = trigger.split_whitespace().count() as f32;
                    score += 5.0 + words;
                    matched.push(trigger.clone());
                } else {
                    // Weak: significant-word overlap.
                    let overlap = trigger
                        .split_whitespace()
                        .filter_map(normalize_word)
                        .filter(|w| is_significant(w) && q_tokens.contains(w))
                        .count();
                    if overlap > 0 {
                        score += 0.5 * overlap as f32;
                        matched.push(trigger.clone());
                    }
                }
            }
            if score > 0.0 {
                matches.push(SkillMatch {
                    id: skill.meta.id.clone(),
                    score,
                    matched_triggers: matched,
                });
            }
        }

        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        matches
    }

    /// The single best passive match for `query`, if any.
    pub fn best_match(&self, query: &str) -> Option<SkillMatch> {
        self.match_query(query).into_iter().next()
    }

    /// Active activation: fetch a skill by id and render its Level-2 body as a
    /// context [`PromptFragment`]. Returns `None` if the skill is unknown.
    ///
    /// This is the disclosure step: Level-1 metadata was always present; here
    /// the body is pulled into context because the skill was chosen (actively
    /// by id, or passively via [`SkillRegistry::best_match`]).
    pub fn activate(&self, id: &str) -> Option<PromptFragment> {
        let skill = self.skills.get(id)?;
        let mut content = format!("# Skill: {}\n\n{}", skill.meta.name, skill.body);
        if skill.has_resources() {
            content.push_str("\n\n## Bundled resources (load on demand)\n");
            for r in &skill.resources {
                content.push_str(&format!("- [{}] {}\n", resource_label(r.kind), r.rel_path));
            }
        }
        Some(PromptFragment::new(
            PromptSource::WorkspaceRules,
            SKILL_BODY_PRIORITY,
            content,
        ))
    }

    /// Convenience: passively match `query`, and if anything matched, activate
    /// the best match, returning `(matched_id, body_fragment)`.
    pub fn auto_activate(&self, query: &str) -> Option<(String, PromptFragment)> {
        let best = self.best_match(query)?;
        let fragment = self.activate(&best.id)?;
        Some((best.id, fragment))
    }

    /// Render the model-facing `<available-skills>` catalog reminder for the
    /// turn-0 / delta system reminder (channel A in design.md
    /// §Auto-Activation).
    ///
    /// Skills carrying `disable-model-invocation: true` are filtered out — they
    /// stay invisible to the model. `whenToUse` is intentionally *not*
    /// rendered here; that field is deferred to a later iteration. Each
    /// entry is `- {id}: {description}` with the description truncated at
    /// [`MAX_LISTING_DESC_CHARS`] characters.
    ///
    /// Behaviour:
    ///
    /// - `char_budget == 0` or no visible skills → returns `String::new()`
    ///   (caller should suppress the reminder).
    /// - Total fits in `char_budget` → full descriptions, all entries kept.
    /// - Over budget → built-in skills keep their full descriptions; the
    ///   remaining budget is divided evenly among non-built-in skills. If
    ///   the per-skill share drops below [`MIN_DESC_LENGTH`], non-built-in
    ///   entries degrade to names-only (`- {id}`); built-ins always retain
    ///   full descriptions.
    pub fn formatted_catalog(&self, char_budget: usize) -> String {
        if char_budget == 0 {
            return String::new();
        }
        let visible: Vec<&Skill> = self
            .skills
            .values()
            .filter(|s| !s.meta.disable_model_invocation)
            .collect();
        if visible.is_empty() {
            return String::new();
        }

        // First try: render every line at full description (capped to
        // MAX_LISTING_DESC_CHARS per line).
        let full_lines: Vec<String> = visible.iter().map(|s| catalog_full_line(s)).collect();
        let full = wrap_catalog(&full_lines);
        if full.chars().count() <= char_budget {
            return full;
        }

        // Over budget: built-ins keep their full lines; non-built-ins split
        // the remaining budget evenly. Compute the wrapper overhead exactly
        // so the truncated output stays close to the budget.
        let wrapper_chars = wrap_catalog(&[]).chars().count() + visible.len().saturating_sub(1);

        let builtin_chars: usize = visible
            .iter()
            .filter(|s| s.meta.origin == SkillOrigin::BuiltIn)
            .map(|s| catalog_full_line(s).chars().count())
            .sum();

        let non_builtin_count = visible
            .iter()
            .filter(|s| s.meta.origin != SkillOrigin::BuiltIn)
            .count();

        if non_builtin_count == 0 {
            // Only built-ins are present; built-ins are by contract never
            // truncated, so we honour them and return full lines even if
            // nominally over budget.
            return wrap_catalog(&full_lines);
        }

        let used = wrapper_chars + builtin_chars;
        let remaining = char_budget.saturating_sub(used);
        let per_skill = remaining / non_builtin_count;
        let names_only = per_skill < MIN_DESC_LENGTH;

        let lines: Vec<String> = visible
            .iter()
            .map(|s| {
                if s.meta.origin == SkillOrigin::BuiltIn {
                    catalog_full_line(s)
                } else if names_only {
                    format!("- {}", s.meta.id)
                } else {
                    let prefix = format!("- {}: ", s.meta.id);
                    let prefix_len = prefix.chars().count();
                    let desc_budget = per_skill
                        .saturating_sub(prefix_len)
                        .min(MAX_LISTING_DESC_CHARS);
                    let desc = truncate_desc(&s.meta.description, desc_budget);
                    format!("{prefix}{desc}")
                }
            })
            .collect();

        wrap_catalog(&lines)
    }

    /// Render the tool-result payload for the `skill` tool (channel B in
    /// design.md §Auto-Activation).
    ///
    /// On success, returns a [`SkillToolOutput`] carrying:
    ///
    /// - the disclosed Level-2 body with `${ARGS}` then `$ARGS` literally
    ///   substituted with `args.unwrap_or("")`,
    /// - the skill's display name,
    /// - the on-disk base directory if known (for the model's
    ///   `read_file` / `grep`),
    /// - the skill's known Level-3 resource paths (relative to `base_dir`).
    ///
    /// On miss:
    ///
    /// - if the id is unknown, returns
    ///   [`BodyForInvokeError::NotFound`] with up to 5 fuzzy-matched
    ///   suggestions ordered by descending similarity,
    /// - if the skill carries `disable-model-invocation: true`, returns
    ///   [`BodyForInvokeError::DisabledForModel`].
    pub fn body_for_invoke(
        &self,
        id: &str,
        args: Option<&str>,
    ) -> Result<SkillToolOutput, BodyForInvokeError> {
        let Some(skill) = self.skills.get(id) else {
            let suggestions = self.fuzzy_suggestions(id);
            return Err(BodyForInvokeError::NotFound {
                id: id.to_string(),
                suggestions,
            });
        };
        if skill.meta.disable_model_invocation {
            return Err(BodyForInvokeError::DisabledForModel {
                id: skill.meta.id.clone(),
            });
        }

        let args_str = args.unwrap_or("");
        // ${ARGS} is the canonical placeholder — substitute it first so the
        // unbraced $ARGS pass below cannot accidentally consume the `$ARGS`
        // prefix of `${ARGS}`.
        let body = skill
            .body
            .replace("${ARGS}", args_str)
            .replace("$ARGS", args_str);

        Ok(SkillToolOutput {
            id: skill.meta.id.clone(),
            name: skill.meta.name.clone(),
            body,
            base_dir: skill
                .base_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            resources: skill.resources.iter().map(|r| r.rel_path.clone()).collect(),
        })
    }

    /// Fuzzy-match `query` against every registered id, returning up to 5
    /// candidates by descending similarity.
    ///
    /// Heuristic (no external dep):
    /// 1. either side contains the other (case-insensitive) → score 10,
    /// 2. otherwise the common-prefix length on lowercased ids,
    /// 3. ties broken alphabetically by id.
    fn fuzzy_suggestions(&self, query: &str) -> Vec<String> {
        let query_lower = query.to_lowercase();
        if query_lower.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(String, usize)> = self
            .skills
            .keys()
            .map(|id| {
                let id_lower = id.to_lowercase();
                let score = if id_lower.contains(query_lower.as_str())
                    || query_lower.contains(id_lower.as_str())
                {
                    10
                } else {
                    common_prefix_chars(&id_lower, &query_lower)
                };
                (id.clone(), score)
            })
            .filter(|(_, s)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        scored.into_iter().take(5).map(|(id, _)| id).collect()
    }
}

/// Build the full catalog line for a skill, with the description capped at
/// [`MAX_LISTING_DESC_CHARS`].
fn catalog_full_line(s: &Skill) -> String {
    let desc = truncate_desc(&s.meta.description, MAX_LISTING_DESC_CHARS);
    format!("- {}: {}", s.meta.id, desc)
}

/// Truncate a description to at most `cap` characters, ending with `…` when
/// truncation occurs. Returns the empty string when `cap == 0`.
fn truncate_desc(s: &str, cap: usize) -> String {
    if cap == 0 {
        return String::new();
    }
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Wrap a list of pre-formatted lines in the `<available-skills>` envelope
/// with the header text from the design doc.
fn wrap_catalog(lines: &[String]) -> String {
    format!(
        "<available-skills>\n{}\n\n{}\n</available-skills>",
        CATALOG_HEADER,
        lines.join("\n")
    )
}

/// Number of leading characters two strings share.
fn common_prefix_chars(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn resource_label(kind: crate::skill::ResourceKind) -> &'static str {
    use crate::skill::ResourceKind::*;
    match kind {
        Reference => "reference",
        Example => "example",
        Script => "script",
        Asset => "asset",
    }
}

/// Common words ignored during word-overlap matching (they carry no intent).
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "for", "in", "on", "at", "by", "with", "this", "that", "these",
    "those", "my", "our", "your", "it", "its", "is", "are", "be", "and", "or", "please", "can",
    "you", "me", "i", "we", "do", "some", "any", "as", "from",
];

/// Lower-case a word and strip surrounding non-alphanumeric punctuation.
fn normalize_word(w: &str) -> Option<String> {
    let cleaned: String = w
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Whether a normalized word is significant (not a stopword, length >= 3).
fn is_significant(w: &str) -> bool {
    w.len() >= 3 && !STOPWORDS.contains(&w)
}

/// Tokenize a (lower-cased) string into a set of significant words.
fn tokenize(s: &str) -> std::collections::BTreeSet<String> {
    s.split_whitespace()
        .filter_map(normalize_word)
        .filter(|w| is_significant(w))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontmatter;
    use crate::skill::{ResourceKind, SkillOrigin, SkillResource};

    fn skill(id: &str, name: &str, triggers_desc: &str) -> Skill {
        let fm = frontmatter::parse(&format!(
            "---\nname: {name}\ndescription: {triggers_desc}\n---\nDo the {name} thing."
        ));
        Skill::from_frontmatter(id, &fm, SkillOrigin::Workspace).unwrap()
    }

    fn registry() -> SkillRegistry {
        let mut reg = SkillRegistry::new();
        reg.register(skill(
            "pdf",
            "PDF Editor",
            "Use when the user asks to \"rotate a pdf\" or \"merge pdfs\".",
        ));
        reg.register(skill(
            "fe",
            "Frontend Design",
            "Use when the user asks to \"build a dashboard\" or \"design a landing page\".",
        ));
        reg
    }

    #[test]
    fn register_and_unregister() {
        let mut reg = registry();
        assert_eq!(reg.len(), 2);
        assert!(reg.contains("pdf"));
        let removed = reg.unregister("pdf").unwrap();
        assert_eq!(removed.meta.id, "pdf");
        assert!(!reg.contains("pdf"));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_replaces_and_returns_previous() {
        let mut reg = SkillRegistry::new();
        assert!(reg.register(skill("x", "V1", "\"do x\"")).is_none());
        let prev = reg.register(skill("x", "V2", "\"do x\"")).unwrap();
        assert_eq!(prev.meta.name, "V1");
        assert_eq!(reg.get("x").unwrap().meta.name, "V2");
    }

    #[test]
    fn passive_match_scores_by_trigger() {
        let reg = registry();
        let matches = reg.match_query("can you help me rotate a pdf for my report?");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, "pdf");
        assert!(matches[0]
            .matched_triggers
            .contains(&"rotate a pdf".to_string()));
    }

    #[test]
    fn best_match_picks_highest_score() {
        let reg = registry();
        let best = reg.best_match("I want to build a dashboard").unwrap();
        assert_eq!(best.id, "fe");
    }

    #[test]
    fn no_match_returns_empty() {
        let reg = registry();
        assert!(reg.match_query("what is the weather today").is_empty());
        assert!(reg.best_match("unrelated").is_none());
    }

    #[test]
    fn activate_returns_body_fragment() {
        let reg = registry();
        let frag = reg.activate("pdf").unwrap();
        assert_eq!(frag.source, PromptSource::WorkspaceRules);
        assert!(frag.content.contains("# Skill: PDF Editor"));
        assert!(frag.content.contains("Do the PDF Editor thing"));
    }

    #[test]
    fn activate_unknown_is_none() {
        let reg = registry();
        assert!(reg.activate("nope").is_none());
    }

    #[test]
    fn auto_activate_matches_then_discloses() {
        let reg = registry();
        let (id, frag) = reg.auto_activate("please merge pdfs for me").unwrap();
        assert_eq!(id, "pdf");
        assert!(frag.content.contains("PDF Editor"));
    }

    #[test]
    fn activate_lists_resources() {
        let mut reg = SkillRegistry::new();
        let mut s = skill("pdf", "PDF", "\"rotate a pdf\"");
        s.resources.push(SkillResource {
            kind: ResourceKind::Script,
            rel_path: "scripts/rotate.py".into(),
        });
        reg.register(s);
        let frag = reg.activate("pdf").unwrap();
        assert!(frag.content.contains("scripts/rotate.py"));
        assert!(frag.content.contains("[script]"));
    }

    #[test]
    fn catalog_blurb_lists_all() {
        let reg = registry();
        let blurb = reg.catalog_blurb();
        assert!(blurb.contains("PDF Editor"));
        assert!(blurb.contains("Frontend Design"));
    }

    // ---------------------------------------------------------------------
    // formatted_catalog
    // ---------------------------------------------------------------------

    /// Helper: register a skill with a controlled origin.
    fn skill_with_origin(id: &str, name: &str, desc: &str, origin: SkillOrigin) -> Skill {
        let fm = frontmatter::parse(&format!(
            "---\nname: {name}\ndescription: {desc}\n---\nbody for {name}"
        ));
        Skill::from_frontmatter(id, &fm, origin).unwrap()
    }

    #[test]
    fn formatted_catalog_empty_returns_empty_string() {
        let reg = SkillRegistry::new();
        assert_eq!(reg.formatted_catalog(8000), "");
    }

    #[test]
    fn formatted_catalog_skips_disable_model_invocation() {
        let mut reg = SkillRegistry::new();
        let mut hidden =
            skill_with_origin("hidden", "Hidden", "\"do hidden\"", SkillOrigin::Workspace);
        hidden.meta.disable_model_invocation = true;
        reg.register(hidden);
        reg.register(skill_with_origin(
            "visible",
            "Visible",
            "\"do visible\"",
            SkillOrigin::Workspace,
        ));

        let out = reg.formatted_catalog(8000);
        assert!(out.contains("<available-skills>"));
        assert!(out.contains("</available-skills>"));
        assert!(out.contains("- visible:"));
        assert!(!out.contains("- hidden"));
    }

    #[test]
    fn formatted_catalog_fits_within_budget() {
        let mut reg = SkillRegistry::new();
        reg.register(skill_with_origin(
            "alpha",
            "Alpha",
            "\"do alpha\"",
            SkillOrigin::Workspace,
        ));
        reg.register(skill_with_origin(
            "beta",
            "Beta",
            "\"do beta\"",
            SkillOrigin::Workspace,
        ));
        reg.register(skill_with_origin(
            "gamma",
            "Gamma",
            "\"do gamma\"",
            SkillOrigin::Workspace,
        ));

        let out = reg.formatted_catalog(8000);
        assert!(out.starts_with("<available-skills>"));
        assert!(out.ends_with("</available-skills>"));
        assert!(out
            .contains("The following skills are available. When a user request matches a skill's"));
        assert!(out.contains("- alpha: "));
        assert!(out.contains("- beta: "));
        assert!(out.contains("- gamma: "));
        // No truncation at 8000 budget.
        assert!(!out.contains("…"));
    }

    #[test]
    fn formatted_catalog_truncates_under_budget() {
        let mut reg = SkillRegistry::new();
        let long_desc = format!("\"{} something more here\"", "x".repeat(200));
        for i in 0..5 {
            reg.register(skill_with_origin(
                &format!("skill-{i}"),
                &format!("S{i}"),
                &long_desc,
                SkillOrigin::Workspace,
            ));
        }

        let budget = 600;
        let out = reg.formatted_catalog(budget);
        let len = out.chars().count();
        assert!(len > 0, "should still emit a reminder when over budget");
        assert!(
            len <= budget + 50,
            "expected output within budget+50 slack, got {len} for budget {budget}"
        );
        // Truncation happened — at least one ellipsis or names-only line.
        assert!(
            out.contains("…")
                || out
                    .lines()
                    .any(|l| l.starts_with("- skill-") && !l.contains(": "))
        );
    }

    #[test]
    fn formatted_catalog_preserves_builtin_descriptions() {
        let mut reg = SkillRegistry::new();
        reg.register(skill_with_origin(
            "core-builtin",
            "Core",
            "\"long builtin description that is very long indeed\"",
            SkillOrigin::BuiltIn,
        ));
        let long_desc = format!("\"{}\"", "z".repeat(150));
        for i in 0..5 {
            reg.register(skill_with_origin(
                &format!("ws-{i}"),
                &format!("W{i}"),
                &long_desc,
                SkillOrigin::Workspace,
            ));
        }

        let out = reg.formatted_catalog(300);
        // Built-in description survives in full even when budget is tight.
        assert!(
            out.contains("long builtin description that is very long indeed"),
            "built-in description must not be truncated, got:\n{out}"
        );
        // Built-in line is a full `- id: desc` line.
        assert!(out.lines().any(|l| l.starts_with("- core-builtin: ")));
    }

    #[test]
    fn formatted_catalog_per_line_cap() {
        let mut reg = SkillRegistry::new();
        let long = "a".repeat(500);
        let desc = format!("\"{long}\"");
        reg.register(skill_with_origin(
            "big",
            "Big",
            &desc,
            SkillOrigin::Workspace,
        ));

        let out = reg.formatted_catalog(8000);
        let line = out
            .lines()
            .find(|l| l.starts_with("- big:"))
            .expect("big line present");
        assert!(
            line.chars().count() <= MAX_LISTING_DESC_CHARS + 10,
            "line len {} exceeded cap {} + 10",
            line.chars().count(),
            MAX_LISTING_DESC_CHARS
        );
        assert!(line.ends_with('…'), "line should end with ellipsis");
    }

    #[test]
    fn formatted_catalog_zero_budget_returns_empty() {
        let mut reg = SkillRegistry::new();
        reg.register(skill_with_origin(
            "alpha",
            "Alpha",
            "\"do alpha\"",
            SkillOrigin::Workspace,
        ));
        assert_eq!(reg.formatted_catalog(0), "");
    }

    #[test]
    fn formatted_catalog_extreme_budget_degrades_to_names_only() {
        let mut reg = SkillRegistry::new();
        for i in 0..10 {
            reg.register(skill_with_origin(
                &format!("s{i}"),
                &format!("S{i}"),
                &format!("\"long description number {i} that takes some space\""),
                SkillOrigin::Workspace,
            ));
        }
        let out = reg.formatted_catalog(100);
        let names_only_lines: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("- s") && !l.contains(": "))
            .collect();
        assert_eq!(
            names_only_lines.len(),
            10,
            "all 10 non-builtin lines should be names-only, got:\n{out}"
        );
    }

    // ---------------------------------------------------------------------
    // body_for_invoke
    // ---------------------------------------------------------------------

    #[test]
    fn body_for_invoke_returns_body_with_args_substituted() {
        let mut reg = SkillRegistry::new();
        let fm = frontmatter::parse(
            "---\nname: Echo\ndescription: \"echo args\"\n---\nargs are: ${ARGS}",
        );
        let s = Skill::from_frontmatter("echo", &fm, SkillOrigin::Workspace).unwrap();
        reg.register(s);
        let out = reg.body_for_invoke("echo", Some("hello world")).unwrap();
        assert_eq!(out.id, "echo");
        assert_eq!(out.name, "Echo");
        assert_eq!(out.body, "args are: hello world");
    }

    #[test]
    fn body_for_invoke_substitutes_dollar_args() {
        let mut reg = SkillRegistry::new();
        let fm = frontmatter::parse(
            "---\nname: Echo\ndescription: \"echo args\"\n---\nbraced ${ARGS} bare $ARGS end",
        );
        let s = Skill::from_frontmatter("echo", &fm, SkillOrigin::Workspace).unwrap();
        reg.register(s);
        let out = reg.body_for_invoke("echo", Some("foo")).unwrap();
        assert!(!out.body.contains("$ARGS"));
        assert!(!out.body.contains("${ARGS}"));
        assert_eq!(out.body, "braced foo bare foo end");
    }

    #[test]
    fn body_for_invoke_no_args_clears_placeholder() {
        let mut reg = SkillRegistry::new();
        let fm = frontmatter::parse(
            "---\nname: Echo\ndescription: \"echo args\"\n---\n[${ARGS}] and [$ARGS]",
        );
        let s = Skill::from_frontmatter("echo", &fm, SkillOrigin::Workspace).unwrap();
        reg.register(s);
        let out = reg.body_for_invoke("echo", None).unwrap();
        assert_eq!(out.body, "[] and []");
    }

    #[test]
    fn body_for_invoke_includes_base_dir_and_resources() {
        use crate::skill::{ResourceKind, SkillResource};
        use std::path::PathBuf;

        let mut reg = SkillRegistry::new();
        let mut s = skill_with_origin(
            "with-res",
            "WithRes",
            "\"has resources\"",
            SkillOrigin::Workspace,
        );
        s.base_dir = Some(PathBuf::from("/some/abs/path/with-res"));
        s.resources.push(SkillResource {
            kind: ResourceKind::Script,
            rel_path: "scripts/run.py".into(),
        });
        s.resources.push(SkillResource {
            kind: ResourceKind::Reference,
            rel_path: "references/notes.md".into(),
        });
        reg.register(s);

        let out = reg.body_for_invoke("with-res", None).unwrap();
        let base = out.base_dir.expect("base_dir present");
        assert!(
            base.contains("with-res"),
            "expected base_dir to carry id, got {base}"
        );
        assert_eq!(
            out.resources,
            vec![
                "scripts/run.py".to_string(),
                "references/notes.md".to_string()
            ]
        );
    }

    #[test]
    fn body_for_invoke_unknown_id_returns_suggestions() {
        let mut reg = SkillRegistry::new();
        reg.register(skill_with_origin(
            "planning-with-files",
            "Planning",
            "\"plan files\"",
            SkillOrigin::BuiltIn,
        ));
        reg.register(skill_with_origin(
            "code-review-skill",
            "Review",
            "\"review code\"",
            SkillOrigin::BuiltIn,
        ));
        reg.register(skill_with_origin(
            "agent-browser",
            "Browser",
            "\"browse\"",
            SkillOrigin::BuiltIn,
        ));

        let err = reg.body_for_invoke("planning", None).unwrap_err();
        match err {
            BodyForInvokeError::NotFound { id, suggestions } => {
                assert_eq!(id, "planning");
                assert!(!suggestions.is_empty(), "expected at least one suggestion");
                assert!(suggestions.len() <= 5);
                assert!(
                    suggestions.iter().any(|s| s.contains("planning")),
                    "planning-with-files should be suggested, got {suggestions:?}"
                );
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn body_for_invoke_disable_model_invocation_returns_error() {
        let mut reg = SkillRegistry::new();
        let mut s = skill_with_origin(
            "user-only",
            "UserOnly",
            "\"only via /user-only\"",
            SkillOrigin::Workspace,
        );
        s.meta.disable_model_invocation = true;
        reg.register(s);

        let err = reg
            .body_for_invoke("user-only", Some("ignored"))
            .unwrap_err();
        // Display message hints at /id form for the user.
        let msg = format!("{err}");
        assert!(msg.contains("/user-only"), "msg: {msg}");
        match err {
            BodyForInvokeError::DisabledForModel { id } => assert_eq!(id, "user-only"),
            other => panic!("expected DisabledForModel, got {other:?}"),
        }
    }

    #[test]
    fn body_for_invoke_fuzzy_match_finds_close_id() {
        let mut reg = SkillRegistry::new();
        reg.register(skill_with_origin(
            "planning-with-files",
            "Planning",
            "\"plan\"",
            SkillOrigin::BuiltIn,
        ));
        reg.register(skill_with_origin(
            "code-review-skill",
            "Review",
            "\"review\"",
            SkillOrigin::BuiltIn,
        ));

        let err = reg.body_for_invoke("plan-with-files", None).unwrap_err();
        match err {
            BodyForInvokeError::NotFound { suggestions, .. } => {
                assert!(
                    suggestions.iter().any(|s| s == "planning-with-files"),
                    "expected planning-with-files in suggestions, got {suggestions:?}"
                );
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }
}
