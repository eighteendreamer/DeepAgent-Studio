//! Claude Code's conventional component locations.
//!
//! Claude Code auto-discovers components from fixed directories at the plugin
//! root. The authoritative description ships inside the `plugin-dev` plugin
//! (`plugins/plugin-dev/skills/plugin-structure/SKILL.md` in
//! `anthropics/claude-code`):
//!
//! ```text
//! plugin-name/
//! ├── .claude-plugin/plugin.json   # manifest
//! ├── commands/                    # slash commands (.md, subdirectories namespace)
//! ├── agents/                      # subagents (.md)
//! ├── skills/<name>/SKILL.md       # agent skills
//! ├── hooks/hooks.json             # event handlers
//! └── .mcp.json                    # MCP servers
//! ```
//!
//! # Supplement, not fallback
//!
//! The same document states the rule that shapes this module:
//!
//! > **Important**: Custom paths supplement defaults—they don't replace them.
//! > Components in both default directories and custom paths will load.
//!
//! So the conventional locations are always scanned; a manifest's `commands`,
//! `agents`, or `hooks` field *adds* locations. This differs from the shipping
//! [`crate::plugin_manifest`] behavior, where a declared path replaces the
//! default (`component_paths` returns early when the field is present). For a
//! Claude-authored plugin that declares one extra directory, replace-semantics
//! silently drops everything in the conventional one. [`supplement`] is the
//! union operation that fixes it.
//!
//! # `mcp.json` versus `.mcp.json`
//!
//! These are different files. `mcp.json` is the v1 fixed location (§7.2);
//! `.mcp.json` is the Claude convention. Both are recognized, `mcp.json` wins
//! when both exist, and the loser is reported rather than dropped silently.
//! Which one was chosen also decides the parsing contract, so
//! [`McpConventionSource`] is carried alongside the path: §7.2 forbids
//! placeholders in `command`, while a Claude plugin legitimately writes
//! `${CLAUDE_PLUGIN_ROOT}/bin/server` there.
//!
//! # Case sensitivity
//!
//! `SKILL.md` is matched exactly because §7.1 says "named exactly". These
//! dialect directories have no such requirement, and Claude Code's own
//! existence checks inherit the host filesystem's case rules — so a plugin with
//! `Commands/` does work under Claude Code on Windows and macOS. Rejecting it
//! here would break a plugin that works upstream, so the entry is accepted and
//! the portability hazard is reported instead: that plugin will not load on a
//! case-sensitive filesystem.

use std::path::{Path, PathBuf};

use crate::plugin::model::{ComponentKind, PluginDiagnostic};
use crate::plugin::spec::path::resolve_existing_within;
use crate::plugin::spec::schema::{
    AGENT_PLUGIN_MCP_RELATIVE_PATH, AGENT_PLUGIN_SKILLS_RELATIVE_PATH,
};

/// Slash commands directory. Subdirectories namespace the commands inside.
pub const CLAUDE_COMMANDS_DIR: &str = "commands";
/// Subagent definitions directory.
pub const CLAUDE_AGENTS_DIR: &str = "agents";
/// Directory holding `hooks.json` and its helper scripts.
pub const CLAUDE_HOOKS_DIR: &str = "hooks";
/// Hook configuration file name inside [`CLAUDE_HOOKS_DIR`].
pub const CLAUDE_HOOKS_FILE: &str = "hooks.json";
/// Claude's MCP configuration file, distinct from v1's `mcp.json`.
pub const CLAUDE_MCP_FILE: &str = ".mcp.json";

/// Which file an MCP configuration was found in.
///
/// The distinction is not cosmetic: it selects the parsing contract. §7.2
/// requires `command` to be a literal single token, whereas a Claude plugin
/// routinely writes `${CLAUDE_PLUGIN_ROOT}/bin/server`. Applying the strict
/// rules to a `.mcp.json` would reject working upstream plugins; applying the
/// lenient rules to `mcp.json` would silently accept a non-conforming package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConventionSource {
    /// `mcp.json` — the v1 fixed location. Parse with strict §7.2 rules.
    Portable,
    /// `.mcp.json` — the Claude convention. Parse with lenient rules that allow
    /// placeholders in `command`.
    Claude,
}

impl McpConventionSource {
    /// The file name this source refers to.
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::Portable => AGENT_PLUGIN_MCP_RELATIVE_PATH,
            Self::Claude => CLAUDE_MCP_FILE,
        }
    }
}

/// An MCP configuration file found at a conventional location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConvention {
    pub path: PathBuf,
    pub source: McpConventionSource,
}

/// The conventional component locations present in a plugin directory.
///
/// Every field is `None` when the location is absent, which §6.2 treats as
/// ordinary rather than exceptional. A location present with the wrong
/// filesystem kind yields `None` plus a diagnostic, leaving the other component
/// types loadable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaudeConventions {
    /// `skills/`
    pub skills: Option<PathBuf>,
    /// `commands/`
    pub commands: Option<PathBuf>,
    /// `agents/`
    pub agents: Option<PathBuf>,
    /// `hooks/hooks.json`
    pub hooks: Option<PathBuf>,
    /// `mcp.json` or `.mcp.json`
    pub mcp: Option<McpConvention>,
    /// Findings worth surfacing even though discovery succeeded.
    pub diagnostics: Vec<PluginDiagnostic>,
}

/// Scans `plugin_root` for the conventional component locations.
///
/// Performs no manifest parsing: the result is unioned with whatever the
/// manifest declares, because Claude Code's custom paths supplement the
/// conventional ones rather than replacing them.
pub fn discover_conventions(plugin_root: &Path) -> ClaudeConventions {
    let mut diagnostics = Vec::new();

    let skills = directory(
        plugin_root,
        AGENT_PLUGIN_SKILLS_RELATIVE_PATH,
        ComponentKind::Skills,
        &mut diagnostics,
    );
    let commands = directory(
        plugin_root,
        CLAUDE_COMMANDS_DIR,
        ComponentKind::Commands,
        &mut diagnostics,
    );
    let agents = directory(
        plugin_root,
        CLAUDE_AGENTS_DIR,
        ComponentKind::Agents,
        &mut diagnostics,
    );
    let hooks = hooks_file(plugin_root, &mut diagnostics);
    let mcp = mcp_file(plugin_root, &mut diagnostics);

    ClaudeConventions {
        skills,
        commands,
        agents,
        hooks,
        mcp,
        diagnostics,
    }
}

/// Adds `convention` to `declared` unless it is already there.
///
/// This is the union that Claude Code's supplement rule requires. Order is
/// preserved so a manifest-declared location keeps its precedence for anything
/// downstream that resolves conflicts by first-wins.
pub fn supplement(declared: &mut Vec<PathBuf>, convention: Option<&Path>) {
    let Some(convention) = convention else {
        return;
    };
    if declared.iter().any(|existing| existing == convention) {
        return;
    }
    declared.push(convention.to_path_buf());
}

/// Resolves a conventional directory, reporting the wrong filesystem kind and a
/// case-only name mismatch.
fn directory(
    plugin_root: &Path,
    name: &str,
    component: ComponentKind,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<PathBuf> {
    let path = plugin_root.join(name);

    // §6.2: an absent conventional location is not an error.
    if !path.exists() {
        return None;
    }

    if !path.is_dir() {
        diagnostics.push(PluginDiagnostic::ComponentInvalid {
            component,
            path: Some(path),
            reason: format!("`{name}` is not a directory"),
        });
        return None;
    }

    report_case_mismatch(plugin_root, name, component, diagnostics);
    contained(plugin_root, path, component, diagnostics)
}

/// Resolves `hooks/hooks.json`.
///
/// A `hooks/` directory without `hooks.json` is silent: the scripts may be
/// referenced from a manifest-declared configuration, or the hooks may be
/// declared inline.
fn hooks_file(plugin_root: &Path, diagnostics: &mut Vec<PluginDiagnostic>) -> Option<PathBuf> {
    let dir = plugin_root.join(CLAUDE_HOOKS_DIR);
    if !dir.is_dir() {
        // A `hooks` entry that is a file is not the Claude layout, and the
        // manifest may legitimately point `hooks` at a file of another name, so
        // there is nothing to report here.
        return None;
    }
    report_case_mismatch(
        plugin_root,
        CLAUDE_HOOKS_DIR,
        ComponentKind::Hooks,
        diagnostics,
    );

    let path = dir.join(CLAUDE_HOOKS_FILE);
    if !path.exists() {
        return None;
    }
    if !path.is_file() {
        diagnostics.push(PluginDiagnostic::ComponentInvalid {
            component: ComponentKind::Hooks,
            path: Some(path),
            reason: format!("`{CLAUDE_HOOKS_DIR}/{CLAUDE_HOOKS_FILE}` is not a regular file"),
        });
        return None;
    }
    report_case_mismatch(&dir, CLAUDE_HOOKS_FILE, ComponentKind::Hooks, diagnostics);
    contained(plugin_root, path, ComponentKind::Hooks, diagnostics)
}

/// Resolves the MCP configuration, preferring v1's `mcp.json` over Claude's
/// `.mcp.json` and reporting the one that lost.
fn mcp_file(plugin_root: &Path, diagnostics: &mut Vec<PluginDiagnostic>) -> Option<McpConvention> {
    let portable = regular_file(plugin_root, AGENT_PLUGIN_MCP_RELATIVE_PATH, diagnostics);
    let claude = regular_file(plugin_root, CLAUDE_MCP_FILE, diagnostics);

    match (portable, claude) {
        (Some(path), Some(_)) => {
            diagnostics.push(PluginDiagnostic::McpDisabled {
                reason: format!(
                    "both `{}` and `{}` are present; using the Agent Plugins fixed location \
                     `{}` and ignoring `{}`",
                    AGENT_PLUGIN_MCP_RELATIVE_PATH,
                    CLAUDE_MCP_FILE,
                    AGENT_PLUGIN_MCP_RELATIVE_PATH,
                    CLAUDE_MCP_FILE
                ),
            });
            mcp_convention(
                plugin_root,
                path,
                McpConventionSource::Portable,
                diagnostics,
            )
        }
        (Some(path), None) => mcp_convention(
            plugin_root,
            path,
            McpConventionSource::Portable,
            diagnostics,
        ),
        (None, Some(path)) => {
            mcp_convention(plugin_root, path, McpConventionSource::Claude, diagnostics)
        }
        (None, None) => None,
    }
}

fn mcp_convention(
    plugin_root: &Path,
    path: PathBuf,
    source: McpConventionSource,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<McpConvention> {
    contained(plugin_root, path, ComponentKind::Mcp, diagnostics)
        .map(|path| McpConvention { path, source })
}

/// Resolves a conventional file, reporting the wrong filesystem kind.
fn regular_file(
    plugin_root: &Path,
    name: &str,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<PathBuf> {
    let path = plugin_root.join(name);
    if !path.exists() {
        return None;
    }
    if !path.is_file() {
        diagnostics.push(PluginDiagnostic::ComponentInvalid {
            component: ComponentKind::Mcp,
            path: Some(path),
            reason: format!("`{name}` is not a regular file"),
        });
        return None;
    }
    Some(path)
}

/// §4.1: a package path resolving outside the plugin root must be rejected,
/// which cannot be decided lexically when symlinks are involved.
fn contained(
    plugin_root: &Path,
    path: PathBuf,
    component: ComponentKind,
    diagnostics: &mut Vec<PluginDiagnostic>,
) -> Option<PathBuf> {
    match resolve_existing_within(plugin_root, &path) {
        Ok(_) => Some(path),
        Err(error) => {
            diagnostics.push(PluginDiagnostic::ComponentInvalid {
                component,
                path: Some(path),
                reason: format!("resolves outside the plugin root: {error}"),
            });
            None
        }
    }
}

/// Reports a conventional entry whose on-disk name differs from `expected` only
/// by case.
///
/// It loads here and under Claude Code on this host, but it will not load on a
/// case-sensitive filesystem — a silent difference worth naming.
fn report_case_mismatch(
    parent: &Path,
    expected: &str,
    component: ComponentKind,
    diagnostics: &mut Vec<PluginDiagnostic>,
) {
    let Some(actual) = case_variant_name(parent, expected) else {
        return;
    };
    diagnostics.push(PluginDiagnostic::ComponentInvalid {
        component,
        path: Some(parent.join(&actual)),
        reason: format!(
            "found `{actual}` where the convention is `{expected}`; this loads on a \
             case-insensitive filesystem but not on a case-sensitive one"
        ),
    });
}

/// The on-disk name when it matches `expected` case-insensitively but not
/// exactly, or `None` when an exact match exists or nothing matches.
fn case_variant_name(parent: &Path, expected: &str) -> Option<String> {
    let entries = std::fs::read_dir(parent).ok()?;
    let mut variant = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == expected {
            return None;
        }
        if name.eq_ignore_ascii_case(expected) {
            variant = Some(name.to_string());
        }
    }
    variant
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, "x").expect("write");
    }

    fn mkdir(path: &Path) {
        std::fs::create_dir_all(path).expect("create dir");
    }

    /// The full layout from the upstream `plugin-structure` skill.
    #[test]
    fn discovers_the_documented_claude_layout() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("commands").join("review.md"));
        touch(&root.join("agents").join("reviewer.md"));
        touch(&root.join("skills").join("api-testing").join("SKILL.md"));
        touch(&root.join("hooks").join("hooks.json"));
        touch(&root.join(".mcp.json"));

        let found = discover_conventions(root);

        assert_eq!(found.commands, Some(root.join("commands")));
        assert_eq!(found.agents, Some(root.join("agents")));
        assert_eq!(found.skills, Some(root.join("skills")));
        assert_eq!(found.hooks, Some(root.join("hooks").join("hooks.json")));
        assert_eq!(
            found.mcp,
            Some(McpConvention {
                path: root.join(".mcp.json"),
                source: McpConventionSource::Claude,
            })
        );
        assert!(found.diagnostics.is_empty(), "{:?}", found.diagnostics);
    }

    /// §6.2: absent conventional locations are ordinary, not errors.
    #[test]
    fn a_plugin_with_no_conventional_locations_reports_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        touch(&tmp.path().join(".claude-plugin").join("plugin.json"));

        let found = discover_conventions(tmp.path());

        assert_eq!(found, ClaudeConventions::default());
    }

    /// Only the offending component type is invalidated; the rest keep loading.
    #[test]
    fn a_conventional_directory_that_is_a_file_invalidates_only_itself() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("commands"));
        touch(&root.join("agents").join("reviewer.md"));

        let found = discover_conventions(root);

        assert!(found.commands.is_none());
        assert_eq!(found.agents, Some(root.join("agents")));
        assert_eq!(found.diagnostics.len(), 1);
        assert!(matches!(
            &found.diagnostics[0],
            PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Commands,
                reason,
                ..
            } if reason.contains("not a directory")
        ));
    }

    #[test]
    fn a_hooks_directory_without_hooks_json_is_silent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("hooks").join("validate.sh"));

        let found = discover_conventions(root);

        assert!(found.hooks.is_none());
        assert!(found.diagnostics.is_empty(), "{:?}", found.diagnostics);
    }

    #[test]
    fn hooks_json_that_is_a_directory_is_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        mkdir(&root.join("hooks").join("hooks.json"));

        let found = discover_conventions(root);

        assert!(found.hooks.is_none());
        assert!(matches!(
            &found.diagnostics[0],
            PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Hooks,
                reason,
                ..
            } if reason.contains("not a regular file")
        ));
    }

    /// `mcp.json` and `.mcp.json` are different files. v1's fixed location wins,
    /// and the ignored one is named rather than dropped silently.
    #[test]
    fn portable_mcp_json_wins_over_claude_dot_mcp_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("mcp.json"));
        touch(&root.join(".mcp.json"));

        let found = discover_conventions(root);

        let mcp = found.mcp.expect("an mcp configuration");
        assert_eq!(mcp.path, root.join("mcp.json"));
        assert_eq!(mcp.source, McpConventionSource::Portable);
        assert!(matches!(
            &found.diagnostics[0],
            PluginDiagnostic::McpDisabled { reason }
                if reason.contains(".mcp.json") && reason.contains("mcp.json")
        ));
    }

    /// The source decides the parsing contract, so it must not be guessed from
    /// the presence of the other file.
    #[test]
    fn mcp_source_reflects_the_file_that_was_found() {
        for (name, expected) in [
            ("mcp.json", McpConventionSource::Portable),
            (".mcp.json", McpConventionSource::Claude),
        ] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path();
            touch(&root.join(name));

            let mcp = discover_conventions(root).mcp.expect("an mcp config");

            assert_eq!(mcp.source, expected, "for {name}");
            assert_eq!(mcp.source.file_name(), name);
        }
    }

    #[test]
    fn an_mcp_path_that_is_a_directory_is_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        mkdir(&root.join(".mcp.json"));

        let found = discover_conventions(root);

        assert!(found.mcp.is_none());
        assert!(matches!(
            &found.diagnostics[0],
            PluginDiagnostic::ComponentInvalid {
                component: ComponentKind::Mcp,
                reason,
                ..
            } if reason.contains("not a regular file")
        ));
    }

    /// Upstream's rule: custom paths supplement the defaults. A declared
    /// directory must not hide the conventional one.
    #[test]
    fn supplement_unions_declared_and_conventional_locations() {
        let root = Path::new(if cfg!(windows) {
            r"C:\plugins\demo"
        } else {
            "/plugins/demo"
        });
        let mut declared = vec![root.join("custom-commands")];

        supplement(&mut declared, Some(&root.join("commands")));

        assert_eq!(
            declared,
            vec![root.join("custom-commands"), root.join("commands")],
            "the declared location keeps its precedence and the convention is added"
        );
    }

    #[test]
    fn supplement_is_idempotent_and_tolerates_an_absent_convention() {
        let root = Path::new(if cfg!(windows) {
            r"C:\plugins\demo"
        } else {
            "/plugins/demo"
        });
        let mut declared = vec![root.join("commands")];

        supplement(&mut declared, Some(&root.join("commands")));
        supplement(&mut declared, None);

        assert_eq!(declared, vec![root.join("commands")]);
    }

    /// A case-only mismatch loads on this host but not on a case-sensitive one.
    /// The check only fires where the filesystem allowed the lookup to succeed.
    #[test]
    fn a_case_only_directory_mismatch_is_reported_without_dropping_the_component() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("Commands").join("review.md"));

        let found = discover_conventions(root);

        if found.commands.is_none() {
            // Case-sensitive filesystem: `commands` genuinely does not exist,
            // which is the outcome the diagnostic warns about.
            assert!(found.diagnostics.is_empty(), "{:?}", found.diagnostics);
            return;
        }

        assert_eq!(found.commands, Some(root.join("commands")));
        assert!(
            found.diagnostics.iter().any(|diagnostic| matches!(
                diagnostic,
                PluginDiagnostic::ComponentInvalid { component: ComponentKind::Commands, reason, .. }
                    if reason.contains("case-sensitive")
            )),
            "expected a portability diagnostic, got {:?}",
            found.diagnostics
        );
    }

    #[test]
    fn an_exact_directory_name_produces_no_case_diagnostic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        touch(&root.join("commands").join("review.md"));

        assert!(discover_conventions(root).diagnostics.is_empty());
    }

    /// §4.1: a conventional directory symlinked outside the plugin root is
    /// refused. Creating symlinks needs privileges on Windows, so the test skips
    /// when refused.
    #[test]
    fn a_conventional_directory_resolving_outside_the_root_is_refused() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("plugin");
        mkdir(&root);
        let outside = tmp.path().join("elsewhere");
        touch(&outside.join("review.md"));

        if !try_symlink_dir(&outside, &root.join("commands")) {
            eprintln!("skipping: platform does not permit creating directory symlinks");
            return;
        }

        let found = discover_conventions(&root);

        assert!(found.commands.is_none());
        assert!(found.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            PluginDiagnostic::ComponentInvalid { reason, .. }
                if reason.contains("outside the plugin root")
        )));
    }

    // --- Real upstream plugins -------------------------------------------------
    //
    // `anthropics/claude-code` is not open source (© Anthropic PBC, all rights
    // reserved), so its plugins cannot be copied into this repository. They are
    // read from the reference checkout when it is present and skipped otherwise,
    // which keeps CI green on a clean clone.

    fn upstream_plugin(name: &str) -> Option<PathBuf> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../借鉴/claude-code/plugins")
            .join(name);
        path.is_dir().then_some(path)
    }

    /// `hookify` ships every component type: 4 commands, 1 agent, 1 skill, and
    /// `hooks/hooks.json`. Its manifest declares no component paths at all, so
    /// everything it contributes comes from the conventions.
    #[test]
    fn hookify_is_discovered_entirely_from_conventions() {
        let Some(root) = upstream_plugin("hookify") else {
            eprintln!("skipping: 借鉴/claude-code/plugins/hookify is not present");
            return;
        };

        let found = discover_conventions(&root);

        assert_eq!(found.commands, Some(root.join("commands")));
        assert_eq!(found.agents, Some(root.join("agents")));
        assert_eq!(found.skills, Some(root.join("skills")));
        assert_eq!(found.hooks, Some(root.join("hooks").join("hooks.json")));
        assert!(found.mcp.is_none(), "hookify declares no MCP servers");
        assert!(found.diagnostics.is_empty(), "{:?}", found.diagnostics);

        // The conventional `skills/` really is a v1-shaped skill root.
        let (skills, diagnostics) = crate::plugin::component::discover_skills(&root);
        assert_eq!(
            skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            vec!["writing-rules"]
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    /// `pr-review-toolkit` has only commands and agents, so the absent locations
    /// must stay quiet rather than accumulate diagnostics.
    #[test]
    fn pr_review_toolkit_reports_only_the_locations_it_ships() {
        let Some(root) = upstream_plugin("pr-review-toolkit") else {
            eprintln!("skipping: 借鉴/claude-code/plugins/pr-review-toolkit is not present");
            return;
        };

        let found = discover_conventions(&root);

        assert_eq!(found.commands, Some(root.join("commands")));
        assert_eq!(found.agents, Some(root.join("agents")));
        assert!(found.skills.is_none());
        assert!(found.hooks.is_none());
        assert!(found.mcp.is_none());
        assert!(found.diagnostics.is_empty(), "{:?}", found.diagnostics);
    }

    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }
}
