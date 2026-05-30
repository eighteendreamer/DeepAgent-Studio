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

use crate::skill::{Skill, SkillMeta};

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
}
