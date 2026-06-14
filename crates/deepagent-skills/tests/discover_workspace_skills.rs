//! Integration test: auto-discover the repo's workspace skills tree and
//! verify the bundled skill set is reachable via the recursive loader.
//!
//! This walks up from the crate dir to the workspace root and loads the real
//! `.deepagent/skills/` tree, doubling as a smoke test that the bundled skills
//! parse correctly.
//!
//! The repo's bundled-skill layout uses three different shapes:
//! - **Double-shell** (`<id>/<id>/SKILL.md`) for `agent-browser`,
//!   `code-review-skill`, `mcp-builder`, `planning-with-files`,
//!   `webapp-testing` — discovered at depth 2.
//! - **Bundled skill collection** (`superpowers/SKILL.md` +
//!   `superpowers/skills/<sub>/SKILL.md`) — the parent `SKILL.md` shadows
//!   the 14 nested sub-skills under the loader's "parent wins over child"
//!   rule, so only the parent `superpowers` entry is registered. The model
//!   reaches the sub-skill instructions on demand by reading
//!   `<base_dir>/skills/<sub>/SKILL.md`.
//! - **Deep claude-plugin layout** (`ui-ux-pro-max-skill/.claude/skills/<sub>/`)
//!   — beyond `max_depth = 3`, normalized into a stub root SKILL.md by the
//!   prebundle script (task 9). This test runs against the raw repo source so
//!   `ui-ux-pro-max-skill` is intentionally NOT asserted here; the prebundled
//!   `apps/desktop/src-tauri/resources/skills/` layout (covered by other
//!   tests + the runtime wiring) is where the stub appears.

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
        // Use depth=3 to reach `superpowers/skills/<sub>/SKILL.md` and the
        // double-shell layouts (`<id>/<id>/SKILL.md`).
        for skill in loader::discover_recursive(&root, SkillOrigin::Workspace, 3).expect("discover")
        {
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

    // Top-level double-shell skills (`<id>/<id>/SKILL.md`, depth 2).
    for id in [
        "agent-browser",
        "code-review-skill",
        "mcp-builder",
        "planning-with-files",
        "webapp-testing",
    ] {
        assert!(reg.contains(id), "missing bundled skill: {id}");
    }

    // Superpowers ships as a single bundle entry (depth 1) — its parent
    // `SKILL.md` shadows every `superpowers/skills/<sub>/SKILL.md` under the
    // loader's "parent wins over child" rule, so the catalog must show
    // exactly one row.
    assert!(
        reg.contains("superpowers"),
        "missing parent bundle skill: superpowers"
    );
    for shadowed in [
        "systematic-debugging",
        "test-driven-development",
        "brainstorming",
        "writing-plans",
        "using-superpowers",
    ] {
        assert!(
            !reg.contains(shadowed),
            "superpowers sub-skill `{shadowed}` should be shadowed by the parent SKILL.md, but the registry still surfaces it"
        );
    }
}

/// Passive trigger matching is highly dependent on each skill's frontmatter
/// having quoted phrases (`extract_triggers` only lifts text inside `"…"` or
/// `'…'`). Most of the currently bundled skills express their `whenToUse`
/// guidance as plain prose without quoted spans, so `best_match` returns
/// `None` for natural-language queries.
///
/// Skill activation in this app now flows through the catalog reminder
/// (channel A) + `SkillTool` (channel B) instead of `match_query`, so this
/// test is left ignored until the bundled skills are normalized to include
/// quoted trigger phrases (or a richer matching model lands). Tracked by
/// task 25 of the skill-marketplace spec.
#[test]
#[ignore = "trigger-phrase passive matching has been superseded by SkillTool channel B; bundled skills lack quoted triggers"]
fn passive_matching_routes_to_expected_skills() {
    if skills_root().is_none() {
        return;
    }
    let reg = registry();

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
    ];

    for (query, expected) in cases {
        let best = reg
            .best_match(query)
            .unwrap_or_else(|| panic!("no skill matched query: {query:?}"));
        assert_eq!(best.id, expected, "query {query:?} routed to {}", best.id);
    }
}
