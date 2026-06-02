//! The built-in command palette (Codex-style ⌘K actions).
//!
//! Returns the static set of commands the UI surfaces in its command palette,
//! plus a fuzzy filter so the frontend can render results as the user types.
//! Commands are pure metadata here; dispatch (what each id does) lives in the
//! UI / Tauri command layer.

use crate::dto::CommandDto;

/// The built-in command set.
pub fn builtin_commands() -> Vec<CommandDto> {
    let c = |id: &str, title: &str, category: &str, shortcut: Option<&str>| CommandDto {
        id: id.to_string(),
        title: title.to_string(),
        category: category.to_string(),
        shortcut: shortcut.map(|s| s.to_string()),
    };
    vec![
        c("session.new", "New Session", "Session", Some("Ctrl+N")),
        c("session.end", "End Session", "Session", None),
        c(
            "session.refresh",
            "Refresh Sessions",
            "Session",
            Some("Ctrl+R"),
        ),
        c("view.timeline", "Show Timeline", "View", Some("Ctrl+1")),
        c(
            "view.metrics",
            "Toggle Metrics Panel",
            "View",
            Some("Ctrl+2"),
        ),
        c("view.diff", "Open Diff View", "View", Some("Ctrl+D")),
        c(
            "approvals.review",
            "Review Pending Approvals",
            "Approvals",
            Some("Ctrl+Shift+A"),
        ),
        c("mcp.list", "List MCP Servers", "MCP", None),
        c("theme.toggle", "Toggle Theme", "View", None),
        c("slash.compact", "/compact", "Slash", None),
        c("slash.cost", "/cost", "Slash", None),
        c("slash.doctor", "/doctor", "Slash", None),
        c("slash.plan", "/plan", "Slash", None),
        c("slash.execute", "/execute", "Slash", None),
        c("slash.resume", "/resume", "Slash", None),
        c("slash.model", "/model", "Slash", None),
        c("slash.clear", "/clear", "Slash", None),
    ]
}

/// Filter commands by a fuzzy query (case-insensitive subsequence match on the
/// title and category). An empty query returns all commands.
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
    fn no_match_returns_empty() {
        let cmds = builtin_commands();
        assert!(filter_commands("zzzzzz", &cmds).is_empty());
    }
}
