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
            data_dir: tmp.path().join("plugin-data"),
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

    /// Real-model end-to-end (no mock): the repo's `/review` command prompt +
    /// the built-in `deepagent-code-review` skill body drive a live DeepSeek
    /// review of a synthetic diff with a planted off-by-one bug. Asserts the
    /// output honors the skill contract ([P0]-[P3] findings + overall verdict).
    /// Reads the key from `DEEPSEEK_API_KEY` or the desktop keychain; skips
    /// cleanly if absent. Run with: `cargo test -p deepagent-app-core
    /// --features web,runtimes,keychain -- --ignored real_deepseek_review --nocapture`.
    #[cfg(all(feature = "keychain", feature = "web"))]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_review_command_honors_finding_contract() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport};
        use std::sync::Arc;

        let key = std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| {
                KeychainStore::new("deepagent-studio")
                    .get("deepseek_api_key")
                    .ok()
                    .flatten()
            });
        let Some(key) = key else {
            eprintln!("[skip] no DeepSeek key in env or keychain");
            return;
        };
        eprintln!("[real-model] key resolved (len={})", key.len());

        // Load the real shipped assets: /review command + built-in skill body.
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let command_path = repo_root.join(".deepagent/commands/review.md");
        let skill_path = repo_root.join(".deepagent/skills/deepagent-code-review/SKILL.md");
        if !command_path.is_file() || !skill_path.is_file() {
            eprintln!("[skip] review command or skill asset missing in this checkout");
            return;
        }
        let command = deepagent_prompts::load_command_file(&command_path).expect("review.md");
        let skill_body = std::fs::read_to_string(&skill_path).expect("skill body");

        // Synthetic diff with a planted off-by-one bug (`<` should be `<=`).
        let diff = "diff --git a/src/pager.rs b/src/pager.rs\n\
            +/// Returns the number of pages needed for `items` at `per_page`.\n\
            +pub fn page_count(items: usize, per_page: usize) -> usize {\n\
            +    let mut pages = 0;\n\
            +    let mut done = 0;\n\
            +    while done < items - per_page {\n\
            +        pages += 1;\n\
            +        done += per_page;\n\
            +    }\n\
            +    pages\n\
            +}\n";
        let user_prompt = command.render(diff);

        let client = ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        );
        let request = deepagent_models::chat::ResponseRequest::new(
            "deepseek-chat".to_string(),
            vec![
                deepagent_core::message::Message::system(&skill_body),
                deepagent_core::message::Message::user(&user_prompt),
            ],
        )
        .with_temperature(0.2)
        .with_max_output_tokens(2048);
        let response = client.stream_response(request).await.expect("live review");
        let text = response.message.content;
        eprintln!("[real-model] review output:\n{text}");

        // Contract: at least one prioritized finding tag and a verdict.
        let has_priority_tag = ["[P0]", "[P1]", "[P2]", "[P3]"]
            .iter()
            .any(|t| text.contains(t));
        assert!(
            has_priority_tag,
            "review output must contain a [P0]-[P3] prefixed finding"
        );
        assert!(
            text.contains("不正确") || text.to_ascii_lowercase().contains("incorrect"),
            "planted off-by-one/underflow bug should yield an 'incorrect' overall verdict"
        );
    }

    /// Real-model end-to-end (no mock): live DeepSeek receives the
    /// enter/exit_worktree schemas plus an explicit "work in a worktree"
    /// request, must call `enter_worktree`, and the model-produced call must
    /// succeed against a real git repo. Run with: `cargo test -p
    /// deepagent-app-core --features web,runtimes,keychain -- --ignored
    /// real_deepseek_worktree --nocapture`.
    #[cfg(all(feature = "keychain", feature = "web"))]
    #[tokio::test]
    #[ignore = "hits the real DeepSeek API; run explicitly with --ignored"]
    async fn real_deepseek_worktree_tool_call_roundtrip() {
        use crate::secret_store::{KeychainStore, SecretStore};
        use deepagent_builtins::{
            worktree_session_state, EnterWorktreeTool, ExitWorktreeTool, ENTER_WORKTREE_TOOL_NAME,
        };
        use deepagent_models::{ModelClient, ModelConfig, ReqwestTransport, ToolSchema};
        use deepagent_tools::Tool;
        use std::sync::Arc;

        let key = std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .or_else(|| {
                KeychainStore::new("deepagent-studio")
                    .get("deepseek_api_key")
                    .ok()
                    .flatten()
            });
        let Some(key) = key else {
            eprintln!("[skip] no DeepSeek key in env or keychain");
            return;
        };
        eprintln!("[real-model] key resolved (len={})", key.len());

        // Real git repo in a tempdir.
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&cwd)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q"]) {
            eprintln!("[skip] git unavailable");
            return;
        }
        assert!(git(&["config", "user.email", "t@example.com"]));
        assert!(git(&["config", "user.name", "t"]));
        assert!(git(&["commit", "--allow-empty", "-q", "-m", "init"]));

        let state = worktree_session_state();
        let enter = EnterWorktreeTool::new(
            deepagent_builtins::bash_tool::SystemExecutor,
            &cwd,
            state.clone(),
        );
        let exit =
            ExitWorktreeTool::new(deepagent_builtins::bash_tool::SystemExecutor, &cwd, state);
        let (ed, xd) = (enter.descriptor(), exit.descriptor());
        let schemas = vec![
            ToolSchema::function(ed.name, ed.description, ed.parameters),
            ToolSchema::function(xd.name, xd.description, xd.parameters),
        ];

        let client = ModelClient::new(
            Arc::new(ReqwestTransport::new()),
            ModelConfig::deepseek(key),
        );
        let request = deepagent_models::chat::ResponseRequest::new(
            "deepseek-chat".to_string(),
            vec![
                deepagent_core::message::Message::system(
                    "You are a coding agent. Use the provided tools when appropriate.",
                ),
                deepagent_core::message::Message::user(
                    "请在一个隔离的 worktree 中开始开发 feature-login 功能（用户明确要求使用 \
                     worktree）。先创建 worktree。",
                ),
            ],
        )
        .with_tools(schemas)
        .with_temperature(0.0)
        .with_max_output_tokens(512);
        let response = client.stream_response(request).await.expect("live call");
        let call = response
            .message
            .tool_calls
            .iter()
            .find(|c| c.name == ENTER_WORKTREE_TOOL_NAME)
            .expect("model must call enter_worktree for an explicit worktree request")
            .clone();
        eprintln!("[real-model] tool call args: {}", call.arguments);

        // Execute the model-produced call against the real repo.
        let out = enter.invoke(call.arguments).await.unwrap();
        assert!(
            out.ok,
            "model-produced enter_worktree failed: {}",
            out.value
        );
        let path = out.value["worktree_path"].as_str().unwrap();
        assert!(std::path::Path::new(path).is_dir());
        eprintln!("[real-model] worktree created at {path}");
    }
}
