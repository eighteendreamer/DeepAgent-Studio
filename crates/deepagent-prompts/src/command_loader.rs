//! Load slash-command definitions from Claude Code-style `commands/<name>.md`
//! files into [`deepagent_intent::CommandDef`].
//!
//! A command file is YAML frontmatter (`description`, `allowed-tools`,
//! `argument-hint`, `disable-model-invocation`) followed by a Markdown body
//! template that may contain `$ARGUMENTS`. The command name comes from the file
//! stem (e.g. `triage-issue.md` → `/triage-issue`).

use std::path::Path;

use deepagent_core::error::{CoreError, Result};
use deepagent_intent::CommandDef;

use crate::frontmatter;

/// Parse a command `.md` document into a [`CommandDef`] with the given name.
pub fn parse_command(name: impl Into<String>, input: &str) -> CommandDef {
    let fm = frontmatter::parse(input);
    let name = name.into();
    let description = fm.get("description").unwrap_or("").to_string();

    let mut def = CommandDef::new(name, description)
        .with_body(fm.body.clone())
        .with_allowed_tools(fm.get_list("allowed-tools"));

    if fm.get_bool("disable-model-invocation").unwrap_or(false) {
        def = def.user_only();
    }
    def
}

/// Load a single command file, deriving the command name from the file stem.
pub fn load_command_file(path: impl AsRef<Path>) -> Result<CommandDef> {
    let path = path.as_ref();
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| CoreError::invalid("command file has no usable name"))?
        .to_string();
    let raw = std::fs::read_to_string(path)
        .map_err(|e| CoreError::other(format!("read {}: {e}", path.display())))?;
    Ok(parse_command(name, &raw))
}

/// Discover all `*.md` command files directly under `dir`, returning their
/// [`CommandDef`]s. A missing directory yields an empty list.
pub fn discover_commands(dir: impl AsRef<Path>) -> Result<Vec<CommandDef>> {
    let dir = dir.as_ref();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let rd = std::fs::read_dir(dir)
        .map_err(|e| CoreError::other(format!("read commands dir {}: {e}", dir.display())))?;
    let mut files: Vec<_> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
        .collect();
    files.sort();

    let mut out = Vec::new();
    for path in files {
        match load_command_file(&path) {
            Ok(def) => out.push(def),
            Err(e) => tracing::warn!(path = %path.display(), error = %e, "skipping command file"),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use deepagent_intent::ARGUMENTS_PLACEHOLDER;

    const TRIAGE: &str = "---\nallowed-tools: Bash(gh:*), Read\ndescription: Triage GitHub issues\n---\nYou are a triage assistant.\n\nContext:\n$ARGUMENTS";

    #[test]
    fn parses_command_with_frontmatter() {
        let def = parse_command("triage", TRIAGE);
        assert_eq!(def.name, "triage");
        assert_eq!(def.description, "Triage GitHub issues");
        assert_eq!(def.allowed_tools, vec!["Bash(gh:*)", "Read"]);
        assert!(def.body.contains(ARGUMENTS_PLACEHOLDER));
        assert!(!def.disable_model_invocation);
    }

    #[test]
    fn render_substitutes_arguments() {
        let def = parse_command("triage", TRIAGE);
        let rendered = def.render("issue #42");
        assert!(rendered.contains("issue #42"));
        assert!(!rendered.contains(ARGUMENTS_PLACEHOLDER));
    }

    /// Smoke test: the repo's shipped `/review` command file parses and keeps
    /// its contract (skill reference, scope resolution, `$ARGUMENTS` slot).
    #[test]
    fn repo_review_command_parses() {
        // CARGO_MANIFEST_DIR = <root>/crates/deepagent-prompts
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.deepagent/commands/review.md");
        if !path.is_file() {
            return; // checkout without the workspace commands tree
        }
        let def = load_command_file(&path).expect("review.md parses");
        assert_eq!(def.name, "review");
        assert!(!def.description.is_empty());
        assert!(def.allowed_tools.iter().any(|t| t.contains("git")));
        assert!(def.body.contains("deepagent-code-review"));
        assert!(def.body.contains(ARGUMENTS_PLACEHOLDER));
        let rendered = def.render("base main");
        assert!(rendered.contains("base main"));
    }

    #[test]
    fn user_only_flag() {
        let def = parse_command(
            "secret",
            "---\ndescription: d\ndisable-model-invocation: true\n---\nbody",
        );
        assert!(def.disable_model_invocation);
    }

    #[test]
    fn discover_reads_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), "---\ndescription: A\n---\nbody A").unwrap();
        std::fs::write(tmp.path().join("b.md"), "---\ndescription: B\n---\nbody B").unwrap();
        std::fs::write(tmp.path().join("ignore.txt"), "not a command").unwrap();

        let cmds = discover_commands(tmp.path()).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].name, "a");
        assert_eq!(cmds[1].name, "b");
    }

    #[test]
    fn discover_missing_dir_empty() {
        assert!(discover_commands("/no/such/commands").unwrap().is_empty());
    }
}
