//! Command palette and slash-completion metadata.
//!
//! The command surface merges built-in UI actions, built-in executable slash
//! commands, and Claude Code-style `commands/*.md` prompt commands discovered
//! from the active project/workspace.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::dto::CommandDto;
use crate::plugin_runtime::PluginCommandRoot;

/// The built-in command set.
pub fn builtin_commands() -> Vec<CommandDto> {
    let c = |id: &str,
             title: &str,
             description: &str,
             category: &str,
             shortcut: Option<&str>|
     -> CommandDto {
        CommandDto {
            id: id.to_string(),
            title: title.to_string(),
            description: description.to_string(),
            category: category.to_string(),
            shortcut: shortcut.map(|s| s.to_string()),
        }
    };

    let mut commands = vec![
        c(
            "session.new",
            "New Session",
            "Start a fresh conversation.",
            "Session",
            Some("Ctrl+N"),
        ),
        c(
            "session.end",
            "End Session",
            "Mark the current session as ended.",
            "Session",
            None,
        ),
        c(
            "session.refresh",
            "Refresh Sessions",
            "Reload the session list.",
            "Session",
            Some("Ctrl+R"),
        ),
        c(
            "view.timeline",
            "Show Timeline",
            "Open the event timeline for the current session.",
            "View",
            Some("Ctrl+1"),
        ),
        c(
            "view.metrics",
            "Toggle Metrics Panel",
            "Show or hide session metrics.",
            "View",
            Some("Ctrl+2"),
        ),
        c(
            "view.diff",
            "Open Diff View",
            "Open the text diff tool.",
            "View",
            Some("Ctrl+D"),
        ),
        c(
            "approvals.review",
            "Review Pending Approvals",
            "Open pending tool approval requests.",
            "Approvals",
            Some("Ctrl+Shift+A"),
        ),
        c(
            "mcp.list",
            "List MCP Servers",
            "Open the configured MCP server list.",
            "MCP",
            None,
        ),
        c(
            "theme.toggle",
            "Toggle Theme",
            "Switch between light and dark UI themes.",
            "View",
            None,
        ),
    ];

    let registry = deepagent_intent::SlashRegistry::with_builtins();
    for name in registry.names() {
        if let Some(command) = registry.get(&name) {
            commands.push(c(
                &format!("slash.{name}"),
                &format!("/{name}"),
                &command.description,
                "内置命令",
                None,
            ));
        }
    }

    commands
}

/// Merge built-ins with project/workspace `commands/*.md` definitions.
pub fn commands_from_roots(
    query: &str,
    roots: impl IntoIterator<Item = PathBuf>,
) -> Vec<CommandDto> {
    commands_from_roots_and_plugins(query, roots, &[])
}

/// Merge built-ins, project/workspace commands, and enabled plugin command
/// definitions. Plugin commands are always namespaced as
/// `/plugin-name:command` so they cannot shadow built-in slash commands.
pub fn commands_from_roots_and_plugins(
    query: &str,
    roots: impl IntoIterator<Item = PathBuf>,
    plugin_roots: &[PluginCommandRoot],
) -> Vec<CommandDto> {
    let mut commands = builtin_commands();
    let mut seen: BTreeSet<String> = commands.iter().map(|c| c.id.clone()).collect();

    for dir in command_dirs(roots) {
        match deepagent_prompts::discover_commands(&dir) {
            Ok(defs) => {
                for def in defs {
                    let id = format!("slash.{}", def.name);
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let description = if def.description.trim().is_empty() {
                        "项目提示命令。".to_string()
                    } else {
                        def.description
                    };
                    commands.push(CommandDto {
                        id,
                        title: format!("/{}", def.name),
                        description,
                        category: "项目命令".to_string(),
                        shortcut: None,
                    });
                }
            }
            Err(e) => {
                tracing::warn!(path = %dir.display(), error = %e, "failed to discover commands")
            }
        }
    }

    for plugin in plugin_roots {
        match deepagent_prompts::discover_commands(&plugin.path) {
            Ok(defs) => {
                for def in defs {
                    let namespaced = format!("{}:{}", plugin.plugin_name, def.name);
                    let id = format!("slash.{namespaced}");
                    if !seen.insert(id.clone()) {
                        continue;
                    }
                    let description = if def.description.trim().is_empty() {
                        "Plugin prompt command.".to_string()
                    } else {
                        def.description
                    };
                    commands.push(CommandDto {
                        id,
                        title: format!("/{namespaced}"),
                        description,
                        category: "Plugin Commands".to_string(),
                        shortcut: None,
                    });
                }
            }
            Err(e) => {
                tracing::warn!(
                    plugin = plugin.plugin_id.as_str(),
                    path = %plugin.path.display(),
                    error = %e,
                    "failed to discover plugin commands"
                )
            }
        }
    }

    filter_commands(query, &commands)
}

pub(crate) fn command_dirs(roots: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for root in roots {
        for dir in [
            root.join("commands"),
            root.join(".deepagent").join("commands"),
            root.join(".claude").join("commands"),
        ] {
            let key = normalize_for_dedupe(&dir);
            if seen.insert(key) {
                out.push(dir);
            }
        }
    }
    out
}

fn normalize_for_dedupe(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// Filter commands by a fuzzy query (case-insensitive subsequence match on the
/// command title and category). An empty query returns all commands.
pub fn filter_commands(query: &str, commands: &[CommandDto]) -> Vec<CommandDto> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return commands.to_vec();
    }
    commands
        .iter()
        .filter(|c| {
            let hay = format!("{} {}", c.title, c.category).to_lowercase();
            is_subsequence(&q, &hay)
        })
        .cloned()
        .collect()
}

/// Whether `needle` is a subsequence of `haystack` (the standard fuzzy match).
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    for nc in needle.chars() {
        if nc == ' ' {
            continue;
        }
        loop {
            match chars.next() {
                Some(hc) if hc == nc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_set_is_nonempty_and_unique() {
        let cmds = builtin_commands();
        assert!(cmds.len() >= 5);
        let mut ids: Vec<&str> = cmds.iter().map(|c| c.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "command ids must be unique");
        assert!(cmds.iter().all(|c| !c.description.trim().is_empty()));
    }

    #[test]
    fn empty_query_returns_all() {
        let cmds = builtin_commands();
        assert_eq!(filter_commands("", &cmds).len(), cmds.len());
    }

    #[test]
    fn fuzzy_matches_subsequence() {
        let cmds = builtin_commands();
        let hits = filter_commands("nsession", &cmds);
        assert!(hits.iter().any(|c| c.id == "session.new"));
    }

    #[test]
    fn matches_by_category() {
        let cmds = builtin_commands();
        let hits = filter_commands("mcp", &cmds);
        assert!(hits.iter().any(|c| c.id == "mcp.list"));
    }

    #[test]
    fn includes_slash_commands_for_composer_completion() {
        let cmds = builtin_commands();
        assert!(cmds.iter().any(|c| c.id == "slash.plan"));
        assert!(filter_commands("plan", &cmds)
            .iter()
            .any(|c| c.title == "/plan"));
    }

    #[test]
    fn discovers_project_command_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("triage.md"),
            "---\ndescription: Triage GitHub issues\n---\nReview $ARGUMENTS",
        )
        .unwrap();

        let hits = commands_from_roots("triage", [tmp.path().to_path_buf()]);
        assert!(hits.iter().any(|c| {
            c.id == "slash.triage"
                && c.title == "/triage"
                && c.description == "Triage GitHub issues"
        }));
    }

    #[test]
    fn dynamic_commands_do_not_replace_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plan.md"),
            "---\ndescription: Shadow plan\n---\nshadow",
        )
        .unwrap();

        let hits = commands_from_roots("plan", [tmp.path().to_path_buf()]);
        assert_eq!(hits.iter().filter(|c| c.id == "slash.plan").count(), 1);
        assert!(hits
            .iter()
            .any(|c| c.id == "slash.plan" && c.category == "内置命令"));
    }

    #[test]
    fn no_match_returns_empty() {
        let cmds = builtin_commands();
        assert!(filter_commands("zzzzzz", &cmds).is_empty());
    }

    #[test]
    fn plugin_commands_are_namespaced_and_cannot_shadow_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugin-commands");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("ship.md"),
            "---\ndescription: Ship the plugin\n---\nShip $ARGUMENTS",
        )
        .unwrap();
        std::fs::write(
            dir.join("plan.md"),
            "---\ndescription: Plugin plan\n---\nPlugin plan",
        )
        .unwrap();

        let plugin_roots = vec![PluginCommandRoot {
            plugin_id: "demo-plugin@personal".to_string(),
            plugin_name: "demo-plugin".to_string(),
            path: dir,
        }];
        let hits = commands_from_roots_and_plugins("demo", Vec::<PathBuf>::new(), &plugin_roots);

        assert!(hits.iter().any(|c| {
            c.id == "slash.demo-plugin:ship"
                && c.title == "/demo-plugin:ship"
                && c.category == "Plugin Commands"
        }));
        assert!(hits
            .iter()
            .any(|c| { c.id == "slash.demo-plugin:plan" && c.title == "/demo-plugin:plan" }));
        assert!(!hits
            .iter()
            .any(|c| c.id == "slash.plan" && c.description == "Plugin plan"));
    }
}
