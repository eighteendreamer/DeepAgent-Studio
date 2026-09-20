//! Canvas node contracts: the backend's single source of truth per node kind.
//!
//! A contract states which operations a node may run, which executor serves it,
//! which prompt profile it owns and which skills it may attach. The router reads
//! this instead of inferring intent from labels, and an unregistered node kind is
//! a hard error rather than a passthrough.

use deepagent_core::error::{CoreError, Result};
use deepagent_prompts::canvas_prompt::{canvas_skills, creative_profile, creative_profiles};
use deepagent_runtime::workflow::{compile, WorkflowDefinition, WorkflowNodeSpec};
use serde::{Deserialize, Serialize};

use crate::canvas_model_gateway::CanvasOperation;

/// How a canvas node is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanvasExecutorKind {
    /// Emits its configured contract only; never calls a model.
    ContractOnly,
    /// Deterministic local processing.
    Local,
    /// Runs on the chat/text model path.
    Responses,
    /// Runs on the canvas media gateway (images, speech).
    CanvasMedia,
    /// Runs the embedding endpoint.
    Embedding,
    /// Runs through an asynchronous media job backend.
    MediaJob,
    /// Combines local work with a model call.
    Composite,
    /// Registered but not executable yet; running it must fail loudly.
    Unsupported,
}

/// What a node accepts and produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasIoContract {
    pub accepted_input_kinds: Vec<String>,
    pub output_kind: String,
}

/// One node kind's full contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasNodeContract {
    pub node_kind: String,
    pub prompt_profile_id: String,
    pub executor_kind: CanvasExecutorKind,
    pub allowed_operations: Vec<CanvasOperation>,
    pub input_contract: CanvasIoContract,
    pub allowed_skill_ids: Vec<String>,
}

macro_rules! contract {
    (
        node: $node:literal,
        executor: $executor:expr,
        operations: [$($operation:ident),* $(,)?],
        inputs: [$($input:literal),* $(,)?],
        output: $output:literal,
        skills: [$($skill:literal),* $(,)?],
    ) => {
        CanvasNodeContract {
            node_kind: $node.to_string(),
            prompt_profile_id: concat!("creative.", $node, ".v1").to_string(),
            executor_kind: $executor,
            allowed_operations: vec![$(CanvasOperation::$operation),*],
            input_contract: CanvasIoContract {
                accepted_input_kinds: vec![$($input.to_string()),*],
                output_kind: $output.to_string(),
            },
            allowed_skill_ids: vec![$($skill.to_string()),*],
        }
    };
}

use CanvasExecutorKind as Executor;

/// Every registered creative node, in plan order.
pub fn canvas_node_contracts() -> Vec<CanvasNodeContract> {
    vec![
        contract! {
            node: "category-picker",
            executor: Executor::ContractOnly,
            operations: [NormalizeIntent],
            inputs: ["Text", "Json", "Image", "Video", "Audio"],
            output: "OperationIntent(Json)",
            skills: [],
        },
        contract! {
            node: "text-gen",
            executor: Executor::Responses,
            operations: [TextGenerate, VisionDescribe],
            inputs: ["Text", "Document", "Image", "Json"],
            output: "Text",
            skills: [],
        },
        contract! {
            node: "image-input",
            executor: Executor::Local,
            operations: [ArtifactImport],
            inputs: ["ExternalImage"],
            output: "ImageArtifact",
            skills: [],
        },
        contract! {
            node: "image-gen",
            executor: Executor::CanvasMedia,
            operations: [ImageGenerate, ImageEdit],
            inputs: ["Text", "Image", "Json"],
            output: "ImageArtifact",
            skills: [
                "image.prompt.optimize.v1",
                "image.reference.replicate.v1",
                "image.style.extract.v1",
                "image.render.enhance.v1",
            ],
        },
        contract! {
            node: "image-compare",
            executor: Executor::Composite,
            operations: [ImageCompare],
            inputs: ["ImageList"],
            output: "CompareResult(Json)",
            skills: [],
        },
        contract! {
            node: "image-edit",
            executor: Executor::Composite,
            operations: [
                ImageEdit,
                ImageCrop,
                ImageRemoveBackground,
                ImageUpscale,
                ImageRepaint,
            ],
            inputs: ["Image", "Text", "Mask"],
            output: "ImageArtifact",
            skills: [
                "image.prompt.optimize.v1",
                "image.reference.replicate.v1",
                "image.render.enhance.v1",
            ],
        },
        contract! {
            node: "script-gen",
            executor: Executor::Responses,
            operations: [TextGenerate],
            inputs: ["Text", "Json"],
            output: "ScriptText",
            skills: [],
        },
        contract! {
            node: "video-gen",
            executor: Executor::MediaJob,
            operations: [VideoGenerate, VideoEdit, VideoExtend],
            inputs: ["Text", "Image", "Video", "Audio"],
            output: "VideoArtifact",
            skills: [],
        },
        contract! {
            node: "video-stitch",
            executor: Executor::Local,
            operations: [VideoCompose],
            inputs: ["VideoList"],
            output: "VideoArtifact",
            skills: [],
        },
        contract! {
            node: "camera",
            executor: Executor::ContractOnly,
            operations: [CameraConstraint],
            inputs: ["Json", "Empty"],
            output: "CameraSettings(Json)",
            skills: [],
        },
        contract! {
            node: "lens",
            executor: Executor::ContractOnly,
            operations: [LensConstraint],
            inputs: ["Json", "Empty"],
            output: "LensSettings(Json)",
            skills: [],
        },
        contract! {
            node: "focal-length",
            executor: Executor::ContractOnly,
            operations: [FocalLengthConstraint],
            inputs: ["Json", "Empty"],
            output: "FocalLengthConstraints(Json)",
            skills: [],
        },
        contract! {
            node: "aperture",
            executor: Executor::ContractOnly,
            operations: [ApertureConstraint],
            inputs: ["Json", "Empty"],
            output: "ApertureConstraints(Json)",
            skills: [],
        },
        contract! {
            node: "director",
            executor: Executor::Responses,
            operations: [DirectorPlan],
            inputs: ["Text", "Json", "Image", "Video"],
            output: "DirectorPlan(Json)",
            skills: [],
        },
        contract! {
            node: "creative-template",
            executor: Executor::Composite,
            operations: [TemplateExpand],
            inputs: ["TemplateRef", "Text", "Json", "Image"],
            output: "PromptText",
            skills: [],
        },
        contract! {
            node: "character-face",
            executor: Executor::Composite,
            operations: [CharacterFaceCompose],
            inputs: ["Text", "Image", "Json"],
            output: "CharacterFaceProfile(Json)",
            skills: ["image.reference.replicate.v1"],
        },
        contract! {
            node: "character-body",
            executor: Executor::Composite,
            operations: [CharacterBodyCompose],
            inputs: ["Text", "Image", "Json"],
            output: "CharacterBodyProfile(Json)",
            skills: [],
        },
        contract! {
            node: "character-style",
            executor: Executor::Composite,
            operations: [CharacterStyleCompose],
            inputs: ["Text", "Image", "Json"],
            output: "CharacterStyleProfile(Json)",
            skills: ["image.style.extract.v1"],
        },
        contract! {
            node: "audio",
            executor: Executor::CanvasMedia,
            operations: [SpeechTranscribe, SpeechSynthesize],
            inputs: ["Audio", "Text"],
            output: "TextOrAudioArtifact",
            skills: [],
        },
        contract! {
            node: "storyboard-grid",
            executor: Executor::Responses,
            operations: [StoryboardPlan],
            inputs: ["Text", "Json", "Image"],
            output: "Storyboard(Json)",
            skills: [],
        },
    ]
}

/// The contract for one node kind; unknown kinds are `UnsupportedNode`.
pub fn canvas_node_contract(node_kind: &str) -> Result<CanvasNodeContract> {
    let node_kind = node_kind.trim();
    canvas_node_contracts()
        .into_iter()
        .find(|contract| contract.node_kind == node_kind)
        .ok_or_else(|| {
            CoreError::not_found(format!(
                "UnsupportedNode: canvas node `{node_kind}` is not registered"
            ))
        })
}

/// The whole canvas prompt/contract catalog for the settings UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPromptCatalogDto {
    pub contracts: Vec<CanvasNodeContract>,
    pub profiles: Vec<deepagent_prompts::canvas_prompt::PromptProfile>,
    pub skills: Vec<deepagent_prompts::canvas_prompt::CanvasSkill>,
}

/// Read the catalog after validating both registries, so a drifted resource
/// fails the read instead of serving a half-consistent catalog.
pub fn canvas_prompt_catalog() -> Result<CanvasPromptCatalogDto> {
    deepagent_prompts::canvas_prompt::validate_canvas_prompts()?;
    validate_canvas_node_contracts()?;
    Ok(CanvasPromptCatalogDto {
        contracts: canvas_node_contracts(),
        profiles: creative_profiles().to_vec(),
        skills: canvas_skills().to_vec(),
    })
}

/// Guard the cross-registry invariants the plan pins: node kind ↔ prompt profile
/// ↔ profile id, skill allowlist agreement, and graph-registry visibility.
pub fn validate_canvas_node_contracts() -> Result<()> {
    let contracts = canvas_node_contracts();
    let mut seen = std::collections::BTreeSet::new();
    for contract in contracts {
        if !seen.insert(contract.node_kind.clone()) {
            return Err(CoreError::invalid(format!(
                "duplicate canvas node contract `{}`",
                contract.node_kind
            )));
        }
        let profile = creative_profile(&contract.node_kind)?;
        if profile.id != contract.prompt_profile_id {
            return Err(CoreError::invalid(format!(
                "node `{}` contract profile `{}` does not match profile id `{}`",
                contract.node_kind, contract.prompt_profile_id, profile.id
            )));
        }
        if profile.default_skill_ids != contract.allowed_skill_ids {
            return Err(CoreError::invalid(format!(
                "node `{}` skill allowlist differs from its prompt profile",
                contract.node_kind
            )));
        }
        if contract.allowed_operations.is_empty()
            && !matches!(
                contract.executor_kind,
                Executor::Unsupported | Executor::ContractOnly
            )
        {
            return Err(CoreError::invalid(format!(
                "node `{}` needs at least one allowed operation",
                contract.node_kind
            )));
        }
        let node = WorkflowNodeSpec {
            id: format!("contract-check-{}", contract.node_kind),
            kind: contract.node_kind.clone(),
            config: serde_json::Map::new(),
        };
        compile(WorkflowDefinition {
            version: 1,
            nodes: vec![node],
            edges: Vec::new(),
        })
        .map(|_| ())
        .map_err(|error| {
            CoreError::invalid(format!(
                "node `{}` is not registered in the workflow graph registry: {error}",
                contract.node_kind
            ))
        })?;
    }
    let expected = deepagent_prompts::canvas_prompt::CREATIVE_NODE_KINDS.len();
    if seen.len() != expected {
        return Err(CoreError::invalid(format!(
            "canvas node contracts expect {expected} entries, found {}",
            seen.len()
        )));
    }
    Ok(())
}

/// The system prompt plus the user-facing prompt for a node, resolved by the
/// backend so nodes only ever store an id and an optional override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasPromptResolution {
    pub node_kind: String,
    pub prompt_profile_id: String,
    pub prompt_profile_version: u32,
    pub system_prompt: String,
    pub resolved_user_prompt: String,
    pub is_default_user_prompt: bool,
    pub executor_kind: CanvasExecutorKind,
    pub allowed_operations: Vec<CanvasOperation>,
    pub allowed_skill_ids: Vec<String>,
}

/// Resolve the prompts a node should show and send.
///
/// `user_prompt_mode` is `default` (show the profile text), `custom` (keep the
/// node's own text) or `disabled` (send nothing). Node parameter changes must
/// never overwrite a custom prompt, so only `default` re-renders.
pub fn resolve_canvas_prompt(
    node_kind: &str,
    user_prompt_mode: Option<&str>,
    user_prompt_override: Option<&str>,
) -> Result<CanvasPromptResolution> {
    let contract = canvas_node_contract(node_kind)?;
    let profile = creative_profile(&contract.node_kind)?;
    let mode = user_prompt_mode.unwrap_or("default");
    let (resolved_user_prompt, is_default) = match mode {
        "custom" => (
            user_prompt_override
                .unwrap_or(&profile.default_user_prompt)
                .to_string(),
            user_prompt_override.is_none(),
        ),
        "disabled" => (String::new(), false),
        _ => (profile.default_user_prompt.clone(), true),
    };
    Ok(CanvasPromptResolution {
        node_kind: contract.node_kind,
        prompt_profile_id: contract.prompt_profile_id,
        prompt_profile_version: profile.version,
        system_prompt: profile.system_prompt.clone(),
        resolved_user_prompt,
        is_default_user_prompt: is_default,
        executor_kind: contract.executor_kind,
        allowed_operations: contract.allowed_operations,
        allowed_skill_ids: contract.allowed_skill_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_prompts::canvas_prompt::CREATIVE_NODE_KINDS;

    #[test]
    fn contracts_profiles_and_graph_registry_agree() {
        validate_canvas_node_contracts().expect("canvas contracts consistent");
    }

    #[test]
    fn every_creative_node_kind_has_exactly_one_contract() {
        let contracts = canvas_node_contracts();
        assert_eq!(contracts.len(), CREATIVE_NODE_KINDS.len());
        for kind in CREATIVE_NODE_KINDS {
            assert!(
                contracts.iter().filter(|c| c.node_kind == kind).count() == 1,
                "{kind} must appear once"
            );
        }
    }

    #[test]
    fn unregistered_node_is_rejected_not_passed_through() {
        let error = canvas_node_contract("mystery").expect_err("unknown node");
        assert!(error.to_string().contains("UnsupportedNode"));
    }

    #[test]
    fn prompt_resolution_defaults_and_respects_custom_mode() {
        let resolved = resolve_canvas_prompt("text-gen", None, None).expect("default");
        assert!(resolved.is_default_user_prompt);
        assert_eq!(resolved.prompt_profile_id, "creative.text-gen.v1");
        assert!(!resolved.system_prompt.trim().is_empty());

        let edited = resolve_canvas_prompt("text-gen", Some("custom"), Some("我自己的提示词"))
            .expect("custom");
        assert!(!edited.is_default_user_prompt);
        assert_eq!(edited.resolved_user_prompt, "我自己的提示词");

        let disabled = resolve_canvas_prompt("text-gen", Some("disabled"), None).expect("disabled");
        assert!(disabled.resolved_user_prompt.is_empty());
        assert!(!disabled.is_default_user_prompt);
    }

    #[test]
    fn image_generation_and_edit_keep_separate_contracts() {
        let gen = canvas_node_contract("image-gen").expect("gen");
        let edit = canvas_node_contract("image-edit").expect("edit");
        assert_eq!(
            gen.allowed_operations,
            vec![CanvasOperation::ImageGenerate, CanvasOperation::ImageEdit]
        );
        assert_eq!(gen.prompt_profile_id, "creative.image-gen.v1");
        assert_eq!(edit.prompt_profile_id, "creative.image-edit.v1");
        assert!(!edit
            .allowed_operations
            .contains(&CanvasOperation::ImageGenerate));
        assert_eq!(edit.executor_kind, CanvasExecutorKind::Composite);
        assert_eq!(gen.executor_kind, CanvasExecutorKind::CanvasMedia);
    }

    #[test]
    fn audio_requires_an_explicit_operation_and_video_is_media_job() {
        let audio = canvas_node_contract("audio").expect("audio");
        assert_eq!(
            audio.allowed_operations,
            vec![
                CanvasOperation::SpeechTranscribe,
                CanvasOperation::SpeechSynthesize
            ]
        );
        let video = canvas_node_contract("video-gen").expect("video");
        assert_eq!(video.executor_kind, CanvasExecutorKind::MediaJob);
        assert!(video
            .allowed_operations
            .contains(&CanvasOperation::VideoExtend));
    }
}
