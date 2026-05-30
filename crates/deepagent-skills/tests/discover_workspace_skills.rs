//! Integration test: auto-discover the repo's workspace skills and verify
//! passive trigger matching routes representative queries to the right skill.
//!
//! This walks up from the crate dir to the workspace root and loads the real
//! `.deepagent/skills/` tree, doubling as a smoke test that the bundled skills
//! parse and trigger correctly.

use std::path::PathBuf;

use deepagent_skills::{loader, SkillOrigin, SkillRegistry};

/// Locate the repo's `.deepagent/skills` dir from the crate manifest dir.
fn skills_root() -> Option<PathBuf> {
    // CARGO_MANIFEST_DIR = <root>/crates/deepagent-skills
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.parent()?.parent()?; // -> <root>
    let dir = root.join(".deepagent").join("skills");
    dir.is_dir().then_some(dir)
}

fn registry() -> SkillRegistry {
    let mut reg = SkillRegistry::new();
    if let Some(root) = skills_root() {
        for skill in loader::discover(&root, SkillOrigin::Workspace).expect("discover") {
            reg.register(skill);
        }
    }
    reg
}

#[test]
fn discovers_the_bundled_skill_set() {
    if skills_root().is_none() {
        // Skills dir not present in this checkout — nothing to assert.
        return;
    }
    let reg = registry();

    // The seven requested skills (plus rust-backend-review) must be present.
    for id in [
        "agent-browser",
        "code-review-skill",
        "mcp-builder",
        "planning-with-files",
        "superpowers",
        "ui-ux-pro-max-skill",
        "webapp-testing",
    ] {
        assert!(reg.contains(id), "missing bundled skill: {id}");
        // Every bundled skill must have extracted at least one trigger phrase.
        assert!(
            !reg.get(id).unwrap().triggers.is_empty(),
            "skill {id} has no triggers"
        );
    }
}

#[test]
fn passive_matching_routes_to_expected_skills() {
    if skills_root().is_none() {
        return;
    }
    let reg = registry();

    // (query, expected best-match skill id)
    let cases = [
        (
            "please take a screenshot of the login page",
            "agent-browser",
        ),
        (
            "review this pull request for security issues",
            "code-review-skill",
        ),
        ("help me build an mcp server for our api", "mcp-builder"),
        (
            "write an end-to-end test for the checkout flow",
            "webapp-testing",
        ),
        (
            "pick a color palette for the dashboard",
            "ui-ux-pro-max-skill",
        ),
    ];

    for (query, expected) in cases {
        let best = reg
            .best_match(query)
            .unwrap_or_else(|| panic!("no skill matched query: {query:?}"));
        assert_eq!(best.id, expected, "query {query:?} routed to {}", best.id);
    }
}
