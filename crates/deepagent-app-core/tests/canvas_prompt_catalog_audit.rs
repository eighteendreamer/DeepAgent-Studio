//! Offline audit of bundled prompt validation at the public catalog boundary.
//! No AppCore instance, provider, user configuration or persisted state is opened.

use deepagent_app_core::canvas_node_contract::{
    canvas_node_contracts, canvas_prompt_catalog, resolve_canvas_prompt,
    validate_canvas_node_contracts,
};
use deepagent_prompts::canvas_prompt::{creative_profiles, CREATIVE_NODE_KINDS};

#[test]
fn individual_canvas_contracts_and_prompt_resolution_remain_available() {
    validate_canvas_node_contracts().expect("node contracts remain consistent");
    assert_eq!(creative_profiles().len(), CREATIVE_NODE_KINDS.len());
    assert_eq!(canvas_node_contracts().len(), CREATIVE_NODE_KINDS.len());
    for kind in CREATIVE_NODE_KINDS {
        let resolved = resolve_canvas_prompt(kind, None, None).expect("resolve individual node");
        assert_eq!(resolved.node_kind, *kind);
        assert!(!resolved.system_prompt.is_empty());
    }
    let local_edit = resolve_canvas_prompt("video-edit", None, None).expect("local editor");
    assert!(local_edit.resolved_user_prompt.is_empty());
    let text = resolve_canvas_prompt("text-gen", None, None).expect("text generator");
    assert!(!text.resolved_user_prompt.is_empty());
    eprintln!(
        "catalog control: individual_nodes={} local_editor_prompt_empty=true text_prompt_present=true",
        CREATIVE_NODE_KINDS.len()
    );
}

#[test]
fn audit_bundled_local_editor_does_not_block_public_prompt_catalog() {
    let result = canvas_prompt_catalog();
    eprintln!(
        "catalog audit: bundled_profiles={} catalog_ok={} error={:?}",
        creative_profiles().len(),
        result.is_ok(),
        result.as_ref().err().map(ToString::to_string)
    );
    assert!(
        result.is_ok(),
        "the public catalog must remain readable for the shipped profiles: {result:?}"
    );
    let catalog = result.unwrap();
    assert_eq!(catalog.profiles.len(), CREATIVE_NODE_KINDS.len());
    assert_eq!(catalog.contracts.len(), CREATIVE_NODE_KINDS.len());
    assert_eq!(
        catalog.skills.len(),
        deepagent_prompts::canvas_prompt::canvas_skills().len()
    );
    for kind in CREATIVE_NODE_KINDS {
        assert_eq!(
            catalog
                .profiles
                .iter()
                .filter(|p| p.node_kind == kind)
                .count(),
            1
        );
        assert_eq!(
            catalog
                .contracts
                .iter()
                .filter(|c| c.node_kind == kind)
                .count(),
            1
        );
        let profile = catalog
            .profiles
            .iter()
            .find(|p| p.node_kind == kind)
            .unwrap();
        let resolved = resolve_canvas_prompt(kind, None, None).unwrap();
        assert_eq!(resolved.prompt_profile_id, profile.id);
        assert_eq!(resolved.prompt_profile_version, profile.version);
        assert_eq!(resolved.resolved_user_prompt, profile.default_user_prompt);
        assert_eq!(resolved.system_prompt, profile.system_prompt);
    }
    let wire = serde_json::to_value(&catalog).unwrap();
    let restored = serde_json::from_value(wire).unwrap();
    assert_eq!(catalog, restored);
    let local = catalog
        .profiles
        .iter()
        .find(|p| p.node_kind == "video-edit")
        .unwrap();
    assert!(
        local.default_user_prompt.is_empty(),
        "do not invent model input for local editing"
    );
    assert_eq!(local.prompt_usage, "local_editor");
    let text = resolve_canvas_prompt("text-gen", Some("custom"), Some("保留用户内容")).unwrap();
    assert_eq!(text.resolved_user_prompt, "保留用户内容");
}
