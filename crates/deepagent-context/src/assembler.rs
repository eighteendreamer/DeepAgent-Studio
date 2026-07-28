//! Source-aware context assembly compatible with DeepAgent and Claude layouts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tokenizer::TokenCounter;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSourceKind {
    System,
    RuntimeEnvironment,
    Managed,
    UserInstructions,
    ProjectInstructions,
    NestedInstructions,
    GitContext,
    PermissionContext,
    PluginContext,
    ToolCatalog,
    SkillCatalog,
    McpCatalog,
    Conversation,
    UserPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextEntry {
    pub source: ContextSourceKind,
    pub origin: String,
    pub priority: u16,
    pub cacheable: bool,
    pub required: bool,
    pub tokens: usize,
    pub content: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextManifest {
    pub entries: Vec<ContextEntry>,
    pub total_tokens: usize,
    pub dropped_origins: Vec<String>,
    pub loaded_paths: Vec<PathBuf>,
}

impl ContextManifest {
    pub fn render(&self) -> String {
        self.entries
            .iter()
            .map(|entry| entry.content.trim())
            .filter(|content| !content.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[derive(Debug, Clone)]
pub struct ContextAssembler {
    workspace: PathBuf,
    user_home: Option<PathBuf>,
    entries: Vec<(ContextSourceKind, String, u16, bool, bool, String)>,
}

impl ContextAssembler {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            user_home: home_dir(),
            entries: Vec::new(),
        }
    }

    pub fn with_user_home(mut self, home: Option<PathBuf>) -> Self {
        self.user_home = home;
        self
    }

    pub fn push(
        mut self,
        source: ContextSourceKind,
        origin: impl Into<String>,
        priority: u16,
        cacheable: bool,
        required: bool,
        content: impl Into<String>,
    ) -> Self {
        self.entries.push((
            source,
            origin.into(),
            priority,
            cacheable,
            required,
            content.into(),
        ));
        self
    }

    pub fn assemble(mut self, counter: &dyn TokenCounter, token_budget: usize) -> ContextManifest {
        let mut loaded_paths = Vec::new();
        self.load_instruction_files(&mut loaded_paths);

        let mut entries = self
            .entries
            .into_iter()
            .filter_map(|(source, origin, priority, cacheable, required, content)| {
                let content = content.trim().to_string();
                if content.is_empty() {
                    return None;
                }
                Some(ContextEntry {
                    source,
                    origin,
                    priority,
                    cacheable,
                    required,
                    tokens: counter.count(&content),
                    content,
                })
            })
            .collect::<Vec<_>>();

        // Canonical source order, then lower-precedence sources before native
        // DeepAgent entries at the same scope. Higher priority survives budget
        // pressure, but rendering remains deterministic.
        entries.sort_by(|a, b| {
            source_rank(a.source)
                .cmp(&source_rank(b.source))
                .then(a.priority.cmp(&b.priority))
                .then(a.origin.cmp(&b.origin))
        });

        let mut total = entries.iter().map(|entry| entry.tokens).sum::<usize>();
        let mut dropped = Vec::new();
        if total > token_budget {
            let mut candidates = entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| !entry.required)
                .map(|(index, entry)| (index, entry.priority, entry.tokens))
                .collect::<Vec<_>>();
            candidates.sort_by_key(|(_, priority, _)| *priority);
            let mut remove = std::collections::BTreeSet::new();
            for (index, _, tokens) in candidates {
                if total <= token_budget {
                    break;
                }
                total = total.saturating_sub(tokens);
                remove.insert(index);
            }
            entries = entries
                .into_iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    if remove.contains(&index) {
                        dropped.push(entry.origin);
                        None
                    } else {
                        Some(entry)
                    }
                })
                .collect();
        }

        ContextManifest {
            entries,
            total_tokens: total,
            dropped_origins: dropped,
            loaded_paths,
        }
    }

    fn load_instruction_files(&mut self, loaded_paths: &mut Vec<PathBuf>) {
        if let Some(home) = self.user_home.clone() {
            for path in [
                home.join(".claude").join("CLAUDE.md"),
                home.join(".deepagent").join("AGENTS.md"),
            ] {
                self.load_file(
                    &path,
                    ContextSourceKind::UserInstructions,
                    if path.to_string_lossy().contains(".deepagent") {
                        410
                    } else {
                        400
                    },
                    loaded_paths,
                );
            }
        }

        for path in [
            self.workspace.join("CLAUDE.md"),
            self.workspace.join("AGENTS.md"),
        ] {
            self.load_file(
                &path,
                ContextSourceKind::ProjectInstructions,
                if path.ends_with("AGENTS.md") {
                    610
                } else {
                    600
                },
                loaded_paths,
            );
        }

        // Claude rules load first; native DeepAgent rules at the same scope
        // receive higher priority and therefore win under budget pressure.
        for (dir, priority) in [
            (self.workspace.join(".claude").join("rules"), 620),
            (self.workspace.join(".deepagent").join("rules"), 630),
        ] {
            for path in markdown_files(&dir) {
                self.load_file(
                    &path,
                    ContextSourceKind::ProjectInstructions,
                    priority,
                    loaded_paths,
                );
            }
        }
    }

    fn load_file(
        &mut self,
        path: &Path,
        source: ContextSourceKind,
        priority: u16,
        loaded_paths: &mut Vec<PathBuf>,
    ) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        if content.trim().is_empty() {
            return;
        }
        loaded_paths.push(path.to_path_buf());
        self.entries.push((
            source,
            path.display().to_string(),
            priority,
            true,
            false,
            format!(
                "<workspace-instructions source=\"{}\">\n{}\n</workspace-instructions>",
                path.display(),
                content.trim()
            ),
        ));
    }
}

fn source_rank(source: ContextSourceKind) -> u8 {
    match source {
        ContextSourceKind::System => 0,
        ContextSourceKind::RuntimeEnvironment => 1,
        ContextSourceKind::Managed => 2,
        ContextSourceKind::UserInstructions => 3,
        ContextSourceKind::ProjectInstructions => 4,
        ContextSourceKind::NestedInstructions => 5,
        ContextSourceKind::GitContext => 6,
        ContextSourceKind::PermissionContext => 7,
        ContextSourceKind::PluginContext => 8,
        ContextSourceKind::ToolCatalog => 9,
        ContextSourceKind::SkillCatalog => 10,
        ContextSourceKind::McpCatalog => 11,
        ContextSourceKind::Conversation => 12,
        ContextSourceKind::UserPrompt => 13,
    }
}

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, out);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    visit(root, &mut out);
    out.sort();
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::HeuristicTokenizer;

    #[test]
    fn loads_both_layouts_and_native_has_higher_priority() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "claude rule").unwrap();
        std::fs::write(dir.path().join("AGENTS.md"), "deepagent rule").unwrap();
        let manifest = ContextAssembler::new(dir.path())
            .with_user_home(None)
            .assemble(&HeuristicTokenizer::new(), 10_000);
        assert_eq!(manifest.loaded_paths.len(), 2);
        let priorities = manifest
            .entries
            .iter()
            .map(|entry| entry.priority)
            .collect::<Vec<_>>();
        assert_eq!(priorities, vec![600, 610]);
        assert!(manifest.render().contains("claude rule"));
        assert!(manifest.render().contains("deepagent rule"));
    }

    #[test]
    fn drops_low_priority_optional_context_first() {
        let manifest = ContextAssembler::new("missing")
            .with_user_home(None)
            .push(
                ContextSourceKind::System,
                "system",
                1000,
                true,
                true,
                "required",
            )
            .push(
                ContextSourceKind::Conversation,
                "old",
                10,
                false,
                false,
                "old ".repeat(100),
            )
            .assemble(&HeuristicTokenizer::new(), 5);
        assert_eq!(manifest.entries.len(), 1);
        assert_eq!(manifest.dropped_origins, vec!["old"]);
    }
}
