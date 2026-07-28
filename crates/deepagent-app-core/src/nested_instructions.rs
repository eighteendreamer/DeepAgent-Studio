//! Nested instruction discovery (kernel-refactor Phase C).
//!
//! Claude Code loads `CLAUDE.md` files lazily: the root file is part of the
//! initial context, while nested ones are discovered when the agent actually
//! touches a subdirectory. This module implements the same behavior for
//! DeepAgent as a [`ToolResultDecorator`]: after a file tool reports a
//! `path`, every not-yet-loaded `CLAUDE.md` / `AGENTS.md` on the directory
//! chain between that path and the workspace root is injected into the tool
//! result inside a `<system-reminder>` envelope, and an
//! [`HookPoint::InstructionsLoaded`] dispatch announces the newly loaded
//! paths (mirroring the root-manifest dispatch in `run_in_session`).
//!
//! Dedupe: paths already loaded by the root
//! [`ContextAssembler`](deepagent_context::ContextAssembler) manifest are
//! seeded into the sent-set, and each run announces a given file at most
//! once. Files are size-capped so a pathological instruction file cannot
//! blow the tool-result budget (the decorator runs AFTER the budget step).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use deepagent_core::id::SessionId;
use deepagent_hooks::{HookContext, HookData, HookPoint, HookRegistry};
use deepagent_runtime::ToolResultDecorator;
use deepagent_tools::ToolOutput;

use crate::system_reminder::{append_to_tool_result, wrap};

/// Tool names whose results carry a `path` worth scanning. Write tools are
/// included: creating a file inside a directory is "visiting" it.
const TRIGGER_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "edit_file",
    "multi_edit",
    "list_dir",
];

/// Instruction file names discovered per directory, in injection order.
/// `.deepagent`'s native name wins ties by being listed last (later block
/// appears later in the reminder, closest to the model's attention).
const INSTRUCTION_FILE_NAMES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// Hard cap per instruction file; anything longer is truncated with a note.
const MAX_FILE_BYTES: usize = 24 * 1024;
/// Hard cap of files injected per tool call (deepest-first beyond the cap
/// are deferred to the next trigger).
const MAX_FILES_PER_CALL: usize = 3;

/// Discovers nested `CLAUDE.md` / `AGENTS.md` files when tools touch a
/// directory, injects them as tool-result system reminders, and fires the
/// `InstructionsLoaded` hook for each newly announced batch.
pub struct NestedInstructionsDecorator {
    root: PathBuf,
    /// Paths already in the model's context (root manifest seed + everything
    /// announced earlier in this run).
    announced: Mutex<HashSet<PathBuf>>,
    hooks: Arc<HookRegistry>,
    session_id: SessionId,
}

impl std::fmt::Debug for NestedInstructionsDecorator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NestedInstructionsDecorator")
            .field("root", &self.root)
            .field("session_id", &self.session_id.to_string())
            .finish_non_exhaustive()
    }
}

impl NestedInstructionsDecorator {
    /// Build for one run. `already_loaded` seeds the dedupe set with the
    /// root manifest's instruction paths so they are never re-injected.
    pub fn new(
        root: PathBuf,
        already_loaded: impl IntoIterator<Item = PathBuf>,
        hooks: Arc<HookRegistry>,
        session_id: SessionId,
    ) -> Self {
        Self {
            root,
            announced: Mutex::new(
                already_loaded
                    .into_iter()
                    .map(|path| normalize(&path))
                    .collect(),
            ),
            hooks,
            session_id,
        }
    }

    /// Directories to scan for `path`: the touched file's parent (or the
    /// directory itself) up to — but excluding — the workspace root, whose
    /// instruction files are loaded by the root manifest.
    fn candidate_dirs(&self, path: &Path) -> Vec<PathBuf> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let start = if absolute.is_dir() {
            absolute
        } else {
            match absolute.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return Vec::new(),
            }
        };
        let root = normalize(&self.root);
        let mut dirs = Vec::new();
        let mut current = normalize(&start);
        loop {
            if current == root {
                break;
            }
            if !current.starts_with(&root) {
                // Outside the workspace (full-access reads): only scan the
                // touched directory itself, never walk foreign trees.
                dirs.clear();
                dirs.push(current);
                break;
            }
            dirs.push(current.clone());
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => break,
            }
        }
        // Shallowest first so parent instructions precede child overrides.
        dirs.reverse();
        dirs
    }

    /// Collect not-yet-announced instruction files for `path`, marking them
    /// announced. Returns `(file path, contents)` pairs, truncated to caps.
    fn discover(&self, path: &Path) -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        let mut announced = self.announced.lock().unwrap_or_else(|p| p.into_inner());
        for dir in self.candidate_dirs(path) {
            for name in INSTRUCTION_FILE_NAMES {
                if found.len() >= MAX_FILES_PER_CALL {
                    return found;
                }
                let candidate = dir.join(name);
                if announced.contains(&candidate) {
                    continue;
                }
                let Ok(raw) = std::fs::read_to_string(&candidate) else {
                    continue;
                };
                if raw.trim().is_empty() {
                    continue;
                }
                announced.insert(candidate.clone());
                let mut content = raw.trim().to_string();
                if content.len() > MAX_FILE_BYTES {
                    let mut cut = MAX_FILE_BYTES;
                    while !content.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    content.truncate(cut);
                    content.push_str(
                        "\n… (truncated: instruction file exceeds the nested-discovery size cap)",
                    );
                }
                found.push((candidate, content));
            }
        }
        found
    }
}

#[async_trait]
impl ToolResultDecorator for NestedInstructionsDecorator {
    async fn decorate(&self, tool_name: &str, output: &mut ToolOutput) {
        if !TRIGGER_TOOLS.contains(&tool_name) || !output.ok {
            return;
        }
        let Some(raw_path) = output
            .value
            .get("path")
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
        else {
            return;
        };
        let discovered = self.discover(Path::new(raw_path));
        if discovered.is_empty() {
            return;
        }

        let mut body = String::from(
            "Nested instruction files apply to the directory you just accessed. \
             Follow them for all work under their directory (deeper files take \
             precedence over shallower ones):\n",
        );
        for (path, content) in &discovered {
            body.push_str(&format!(
                "\n<workspace-instructions source=\"{}\">\n{}\n</workspace-instructions>\n",
                path.display(),
                content
            ));
        }
        append_to_tool_result(&mut output.value, &wrap(&body));

        let paths: Vec<String> = discovered
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect();
        if let Err(error) = self
            .hooks
            .dispatch(&HookContext::new(
                self.session_id,
                HookPoint::InstructionsLoaded,
                HookData::Instructions {
                    paths: paths.clone(),
                },
            ))
            .await
        {
            tracing::warn!(error = %error, "InstructionsLoaded hook failed for nested instructions");
        }
        tracing::info!(
            count = paths.len(),
            ?paths,
            "nested instructions discovered"
        );
    }
}

/// Light normalization: component-wise cleanup without touching the
/// filesystem (canonicalize would break on not-yet-created paths and
/// introduce `\\?\` prefixes on Windows).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decorator(root: &Path, seeded: Vec<PathBuf>) -> NestedInstructionsDecorator {
        NestedInstructionsDecorator::new(
            root.to_path_buf(),
            seeded,
            Arc::new(HookRegistry::new()),
            SessionId::new(),
        )
    }

    fn output_with_path(path: &str) -> ToolOutput {
        ToolOutput::success(serde_json::json!({ "path": path, "content": "x" }))
    }

    #[tokio::test]
    async fn discovers_nested_instructions_once_per_run() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("src").join("api");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("CLAUDE.md"), "api rules").unwrap();
        std::fs::write(tmp.path().join("src").join("AGENTS.md"), "src agent rules").unwrap();

        let decorator = decorator(tmp.path(), Vec::new());
        let target = nested.join("handler.rs");
        std::fs::write(&target, "fn main() {}").unwrap();

        let mut output = output_with_path(target.to_string_lossy().as_ref());
        decorator.decorate("read_file", &mut output).await;
        let reminder = output.value["_system_reminder"].as_str().unwrap();
        // Shallow (src/AGENTS.md) before deep (src/api/CLAUDE.md).
        let src_pos = reminder.find("src agent rules").unwrap();
        let api_pos = reminder.find("api rules").unwrap();
        assert!(src_pos < api_pos);

        // Second touch of the same directory injects nothing new.
        let mut second = output_with_path(target.to_string_lossy().as_ref());
        decorator.decorate("read_file", &mut second).await;
        assert!(second.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn root_manifest_paths_are_never_reinjected() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("docs");
        std::fs::create_dir_all(&sub).unwrap();
        let preloaded = sub.join("CLAUDE.md");
        std::fs::write(&preloaded, "docs rules").unwrap();

        let decorator = decorator(tmp.path(), vec![preloaded.clone()]);
        let mut output = output_with_path(sub.join("readme.txt").to_string_lossy().as_ref());
        decorator.decorate("read_file", &mut output).await;
        assert!(output.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn workspace_root_files_and_failed_tools_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("CLAUDE.md"), "root rules").unwrap();
        let decorator = decorator(tmp.path(), Vec::new());

        // Root-level instruction files belong to the root manifest, not to
        // nested discovery.
        let mut output = output_with_path(tmp.path().join("main.rs").to_string_lossy().as_ref());
        decorator.decorate("read_file", &mut output).await;
        assert!(output.value.get("_system_reminder").is_none());

        // Failed tool results and non-trigger tools are untouched.
        let sub = tmp.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("AGENTS.md"), "sub rules").unwrap();
        let mut failed = ToolOutput {
            ok: false,
            value: serde_json::json!({
                "path": sub.join("x.txt").to_string_lossy(),
            }),
            truncated: false,
        };
        decorator.decorate("read_file", &mut failed).await;
        assert!(failed.value.get("_system_reminder").is_none());
        let mut other_tool = output_with_path(sub.join("x.txt").to_string_lossy().as_ref());
        decorator.decorate("bash", &mut other_tool).await;
        assert!(other_tool.value.get("_system_reminder").is_none());
    }

    #[tokio::test]
    async fn relative_paths_resolve_against_workspace_root() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("pkg");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("CLAUDE.md"), "pkg rules").unwrap();

        let decorator = decorator(tmp.path(), Vec::new());
        let mut output = output_with_path("pkg/lib.rs");
        decorator.decorate("write_file", &mut output).await;
        let reminder = output.value["_system_reminder"].as_str().unwrap();
        assert!(reminder.contains("pkg rules"));
    }
}
