//! Canvas creative prompt profiles and skills.
//!
//! One `PromptProfile` belongs to exactly one canvas node kind, keyed
//! `creative.<node_kind>.v1`. Nodes persist only the profile id (plus optional
//! user overrides); the system prompt and the default user prompt stay here, so
//! a prompt can never drift onto the wrong node or be rewritten by a provider
//! choice. Resources are compiled in, which keeps a packaged desktop build
//! working without shipping a resource directory.

use std::sync::OnceLock;

use deepagent_core::error::{CoreError, Result};
use serde::{Deserialize, Serialize};

/// Largest accepted system prompt, in characters.
const MAX_PROMPT_CHARS: usize = 8_192;

/// A prompt profile bound to one canvas node kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptProfile {
    /// `creative.<node_kind>.v<version>`.
    pub id: String,
    /// Bumped only when the prompt text changes materially.
    pub version: u32,
    /// The single canvas node kind this profile belongs to.
    pub node_kind: String,
    /// How the profile is consumed; it never selects a provider.
    pub prompt_usage: String,
    /// Node-scoped system prompt; never stored on the node itself.
    pub system_prompt: String,
    /// Prompt shown in the node until the user edits it.
    pub default_user_prompt: String,
    /// Shape the node must produce.
    pub output_contract: String,
    /// Attachment kinds the node may receive.
    pub accepted_attachments: Vec<String>,
    /// Skills enabled by default for this node kind.
    pub default_skill_ids: Vec<String>,
    /// Disabled profiles are excluded from resolution.
    pub enabled: bool,
}

impl PromptProfile {
    /// Defaults are model input only for usages that actually invoke a model.
    /// Keep the wire's existing usage labels, and reject unknown labels rather
    /// than interpreting a misspelled model usage as a prompt-free local node.
    pub fn requires_default_user_prompt(&self) -> Result<bool> {
        match self.prompt_usage.as_str() {
            "IntentNormalization"
            | "ArtifactContract"
            | "ConstraintContract"
            | "LocalVideoCompose"
            | "local_editor" => Ok(false),
            "ResponsesText"
            | "CanvasImage"
            | "LocalCompareOrSemanticVision"
            | "CanvasImageEdit"
            | "MediaJobVideo"
            | "ResponsesDirectionPlan"
            | "ResponsesTemplateExpansion"
            | "CharacterContext"
            | "SpeechMedia"
            | "ResponsesStoryboard" => Ok(true),
            _ => Err(CoreError::invalid(format!(
                "canvas prompt profile `{}` has an unknown promptUsage `{}`",
                self.id, self.prompt_usage
            ))),
        }
    }
}

/// A provider-agnostic skill block a node may attach.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CanvasSkill {
    /// Versioned skill id, e.g. `image.style.extract.v1`.
    pub id: String,
    /// Numeric part of the id.
    pub version: u32,
    /// Display name without the version suffix.
    pub name: String,
    /// Instruction block appended to the node prompt.
    pub system_prompt: String,
    /// Disabled skills are never injected.
    pub enabled: bool,
}

macro_rules! embed_profiles {
    ($($node:literal),+ $(,)?) => {{
        static PROFILES: OnceLock<Vec<PromptProfile>> = OnceLock::new();
        PROFILES.get_or_init(|| {
            [
                $(include_str!(concat!("../resources/canvas/creative/", $node, ".json"))),+
            ]
            .into_iter()
            .map(|raw| {
                serde_json::from_str::<PromptProfile>(raw)
                    .unwrap_or_else(|error| {
                        panic!("invalid canvas prompt profile resource: {error}")
                    })
            })
            .collect()
        })
    }};
}

macro_rules! embed_skills {
    ($($skill:literal),+ $(,)?) => {{
        static SKILLS: OnceLock<Vec<CanvasSkill>> = OnceLock::new();
        SKILLS.get_or_init(|| {
            [
                $(include_str!(concat!("../resources/canvas/skill/", $skill, ".json"))),+
            ]
            .into_iter()
            .map(|raw| {
                serde_json::from_str::<CanvasSkill>(raw)
                    .unwrap_or_else(|error| panic!("invalid canvas skill resource: {error}"))
            })
            .collect()
        })
    }};
}

/// Every canvas node kind that must own exactly one profile.
pub const CREATIVE_NODE_KINDS: [&str; 21] = [
    "category-picker",
    "text-gen",
    "image-input",
    "image-gen",
    "image-compare",
    "image-edit",
    "script-gen",
    "video-gen",
    "video-edit",
    "video-stitch",
    "camera",
    "lens",
    "focal-length",
    "aperture",
    "director",
    "creative-template",
    "character-face",
    "character-body",
    "character-style",
    "audio",
    "storyboard-grid",
];

/// All embedded creative profiles in registry order.
pub fn creative_profiles() -> &'static [PromptProfile] {
    embed_profiles!(
        "category-picker",
        "text-gen",
        "image-input",
        "image-gen",
        "image-compare",
        "image-edit",
        "script-gen",
        "video-gen",
        "video-edit",
        "video-stitch",
        "camera",
        "lens",
        "focal-length",
        "aperture",
        "director",
        "creative-template",
        "character-face",
        "character-body",
        "character-style",
        "audio",
        "storyboard-grid",
    )
    .as_slice()
}

/// All embedded canvas skills.
pub fn canvas_skills() -> &'static [CanvasSkill] {
    embed_skills!(
        "image.prompt.optimize.v1",
        "image.reference.replicate.v1",
        "image.style.extract.v1",
        "image.render.enhance.v1",
    )
    .as_slice()
}

/// The single profile for a node kind. An unregistered node kind is an error,
/// never a silent fall back to a generic prompt.
pub fn creative_profile(node_kind: &str) -> Result<&'static PromptProfile> {
    let node_kind = node_kind.trim();
    creative_profiles()
        .iter()
        .find(|profile| profile.node_kind == node_kind)
        .ok_or_else(|| CoreError::not_found(format!("UnsupportedNode: canvas node `{node_kind}`")))
}

/// Look up a skill by id.
pub fn canvas_skill(skill_id: &str) -> Option<&'static CanvasSkill> {
    canvas_skills().iter().find(|skill| skill.id == skill_id)
}

/// Reject profiles that could bind a prompt to the wrong node or smuggle a
/// vendor name into a supposedly provider-neutral prompt.
pub fn validate_canvas_prompts() -> Result<()> {
    validate_profiles(creative_profiles())
}

fn validate_profiles(profiles: &[PromptProfile]) -> Result<()> {
    if profiles.len() != CREATIVE_NODE_KINDS.len() {
        return Err(CoreError::invalid(format!(
            "canvas prompt profiles expect {} entries, found {}",
            CREATIVE_NODE_KINDS.len(),
            profiles.len()
        )));
    }

    let mut seen_kinds = std::collections::BTreeSet::new();
    let mut seen_ids = std::collections::BTreeSet::new();
    for profile in profiles {
        if !seen_kinds.insert(profile.node_kind.as_str()) {
            return Err(CoreError::invalid(format!(
                "duplicate canvas prompt profile for node `{}`",
                profile.node_kind
            )));
        }
        if !CREATIVE_NODE_KINDS.contains(&profile.node_kind.as_str()) {
            return Err(CoreError::invalid(format!(
                "canvas prompt profile references unregistered node `{}`",
                profile.node_kind
            )));
        }
        let expected_id = format!("creative.{}.v{}", profile.node_kind, profile.version);
        if profile.id != expected_id {
            return Err(CoreError::invalid(format!(
                "canvas prompt profile id `{}` must be `{expected_id}`",
                profile.id
            )));
        }
        if !seen_ids.insert(profile.id.as_str()) {
            return Err(CoreError::invalid(format!(
                "duplicate canvas prompt profile id `{}`",
                profile.id
            )));
        }
        if profile.version == 0 || !profile.enabled {
            return Err(CoreError::invalid(format!(
                "canvas prompt profile `{}` must be enabled with a positive version",
                profile.id
            )));
        }
        let needs_user_prompt = profile.requires_default_user_prompt()?;
        for (field, value, required) in [
            ("systemPrompt", &profile.system_prompt, true),
            (
                "defaultUserPrompt",
                &profile.default_user_prompt,
                needs_user_prompt,
            ),
        ] {
            let trimmed = value.trim();
            if required && trimmed.is_empty() {
                return Err(CoreError::invalid(format!(
                    "canvas prompt profile `{}` has an empty {field}",
                    profile.id
                )));
            }
            if value.chars().count() > MAX_PROMPT_CHARS {
                return Err(CoreError::invalid(format!(
                    "canvas prompt profile `{}` exceeds {MAX_PROMPT_CHARS} characters in {field}",
                    profile.id
                )));
            }
        }
        if let Some(vendor) = forbidden_vendor_token(&profile.system_prompt) {
            return Err(CoreError::invalid(format!(
                "canvas prompt profile `{}` must stay provider-neutral but mentions `{vendor}`",
                profile.id
            )));
        }
        for skill_id in &profile.default_skill_ids {
            if canvas_skill(skill_id).is_none() {
                return Err(CoreError::invalid(format!(
                    "canvas prompt profile `{}` references unknown skill `{skill_id}`",
                    profile.id
                )));
            }
        }
    }

    if let Some(missing) = CREATIVE_NODE_KINDS
        .iter()
        .find(|kind| !profiles.iter().any(|profile| profile.node_kind == **kind))
    {
        return Err(CoreError::invalid(format!(
            "canvas node `{missing}` has no prompt profile"
        )));
    }

    for skill in canvas_skills() {
        if let Some(vendor) = forbidden_vendor_token(&skill.system_prompt) {
            return Err(CoreError::invalid(format!(
                "canvas skill `{}` must stay provider-neutral but mentions `{vendor}`",
                skill.id
            )));
        }
        if skill.system_prompt.trim().is_empty() {
            return Err(CoreError::invalid(format!(
                "canvas skill `{}` has an empty prompt",
                skill.id
            )));
        }
    }
    Ok(())
}

/// Vendor identifiers that must not appear in canvas prompts or skills: prompt
/// selection is node-driven, endpoint selection is router-driven.
fn forbidden_vendor_token(text: &str) -> Option<&'static str> {
    const FORBIDDEN: &[&str] = &[
        "OpenAI",
        "openai",
        "Anthropic",
        "anthropic",
        "Gemini",
        "gemini",
        "DeepSeek",
        "deepseek",
        "Midjourney",
        "Stable Diffusion",
        "gpt-",
        "claude-",
    ];
    FORBIDDEN
        .iter()
        .find(|token| text.contains(**token))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_node_kind_owns_exactly_one_profile() {
        validate_canvas_prompts().expect("canvas prompts valid");
        assert_eq!(creative_profiles().len(), CREATIVE_NODE_KINDS.len());
        assert_eq!(canvas_skills().len(), 4);
    }

    #[test]
    fn h306_empty_defaults_follow_usage_without_relaxing_model_profiles() {
        for original in creative_profiles() {
            let mut profiles = creative_profiles().to_vec();
            let profile = profiles.iter_mut().find(|p| p.id == original.id).unwrap();
            profile.default_user_prompt.clear();
            let result = validate_profiles(&profiles);
            if original.requires_default_user_prompt().unwrap() {
                let error = result
                    .expect_err("model profiles still need default input")
                    .to_string();
                assert!(error.contains(&original.id) && error.contains("empty defaultUserPrompt"));
            } else {
                result.expect("local and contract usages do not require model input");
            }
        }
    }

    #[test]
    fn h306_prompt_free_usage_still_rejects_broken_resources() {
        for field in [
            "system",
            "usage",
            "length",
            "whitespace_length",
            "identity",
            "skill",
            "disabled",
        ] {
            let mut profiles = creative_profiles().to_vec();
            let local = profiles
                .iter_mut()
                .find(|p| p.node_kind == "video-edit")
                .unwrap();
            match field {
                "system" => local.system_prompt.clear(),
                "usage" => local.prompt_usage = "local_editro".into(),
                "length" => local.default_user_prompt = "字".repeat(MAX_PROMPT_CHARS + 1),
                "whitespace_length" => local.default_user_prompt = " ".repeat(MAX_PROMPT_CHARS + 1),
                "identity" => local.id = "creative.text-gen.v1".into(),
                "skill" => local.default_skill_ids.push("unknown-skill".into()),
                "disabled" => local.enabled = false,
                _ => unreachable!(),
            }
            assert!(
                validate_profiles(&profiles).is_err(),
                "invalid {field} must fail closed"
            );
        }
    }

    #[test]
    fn profile_binding_is_one_to_one() {
        for kind in CREATIVE_NODE_KINDS {
            let profile = creative_profile(kind)
                .unwrap_or_else(|error| panic!("node `{kind}` must resolve a profile: {error}"));
            assert_eq!(profile.node_kind, kind);
            assert_eq!(profile.id, format!("creative.{kind}.v{}", profile.version));
        }
        let error = creative_profile("does-not-exist").expect_err("unknown node");
        assert!(error.to_string().contains("UnsupportedNode"));
    }

    #[test]
    fn image_generation_and_edit_prompts_are_not_interchangeable() {
        let gen = creative_profile("image-gen").expect("gen");
        let edit = creative_profile("image-edit").expect("edit");
        assert_ne!(gen.id, edit.id);
        assert_ne!(gen.system_prompt, edit.system_prompt);
        assert!(gen
            .system_prompt
            .contains("不得自行选择 ImageGenerate 或 ImageEdit"));
        assert!(edit.system_prompt.contains("不得自动降级为普通图片生成"));
        assert!(!edit
            .default_skill_ids
            .contains(&"image.style.extract.v1".to_string()));
    }

    #[test]
    fn local_and_contract_only_nodes_stay_deterministic() {
        let stitch = creative_profile("video-stitch").expect("stitch");
        assert!(stitch.system_prompt.contains("不需要大模型参与"));
        let camera = creative_profile("camera").expect("camera");
        assert_eq!(camera.prompt_usage, "ConstraintContract");
        assert!(camera.accepted_attachments.contains(&"Json".to_string()));
    }
}
