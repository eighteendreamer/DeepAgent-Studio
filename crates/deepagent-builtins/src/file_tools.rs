//! File-system built-in tools, aligned with Claude Code's Read/Write/Edit/LS/
//! Glob/Grep. All paths are confined by [`WorkspaceRoot`] (no traversal, no
//! sensitive files). Each tool declares the right [`RiskLevel`] and required
//! [`Permission`] so the capability registry gates it.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolOutput};

use crate::fs_guard::WorkspaceRoot;
use crate::glob_match::glob_match;

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// `read_file` — read a UTF-8 text file within the workspace.
pub struct ReadFileTool {
    root: WorkspaceRoot,
}

impl ReadFileTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_file".into(),
            description: "Read a UTF-8 text file within the workspace. Args: { path }.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(path) = arg_str(&args, "path") else {
            return Ok(ToolOutput::failure("missing 'path'"));
        };
        let resolved = match self.root.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => Ok(ToolOutput::success(serde_json::json!({
                "path": self.root.relativize(&resolved),
                "content": content,
                "lines": content.lines().count(),
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("read failed: {e}"))),
        }
    }
}

/// `write_file` — create or overwrite a file within the workspace.
pub struct WriteFileTool {
    root: WorkspaceRoot,
}

impl WriteFileTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file".into(),
            description:
                "Create or overwrite a file within the workspace. Args: { path, content }.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let (Some(path), Some(content)) = (arg_str(&args, "path"), arg_str(&args, "content"))
        else {
            return Ok(ToolOutput::failure("missing 'path' or 'content'"));
        };
        let resolved = match self.root.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        if let Some(parent) = resolved.parent() {
            if let Err(e) = tokio::fs::create_dir_all(parent).await {
                return Ok(ToolOutput::failure(format!("mkdir failed: {e}")));
            }
        }
        match tokio::fs::write(&resolved, content).await {
            Ok(()) => Ok(ToolOutput::success(serde_json::json!({
                "path": self.root.relativize(&resolved),
                "bytes": content.len(),
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("write failed: {e}"))),
        }
    }
}

/// `edit_file` — replace the first occurrence of `old` with `new` in a file.
/// Mirrors Claude Code's Edit: exact, unique-ish string replacement.
pub struct EditFileTool {
    root: WorkspaceRoot,
}

impl EditFileTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit_file".into(),
            description:
                "Replace an exact string in a file. Args: { path, old, new, replace_all? }.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                    "replace_all": { "type": "boolean" }
                },
                "required": ["path", "old", "new"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let (Some(path), Some(old), Some(new)) = (
            arg_str(&args, "path"),
            arg_str(&args, "old"),
            arg_str(&args, "new"),
        ) else {
            return Ok(ToolOutput::failure("missing 'path', 'old', or 'new'"));
        };
        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let resolved = match self.root.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::failure(format!("read failed: {e}"))),
        };
        let count = content.matches(old).count();
        if count == 0 {
            return Ok(ToolOutput::failure("the 'old' string was not found"));
        }
        if count > 1 && !replace_all {
            return Ok(ToolOutput::failure(format!(
                "'old' matches {count} times; pass replace_all=true or provide more context"
            )));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        match tokio::fs::write(&resolved, &updated).await {
            Ok(()) => Ok(ToolOutput::success(serde_json::json!({
                "path": self.root.relativize(&resolved),
                "replacements": if replace_all { count } else { 1 },
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("write failed: {e}"))),
        }
    }
}

/// `multi_edit` — apply several ordered exact-string replacements to one file
/// **atomically**: all edits succeed or none are written. Mirrors Claude Code's
/// `MultiEdit`. Edits apply in sequence (each to the result of the previous),
/// so later edits can target text introduced by earlier ones.
pub struct MultiEditTool {
    root: WorkspaceRoot,
}

impl MultiEditTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for MultiEditTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "multi_edit".into(),
            description: "Apply an ordered list of exact-string replacements to a single file \
                atomically (all-or-nothing). Args: { path, edits: [{ old, new, replace_all? }] }."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "edits": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old": { "type": "string" },
                                "new": { "type": "string" },
                                "replace_all": { "type": "boolean" }
                            },
                            "required": ["old", "new"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(path) = arg_str(&args, "path") else {
            return Ok(ToolOutput::failure("missing 'path'"));
        };
        let Some(edits) = args.get("edits").and_then(|v| v.as_array()) else {
            return Ok(ToolOutput::failure("missing 'edits' array"));
        };
        if edits.is_empty() {
            return Ok(ToolOutput::failure("'edits' must not be empty"));
        }

        let resolved = match self.root.resolve(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        let mut content = match tokio::fs::read_to_string(&resolved).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolOutput::failure(format!("read failed: {e}"))),
        };

        // Apply edits in order against an in-memory copy; only persist if every
        // edit succeeds (atomic).
        let mut total_replacements = 0usize;
        for (i, edit) in edits.iter().enumerate() {
            let (Some(old), Some(new)) = (arg_str(edit, "old"), arg_str(edit, "new")) else {
                return Ok(ToolOutput::failure(format!(
                    "edit[{i}] missing 'old' or 'new'"
                )));
            };
            let replace_all = edit
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let count = content.matches(old).count();
            if count == 0 {
                return Ok(ToolOutput::failure(format!(
                    "edit[{i}]: the 'old' string was not found (no edits applied)"
                )));
            }
            if count > 1 && !replace_all {
                return Ok(ToolOutput::failure(format!(
                    "edit[{i}]: 'old' matches {count} times; pass replace_all=true or add context \
                     (no edits applied)"
                )));
            }
            content = if replace_all {
                total_replacements += count;
                content.replace(old, new)
            } else {
                total_replacements += 1;
                content.replacen(old, new, 1)
            };
        }

        match tokio::fs::write(&resolved, &content).await {
            Ok(()) => Ok(ToolOutput::success(serde_json::json!({
                "path": self.root.relativize(&resolved),
                "edits_applied": edits.len(),
                "replacements": total_replacements,
            }))),
            Err(e) => Ok(ToolOutput::failure(format!("write failed: {e}"))),
        }
    }
}

/// `list_dir` — list a directory's immediate entries within the workspace.
pub struct ListDirTool {
    root: WorkspaceRoot,
}

impl ListDirTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "list_dir".into(),
            description: "List immediate entries of a directory. Args: { path? } (default root)."
                .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let rel = arg_str(&args, "path").unwrap_or(".");
        let resolved = match self.root.resolve(rel) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        let mut rd = match tokio::fs::read_dir(&resolved).await {
            Ok(rd) => rd,
            Err(e) => return Ok(ToolOutput::failure(format!("read_dir failed: {e}"))),
        };
        let mut entries = Vec::new();
        while let Ok(Some(e)) = rd.next_entry().await {
            let is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            let name = e.file_name().to_string_lossy().to_string();
            entries.push(serde_json::json!({ "name": name, "dir": is_dir }));
        }
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(ToolOutput::success(serde_json::json!({
            "path": self.root.relativize(&resolved),
            "entries": entries,
        })))
    }
}

/// `glob` — find files matching a glob pattern under the workspace root.
pub struct GlobTool {
    root: WorkspaceRoot,
    max_results: usize,
}

impl GlobTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root,
            max_results: 500,
        }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "glob".into(),
            description: "Find files matching a glob (e.g. src/**/*.rs). Args: { pattern }.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "pattern": { "type": "string" } },
                "required": ["pattern"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(pattern) = arg_str(&args, "pattern") else {
            return Ok(ToolOutput::failure("missing 'pattern'"));
        };
        let mut matches = Vec::new();
        let root = self.root.path().to_path_buf();
        walk_collect(&root, &root, pattern, &mut matches, self.max_results);
        matches.sort();
        Ok(ToolOutput::success(serde_json::json!({
            "pattern": pattern,
            "matches": matches,
            "truncated": matches.len() >= self.max_results,
        })))
    }
}

/// `grep` — search file contents for a literal substring under the workspace.
pub struct GrepTool {
    root: WorkspaceRoot,
    max_results: usize,
}

impl GrepTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self {
            root,
            max_results: 200,
        }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "grep".into(),
            description:
                "Search file contents for a literal substring. Args: { query, glob? (default **) }."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "glob": { "type": "string" }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(query) = arg_str(&args, "query") else {
            return Ok(ToolOutput::failure("missing 'query'"));
        };
        let pattern = arg_str(&args, "glob").unwrap_or("**");
        let root = self.root.path().to_path_buf();

        let mut files = Vec::new();
        walk_collect(&root, &root, pattern, &mut files, 5000);

        let mut hits = Vec::new();
        for rel in &files {
            if hits.len() >= self.max_results {
                break;
            }
            let full = root.join(rel);
            // Skip unreadable / binary-ish files quietly.
            let Ok(content) = tokio::fs::read_to_string(&full).await else {
                continue;
            };
            for (i, line) in content.lines().enumerate() {
                if line.contains(query) {
                    hits.push(serde_json::json!({
                        "file": rel,
                        "line": i + 1,
                        "text": line.chars().take(200).collect::<String>(),
                    }));
                    if hits.len() >= self.max_results {
                        break;
                    }
                }
            }
        }
        Ok(ToolOutput::success(serde_json::json!({
            "query": query,
            "hits": hits,
            "truncated": hits.len() >= self.max_results,
        })))
    }
}

/// Directories never descended into during glob/grep.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    ".venv",
    "__pycache__",
];

/// Recursively collect workspace-relative, forward-slashed paths matching
/// `pattern`. Bounded by `limit`.
fn walk_collect(root: &Path, dir: &Path, pattern: &str, out: &mut Vec<String>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                continue;
            }
            walk_collect(root, &path, pattern, out, limit);
        } else {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if glob_match(pattern, &rel) {
                out.push(rel);
            }
        }
    }
}

/// Register all file tools into a registry over the given root (helper).
pub fn file_tools(root: WorkspaceRoot) -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(ReadFileTool::new(root.clone())),
        Arc::new(WriteFileTool::new(root.clone())),
        Arc::new(EditFileTool::new(root.clone())),
        Arc::new(MultiEditTool::new(root.clone())),
        Arc::new(ListDirTool::new(root.clone())),
        Arc::new(GlobTool::new(root.clone())),
        Arc::new(GrepTool::new(root)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> (tempfile::TempDir, WorkspaceRoot) {
        let dir = tempfile::tempdir().unwrap();
        let root = WorkspaceRoot::new(dir.path());
        (dir, root)
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let (_d, root) = temp_root();
        let w = WriteFileTool::new(root.clone());
        let out = w
            .invoke(serde_json::json!({"path": "src/a.txt", "content": "hello"}))
            .await
            .unwrap();
        assert!(out.ok);

        let r = ReadFileTool::new(root.clone());
        let out = r
            .invoke(serde_json::json!({"path": "src/a.txt"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["content"], "hello");
    }

    #[tokio::test]
    async fn read_rejects_traversal_and_sensitive() {
        let (_d, root) = temp_root();
        let r = ReadFileTool::new(root.clone());
        assert!(
            !r.invoke(serde_json::json!({"path": "../../etc/passwd"}))
                .await
                .unwrap()
                .ok
        );
        assert!(
            !r.invoke(serde_json::json!({"path": ".env"}))
                .await
                .unwrap()
                .ok
        );
    }

    #[tokio::test]
    async fn edit_replaces_unique_string() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "foo bar baz"}))
            .await
            .unwrap();
        let e = EditFileTool::new(root.clone());
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "bar", "new": "QUX"}))
            .await
            .unwrap();
        assert!(out.ok);
        let content = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        assert_eq!(content.value["content"], "foo QUX baz");
    }

    #[tokio::test]
    async fn edit_ambiguous_match_requires_replace_all() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "a a a"}))
            .await
            .unwrap();
        let e = EditFileTool::new(root.clone());
        // Without replace_all, 3 matches => failure.
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "a", "new": "b"}))
            .await
            .unwrap();
        assert!(!out.ok);
        // With replace_all, succeeds.
        let out = e
            .invoke(
                serde_json::json!({"path": "x.txt", "old": "a", "new": "b", "replace_all": true}),
            )
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["replacements"], 3);
    }

    #[tokio::test]
    async fn glob_and_grep_find_files() {
        let (_d, root) = temp_root();
        let w = WriteFileTool::new(root.clone());
        w.invoke(serde_json::json!({"path": "src/main.rs", "content": "fn main() { let x = 1; }"}))
            .await
            .unwrap();
        w.invoke(serde_json::json!({"path": "src/lib.rs", "content": "pub fn f() {}"}))
            .await
            .unwrap();
        w.invoke(serde_json::json!({"path": "README.md", "content": "# docs"}))
            .await
            .unwrap();

        let g = GlobTool::new(root.clone());
        let out = g
            .invoke(serde_json::json!({"pattern": "src/**/*.rs"}))
            .await
            .unwrap();
        let matches = out.value["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 2);

        let grep = GrepTool::new(root.clone());
        let out = grep
            .invoke(serde_json::json!({"query": "fn main"}))
            .await
            .unwrap();
        let hits = out.value["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["file"], "src/main.rs");
    }

    #[tokio::test]
    async fn list_dir_returns_entries() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "a.txt", "content": "x"}))
            .await
            .unwrap();
        let l = ListDirTool::new(root.clone());
        let out = l.invoke(serde_json::json!({})).await.unwrap();
        assert!(out.ok);
        let entries = out.value["entries"].as_array().unwrap();
        assert!(entries.iter().any(|e| e["name"] == "a.txt"));
    }

    #[test]
    fn file_tools_helper_returns_all() {
        let (_d, root) = temp_root();
        assert_eq!(file_tools(root).len(), 7);
    }

    #[tokio::test]
    async fn multi_edit_applies_all_atomically() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "alpha beta gamma"}))
            .await
            .unwrap();
        let m = MultiEditTool::new(root.clone());
        let out = m
            .invoke(serde_json::json!({
                "path": "x.txt",
                "edits": [
                    { "old": "alpha", "new": "ONE" },
                    { "old": "gamma", "new": "THREE" }
                ]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["edits_applied"], 2);
        let content = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        assert_eq!(content.value["content"], "ONE beta THREE");
    }

    #[tokio::test]
    async fn multi_edit_is_atomic_on_failure() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "keep this"}))
            .await
            .unwrap();
        let m = MultiEditTool::new(root.clone());
        // Second edit's 'old' is absent → whole op fails, nothing written.
        let out = m
            .invoke(serde_json::json!({
                "path": "x.txt",
                "edits": [
                    { "old": "keep", "new": "KEPT" },
                    { "old": "absent", "new": "X" }
                ]
            }))
            .await
            .unwrap();
        assert!(!out.ok);
        let content = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        // Unchanged: the first edit was NOT persisted.
        assert_eq!(content.value["content"], "keep this");
    }

    #[tokio::test]
    async fn multi_edit_sequential_edits_see_prior_results() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "a"}))
            .await
            .unwrap();
        let m = MultiEditTool::new(root.clone());
        // Second edit targets text the first introduced.
        let out = m
            .invoke(serde_json::json!({
                "path": "x.txt",
                "edits": [
                    { "old": "a", "new": "b" },
                    { "old": "b", "new": "c" }
                ]
            }))
            .await
            .unwrap();
        assert!(out.ok);
        let content = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        assert_eq!(content.value["content"], "c");
    }
}
