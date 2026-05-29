//! Markdown observations (the "md" in Anthropic's md + embedding + BM25 +
//! rerank stack), modeled on claude-mem's observation schema.
//!
//! claude-mem stores memories as structured *observations* — `type`, `title`,
//! `subtitle`, `facts`, `narrative`, `concepts`, `files` — and renders them as
//! markdown for both display and indexing. We mirror that schema. An
//! [`Observation`] converts to a [`MemoryItem`] (so it flows through the
//! existing ranking/persistence machinery) and renders to markdown
//! ([`Observation::to_markdown`]) which is what gets embedded and BM25-indexed.

use serde::{Deserialize, Serialize};

use deepagent_core::clock::Timestamp;

use crate::{MemoryItem, MemoryTier};

/// The kind of observation (claude-mem uses these to type memories).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationType {
    /// A decision and its rationale.
    Decision,
    /// A bug fix.
    BugFix,
    /// A new feature / capability.
    Feature,
    /// A piece of knowledge / fact learned.
    Knowledge,
    /// A failure / dead-end to avoid repeating.
    Failure,
    /// Project / workspace structure.
    Workspace,
}

impl ObservationType {
    /// Map an observation type onto the memory tier it belongs to.
    pub fn tier(&self) -> MemoryTier {
        match self {
            ObservationType::Decision => MemoryTier::Procedural,
            ObservationType::BugFix | ObservationType::Feature => MemoryTier::Episodic,
            ObservationType::Knowledge => MemoryTier::Semantic,
            ObservationType::Failure => MemoryTier::Failure,
            ObservationType::Workspace => MemoryTier::Workspace,
        }
    }

    /// Short label.
    pub fn label(&self) -> &'static str {
        match self {
            ObservationType::Decision => "decision",
            ObservationType::BugFix => "bugfix",
            ObservationType::Feature => "feature",
            ObservationType::Knowledge => "knowledge",
            ObservationType::Failure => "failure",
            ObservationType::Workspace => "workspace",
        }
    }
}

/// A structured, markdown-renderable memory observation (claude-mem style).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// What kind of observation this is.
    pub obs_type: ObservationType,
    /// One-line title.
    pub title: String,
    /// Optional subtitle / context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Bullet-point facts.
    #[serde(default)]
    pub facts: Vec<String>,
    /// A free-form narrative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub narrative: Option<String>,
    /// Concept tags (used for filtering + boosting).
    #[serde(default)]
    pub concepts: Vec<String>,
    /// Files touched / referenced.
    #[serde(default)]
    pub files: Vec<String>,
    /// Baseline importance in [0,1].
    pub importance: f32,
}

impl Observation {
    /// A minimal observation.
    pub fn new(obs_type: ObservationType, title: impl Into<String>) -> Self {
        Self {
            obs_type,
            title: title.into(),
            subtitle: None,
            facts: Vec::new(),
            narrative: None,
            concepts: Vec::new(),
            files: Vec::new(),
            importance: 0.5,
        }
    }

    /// Set the subtitle (builder).
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = Some(s.into());
        self
    }

    /// Set the narrative (builder).
    pub fn narrative(mut self, s: impl Into<String>) -> Self {
        self.narrative = Some(s.into());
        self
    }

    /// Add facts (builder).
    pub fn facts(mut self, facts: impl IntoIterator<Item = String>) -> Self {
        self.facts = facts.into_iter().collect();
        self
    }

    /// Add concept tags (builder).
    pub fn concepts(mut self, concepts: impl IntoIterator<Item = String>) -> Self {
        self.concepts = concepts.into_iter().collect();
        self
    }

    /// Add files (builder).
    pub fn files(mut self, files: impl IntoIterator<Item = String>) -> Self {
        self.files = files.into_iter().collect();
        self
    }

    /// Set importance (builder), clamped to [0,1].
    pub fn importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }

    /// Render the observation as markdown — the canonical text that is embedded
    /// and BM25-indexed, and shown in context.
    pub fn to_markdown(&self) -> String {
        let mut md = format!("## [{}] {}\n", self.obs_type.label(), self.title);
        if let Some(sub) = &self.subtitle {
            md.push_str(&format!("_{sub}_\n"));
        }
        if let Some(narr) = &self.narrative {
            md.push_str(&format!("\n{narr}\n"));
        }
        if !self.facts.is_empty() {
            md.push_str("\nFacts:\n");
            for f in &self.facts {
                md.push_str(&format!("- {f}\n"));
            }
        }
        if !self.concepts.is_empty() {
            md.push_str(&format!("\nConcepts: {}\n", self.concepts.join(", ")));
        }
        if !self.files.is_empty() {
            md.push_str(&format!("Files: {}\n", self.files.join(", ")));
        }
        md.trim_end().to_string()
    }

    /// The plain searchable text (title + subtitle + narrative + facts +
    /// concepts), used for embedding/BM25 without markdown punctuation noise.
    pub fn searchable_text(&self) -> String {
        let mut parts = vec![self.title.clone()];
        if let Some(s) = &self.subtitle {
            parts.push(s.clone());
        }
        if let Some(n) = &self.narrative {
            parts.push(n.clone());
        }
        parts.extend(self.facts.iter().cloned());
        parts.extend(self.concepts.iter().cloned());
        parts.join(" ")
    }

    /// Convert into a [`MemoryItem`] (markdown becomes the item content, so the
    /// rest of the memory machinery — ranking, persistence — works unchanged).
    pub fn into_memory_item(self, now: Timestamp) -> MemoryItem {
        let tier = self.obs_type.tier();
        let importance = self.importance;
        let concepts = self.concepts.clone();
        let content = self.to_markdown();
        MemoryItem::new(tier, content, importance, now).with_tags(concepts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn renders_markdown_with_sections() {
        let obs = Observation::new(ObservationType::BugFix, "Fix payment timeout")
            .subtitle("retry logic was too aggressive")
            .facts(["increased base delay".into(), "added jitter".into()])
            .concepts(["payment".into(), "retry".into()])
            .files(["payment/retry.rs".into()]);
        let md = obs.to_markdown();
        assert!(md.contains("## [bugfix] Fix payment timeout"));
        assert!(md.contains("_retry logic was too aggressive_"));
        assert!(md.contains("- increased base delay"));
        assert!(md.contains("Concepts: payment, retry"));
        assert!(md.contains("Files: payment/retry.rs"));
    }

    #[test]
    fn type_maps_to_tier() {
        assert_eq!(ObservationType::Failure.tier(), MemoryTier::Failure);
        assert_eq!(ObservationType::Knowledge.tier(), MemoryTier::Semantic);
        assert_eq!(ObservationType::Decision.tier(), MemoryTier::Procedural);
    }

    #[test]
    fn searchable_text_excludes_markdown() {
        let obs = Observation::new(ObservationType::Knowledge, "Title")
            .narrative("some narrative")
            .facts(["a fact".into()]);
        let text = obs.searchable_text();
        assert!(text.contains("Title"));
        assert!(text.contains("some narrative"));
        assert!(text.contains("a fact"));
        assert!(!text.contains('#'));
    }

    #[test]
    fn into_memory_item_carries_tier_and_tags() {
        let item = Observation::new(ObservationType::Failure, "broke prod")
            .concepts(["deploy".into()])
            .importance(0.9)
            .into_memory_item(at(0));
        assert_eq!(item.tier, MemoryTier::Failure);
        assert!((item.importance - 0.9).abs() < 1e-6);
        assert_eq!(item.tags, vec!["deploy".to_string()]);
        assert!(item.content.contains("broke prod"));
    }
}
