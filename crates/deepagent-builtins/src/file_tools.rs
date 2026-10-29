//! File-system built-in tools, aligned with Claude Code's Read/Write/Edit/LS/
//! Glob/Grep. All paths are confined by [`WorkspaceRoot`] (no traversal, no
//! sensitive files). Each tool declares the right [`RiskLevel`] and required
//! [`Permission`] so the capability registry gates it.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use deepagent_core::error::Result;
use deepagent_tools::permission::{Permission, PermissionSet, RiskLevel};
use deepagent_tools::{Tool, ToolDescriptor, ToolExecutionContext, ToolOutput};

use crate::file_cache::{FileStateCache, SharedFileStateCache};
use crate::file_mutation::{self, Mutation, Replacement};
use crate::fs_guard::WorkspaceRoot;
use crate::glob_match::glob_match;

const LARGE_FILE_LINE_THRESHOLD: usize = 2000;
const DEFAULT_LARGE_FILE_LIMIT: usize = 2000;

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn arg_usize(args: &serde_json::Value, key: &str) -> Option<std::result::Result<usize, String>> {
    let value = args.get(key)?;
    if let Some(n) = value.as_u64() {
        return Some(usize::try_from(n).map_err(|_| format!("'{key}' is too large")));
    }
    Some(Err(format!("'{key}' must be a positive integer")))
}

fn new_shared_cache() -> SharedFileStateCache {
    Arc::new(Mutex::new(FileStateCache::new()))
}

fn cache_invalidate(cache: &SharedFileStateCache, path: &Path) {
    cache
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .invalidate(path);
}

/// Sentinel content returned by `read_file` when the model re-reads an
/// unchanged file with the same `(offset, limit)` parameters. Replaces the
/// real content to save tokens — the model has already seen it this turn.
const FILE_UNCHANGED_STUB: &str = "<FILE_UNCHANGED_STUB: file unchanged since last read; content omitted to save tokens. To force a fresh read, change offset/limit, edit the file first, or use grep/list_dir.>";

fn read_window(
    content: &str,
    total_lines: usize,
    offset_arg: Option<usize>,
    limit_arg: Option<usize>,
) -> (String, usize, Option<usize>, bool, String) {
    let start_line = offset_arg.unwrap_or(1).max(1);
    let default_large_slice =
        offset_arg.is_none() && limit_arg.is_none() && total_lines > LARGE_FILE_LINE_THRESHOLD;
    let limit = if default_large_slice {
        Some(DEFAULT_LARGE_FILE_LIMIT)
    } else {
        limit_arg
    };

    let selected: Vec<&str> = match limit {
        Some(limit) => content.lines().skip(start_line - 1).take(limit).collect(),
        None => content.lines().skip(start_line - 1).collect(),
    };
    let returned_lines = selected.len();
    let next_offset = if returned_lines == 0 {
        None
    } else {
        let next = start_line + returned_lines;
        (next <= total_lines).then_some(next)
    };
    let truncated = start_line > 1 || next_offset.is_some();
    let mut message = if returned_lines == 0 && start_line > total_lines {
        format!("file has {total_lines} lines; offset {start_line} is past end")
    } else {
        format!("returned {returned_lines} of {total_lines} lines")
    };
    if let Some(next) = next_offset {
        message.push_str(&format!(
            "; output truncated; use offset {next} to read more"
        ));
    }

    (
        format_with_line_numbers(&selected, start_line),
        returned_lines,
        next_offset,
        truncated,
        message,
    )
}

/// Render a `cat -n`-style view of `lines`, where the first line carries
/// `start_line` as its line number. The format mirrors GNU `cat -n`:
/// `<6-wide right-aligned line number>\t<line content>`. The line-number
/// prefix is for the model to reference code locations precisely; it is NOT
/// part of the file content. Subsequent edit_file calls must strip the prefix
/// from `old_string` and preserve the file's original indentation.
fn format_with_line_numbers<S: AsRef<str>>(lines: &[S], start_line: usize) -> String {
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{}", start_line + i, line.as_ref()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `read_file` — read a UTF-8 text file within the workspace.
pub struct ReadFileTool {
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl ReadFileTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self::with_cache(root, new_shared_cache())
    }

    /// Build over a workspace root with a shared per-run cache.
    pub(crate) fn with_cache(root: WorkspaceRoot, cache: SharedFileStateCache) -> Self {
        Self { root, cache }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "read_file".into(),
            description: "Read a UTF-8 text file within the workspace. Args: { path, offset?, limit? }.\n\
                - For understanding code behavior, call chains, impact, or symbol context, prefer codegraph_node/codegraph_explore first; use read_file for known paths, non-code files, pre-edit inspection, or when codegraph is unavailable.\n\
                - The path must be workspace-relative or workspace-absolute.\n\
                - By default reads up to 2000 lines from the start. Use `offset` (1-based starting line) and `limit` (line count) to read a window for very large files.\n\
                - Output format: each line is prefixed by `<line_number>\\t<content>` (cat -n style; line number right-padded to 6 chars). Line numbers start at 1, or at `offset` when given.\n\
                - IMPORTANT: When you later use edit_file, the `old_string` parameter must contain the exact content AFTER the line-number prefix. NEVER include the line-number prefix itself in old_string. Preserve the EXACT indentation (tabs/spaces) of the original file content.\n\
                - Files over 2000 lines auto-truncate; the response carries `next_offset` so you can continue reading.\n\
                - If you call read_file twice on the same path with the same offset/limit and the file hasn't changed, the second call returns a short FILE_UNCHANGED_STUB instead of the body — re-reading is wasted tokens. To force a fresh read, change `offset`/`limit`, modify the file, or use `grep`/`list_dir`.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line number to start reading from."
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of lines to return."
                    }
                },
                "required": ["path"]
            }),
            risk: RiskLevel::Safe,
            required_permissions: PermissionSet::read_only(),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        self.invoke_with_context(args, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let Some(path) = arg_str(&args, "path") else {
            return Ok(ToolOutput::failure("missing 'path'"));
        };
        let offset = match arg_usize(&args, "offset") {
            Some(Ok(0)) => return Ok(ToolOutput::failure("'offset' must be >= 1")),
            Some(Ok(n)) => Some(n),
            Some(Err(e)) => return Ok(ToolOutput::failure(e)),
            None => None,
        };
        let limit = match arg_usize(&args, "limit") {
            Some(Ok(0)) => return Ok(ToolOutput::failure("'limit' must be >= 1")),
            Some(Ok(n)) => Some(n),
            Some(Err(e)) => return Ok(ToolOutput::failure(e)),
            None => None,
        };
        let resolved = match self.root.resolve_read(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        let root = self.root.clone();
        let cache = self.cache.clone();
        tokio::task::spawn_blocking(move || {
            if context.is_cancelled() {
                return ToolOutput::failure("FILE_OPERATION_CANCELLED: no read observation recorded");
            }
            let _operation = match deepagent_persistence::fs_commit::lock_path_operation(&resolved) {
                Ok(guard) => guard,
                Err(error) => return ToolOutput::failure(format!("file operation lock failed: {error}")),
            };
            // Metadata is not content identity: editors can preserve both
            // timestamp and length. Always compare actual bytes before a stub
            // or a newly delivered observation can authorize a later edit.
            let mut file = match root.open_read(&resolved) {
                Ok(file) => file,
                Err(error) => return ToolOutput::failure(format!("read failed: {error}")),
            };
            let mut current = String::new();
            if let Err(error) = file.read_to_string(&mut current) {
                return ToolOutput::failure(format!("read failed: {error}"));
            }
            if context.is_cancelled() {
                return ToolOutput::failure("FILE_OPERATION_CANCELLED: no read observation recorded");
            }
            let mut cache = cache.lock().unwrap_or_else(|p| p.into_inner());
            let (cached, cache_hit) = match cache.matching_content(&resolved, &current) {
                Some(file) => (file, true),
                None => {
                    let metadata = match file.metadata() {
                        Ok(metadata) => metadata,
                        Err(error) => return ToolOutput::failure(format!("metadata failed: {error}")),
                    };
                    (cache.store(resolved.clone(), current, &metadata), false)
                }
            };
            let (content, returned_lines, next_offset, truncated, message) =
                read_window(&cached.content, cached.line_count, offset, limit);
            let unchanged = cache_hit && cache.last_read_matches_in_context(&resolved, context.observation(), offset, limit);
            if unchanged {
                return ToolOutput::success(serde_json::json!({
                "path": root.relativize(&resolved),
                "content": FILE_UNCHANGED_STUB,
                "lines": cached.line_count,
                "offset": offset.unwrap_or(1),
                "limit": limit,
                "returned_lines": 0,
                "truncated": truncated,
                "next_offset": next_offset,
                "cache_hit": true,
                "content_hash": cached.content_hash,
                "unchanged_stub": true,
                "message": "file unchanged since last read with same offset/limit; content omitted to save tokens",
                }));
            }
            cache.record_read_in_context(&resolved, context.observation(), offset, limit);
            cache.record_window(&resolved, context.observation(), offset.unwrap_or(1), returned_lines);
            ToolOutput::success(serde_json::json!({
            "path": root.relativize(&resolved),
            "content": content,
            "lines": cached.line_count,
            "offset": offset.unwrap_or(1),
            "limit": limit,
            "returned_lines": returned_lines,
            "truncated": truncated,
            "next_offset": next_offset,
            "cache_hit": cache_hit,
            "content_hash": cached.content_hash,
            "message": message,
            }))
        }).await.map_err(|error| deepagent_core::error::CoreError::other(format!("file read join failed: {error}")))
    }
}

/// `write_file` — create or overwrite a file within the workspace.
pub struct WriteFileTool {
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl WriteFileTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self::with_cache(root, new_shared_cache())
    }

    /// Build over a workspace root with a shared per-run cache.
    pub(crate) fn with_cache(root: WorkspaceRoot, cache: SharedFileStateCache) -> Self {
        Self { root, cache }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "write_file".into(),
            description:
                "Create or overwrite a file within the workspace. Args: { path, content }.\n\
                - Creating a NEW file (path does not exist): no prior read required.\n\
                - Overwriting an EXISTING file requires read_file coverage of the entire current version (page through large files). Changed or deleted files cause a conflict; read again before retrying. Successful writes invalidate that read authority.".into(),
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
        self.invoke_with_context(args, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let (Some(path), Some(content)) = (arg_str(&args, "path"), arg_str(&args, "content"))
        else {
            return Ok(ToolOutput::failure("missing 'path' or 'content'"));
        };
        let resolved = match self.root.resolve_write(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        file_mutation::apply(
            self.root.clone(),
            self.cache.clone(),
            resolved,
            Mutation::Write(content.to_owned()),
            context,
        )
        .await
    }
}

/// `edit_file` — replace the first occurrence of `old` with `new` in a file.
/// Mirrors Claude Code's Edit: exact, unique-ish string replacement.
pub struct EditFileTool {
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl EditFileTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self::with_cache(root, new_shared_cache())
    }

    /// Build over a workspace root with a shared per-run cache.
    pub(crate) fn with_cache(root: WorkspaceRoot, cache: SharedFileStateCache) -> Self {
        Self { root, cache }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "edit_file".into(),
            description:
                "Replace an exact string in a file. Args: { path, old, new, replace_all? }.\n\
                - INVARIANT: read_file must have delivered the changed ranges of the current file version. External changes cause a conflict; read again and regenerate the edit. Successful writes invalidate prior read authority.\n\
                - `old` must match exactly (whitespace, indentation). Do NOT include the cat -n line-number prefix from read_file output.\n\
                - Pick the smallest unique `old` (2–4 surrounding lines is usually enough). If `old` matches multiple times, pass `replace_all: true` (typical for variable renames) or extend `old` with more context.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old": { "type": "string" },
                    "new": { "type": "string" },
                    "replace_all": {
                        "type": "boolean",
                        "default": false,
                        "description": "Replace ALL occurrences (true) or require old to be unique (false). Default false."
                    }
                },
                "required": ["path", "old", "new"]
            }),
            risk: RiskLevel::Low,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        self.invoke_with_context(args, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
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

        let resolved = match self.root.resolve_write(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        file_mutation::apply(
            self.root.clone(),
            self.cache.clone(),
            resolved,
            Mutation::Edit {
                edits: vec![Replacement {
                    old: old.to_owned(),
                    new: new.to_owned(),
                    all: replace_all,
                }],
                multi: false,
            },
            context,
        )
        .await
    }
}

/// `multi_edit` — apply several ordered exact-string replacements to one file
/// **atomically**: all edits succeed or none are written. Mirrors Claude Code's
/// `MultiEdit`. Edits apply in sequence (each to the result of the previous),
/// so later edits can target text introduced by earlier ones.
pub struct MultiEditTool {
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl MultiEditTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self::with_cache(root, new_shared_cache())
    }

    /// Build over a workspace root with a shared per-run cache.
    pub(crate) fn with_cache(root: WorkspaceRoot, cache: SharedFileStateCache) -> Self {
        Self { root, cache }
    }
}

#[async_trait]
impl Tool for MultiEditTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "multi_edit".into(),
            description: "Apply an ordered list of exact-string replacements to a single file \
                atomically (all-or-nothing). Args: { path, edits: [{ old, new, replace_all? }] }.\n\
                - INVARIANT: read_file must cover every changed range of the current version; changes require re-reading. Successful writes invalidate prior read authority.\n\
                - Each edit's `old` matches against the result of all preceding edits — chain edits when later ones depend on earlier rewrites.\n\
                - If any edit fails (string not found / ambiguous match without replace_all) NOTHING is written."
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
                                "replace_all": {
                                    "type": "boolean",
                                    "default": false
                                }
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
        self.invoke_with_context(args, ToolExecutionContext::default())
            .await
    }

    async fn invoke_with_context(
        &self,
        args: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput> {
        let Some(path) = arg_str(&args, "path") else {
            return Ok(ToolOutput::failure("missing 'path'"));
        };
        let Some(edits) = args.get("edits").and_then(|v| v.as_array()) else {
            return Ok(ToolOutput::failure("missing 'edits' array"));
        };
        if edits.is_empty() {
            return Ok(ToolOutput::failure("'edits' must not be empty"));
        }

        let resolved = match self.root.resolve_write(path) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        let mut replacements = Vec::with_capacity(edits.len());
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
            replacements.push(Replacement {
                old: old.to_owned(),
                new: new.to_owned(),
                all: replace_all,
            });
        }
        file_mutation::apply(
            self.root.clone(),
            self.cache.clone(),
            resolved,
            Mutation::Edit {
                edits: replacements,
                multi: true,
            },
            context,
        )
        .await
    }
}

/// `delete_path` - delete one file or directory without relying on shell syntax.
pub struct DeletePathTool {
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl DeletePathTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self::with_cache(root, new_shared_cache())
    }

    pub(crate) fn with_cache(root: WorkspaceRoot, cache: SharedFileStateCache) -> Self {
        Self { root, cache }
    }
}

#[async_trait]
impl Tool for DeletePathTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "delete_path".into(),
            description: "Delete one file or directory using native filesystem APIs. Args: { path, recursive? }. Prefer this tool over shell rm/del/Remove-Item. Directories require recursive=true. The workspace root itself can never be deleted. Success is returned only after the path is verified absent.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "recursive": { "type": "boolean", "default": false }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let Some(path) = arg_str(&args, "path") else {
            return Ok(ToolOutput::failure("missing 'path'"));
        };
        let recursive = args
            .get("recursive")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let resolved = match self.root.resolve_write(path) {
            Ok(path) => path,
            Err(error) => return Ok(ToolOutput::failure(error.to_string())),
        };
        if resolved == self.root.path() {
            return Ok(ToolOutput::failure(
                "deleting the workspace root is forbidden",
            ));
        }
        let root = self.root.clone();
        let operation_path = resolved.clone();
        let output =
            tokio::task::spawn_blocking(move || delete_entry(&root, &operation_path, recursive))
                .await
                .unwrap_or_else(|error| {
                    ToolOutput::failure(format!("delete worker failed: {error}"))
                });
        if output.ok {
            cache_invalidate(&self.cache, &resolved);
        }
        Ok(output)
    }
}

fn delete_entry(root: &WorkspaceRoot, resolved: &Path, recursive: bool) -> ToolOutput {
    let target = match root.entry_target(resolved, false) {
        Ok(target) => target,
        Err(error) => return ToolOutput::failure(format!("delete authorization failed: {error}")),
    };
    let physical_root = match root.read_identity(root.path()) {
        Ok(path) => path,
        Err(error) => return ToolOutput::failure(format!("workspace identity failed: {error}")),
    };
    if target.path() == physical_root {
        return ToolOutput::failure("deleting the workspace root is forbidden");
    }
    let is_directory = match target.delete_entry(recursive) {
        Ok(is_directory) => is_directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ToolOutput::failure(format!(
                "delete target does not exist: {}",
                root.relativize(resolved)
            ))
        }
        Err(error) => return ToolOutput::failure(format!("delete failed: {error}")),
    };
    let kind = if is_directory { "directory" } else { "file" };
    if std::fs::symlink_metadata(target.path()).is_ok() {
        return ToolOutput::failure("delete returned successfully but the target still exists");
    }
    ToolOutput::success(serde_json::json!({
        "path": root.relativize(resolved),
        "deleted": true,
        "kind": kind,
        "verified_absent": true,
    }))
}

/// `move_path` - rename or move one path without shell-specific commands.
pub struct MovePathTool {
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
}

impl MovePathTool {
    /// Build over a workspace root.
    pub fn new(root: WorkspaceRoot) -> Self {
        Self::with_cache(root, new_shared_cache())
    }

    pub(crate) fn with_cache(root: WorkspaceRoot, cache: SharedFileStateCache) -> Self {
        Self { root, cache }
    }
}

#[async_trait]
impl Tool for MovePathTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "move_path".into(),
            description: "Rename or move a file/directory using native filesystem APIs. Args: { source, destination, overwrite? }. Prefer this over shell mv/move/Rename-Item. Source and destination cannot contain each other. Existing targets require overwrite=true and are retained until publication succeeds; failed rollback reports a recovery_path. Cross-volume copy is not supported. Success is returned only after source absence and destination existence are verified.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "source": { "type": "string" },
                    "destination": { "type": "string" },
                    "overwrite": { "type": "boolean", "default": false }
                },
                "required": ["source", "destination"],
                "additionalProperties": false
            }),
            risk: RiskLevel::High,
            required_permissions: PermissionSet::from_iter_perms([Permission::WorkspaceWrite]),
        }
    }

    async fn invoke(&self, args: serde_json::Value) -> Result<ToolOutput> {
        let (Some(source), Some(destination)) =
            (arg_str(&args, "source"), arg_str(&args, "destination"))
        else {
            return Ok(ToolOutput::failure("missing 'source' or 'destination'"));
        };
        let overwrite = args
            .get("overwrite")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let source = match self.root.resolve_write(source) {
            Ok(path) => path,
            Err(error) => return Ok(ToolOutput::failure(error.to_string())),
        };
        let destination = match self.root.resolve_write(destination) {
            Ok(path) => path,
            Err(error) => return Ok(ToolOutput::failure(error.to_string())),
        };
        let root = self.root.clone();
        let move_source = source.clone();
        let move_destination = destination.clone();
        // Keep staging, publication, and rollback in one non-abortable local
        // operation: dropping an async wait must not drop a data-owning TempDir
        // between two filesystem awaits. This does not promise remote cancel
        // or crash atomicity; the native commit reports its actual result.
        let output = tokio::task::spawn_blocking(move || {
            crate::file_move::move_path(&root, &move_source, &move_destination, overwrite)
        })
        .await
        .unwrap_or_else(|error| {
            let mut output = ToolOutput::failure(format!(
                "move worker failed; inspect source and destination before retrying: {error}"
            ));
            output.value["outcome_unknown"] = true.into();
            output
        });
        // Invalidate even on failed rollback: a retained backup or concurrent
        // target means the pre-operation cache cannot be treated as current.
        cache_invalidate(&self.cache, &source);
        cache_invalidate(&self.cache, &destination);
        Ok(output)
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
        let resolved = match self.root.resolve_read(rel) {
            Ok(p) => p,
            Err(e) => return Ok(ToolOutput::failure(e.to_string())),
        };
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let snapshot = match root.read_directory(&resolved) {
                Ok(snapshot) => snapshot,
                Err(error) => return ToolOutput::failure(format!("read_dir failed: {error}")),
            };
            let mut entries: Vec<_> = snapshot
                .entries
                .into_iter()
                // Discovery uses the same lexical/physical read authorization
                // as recursive glob/grep, before exposing each candidate name.
                .filter(|entry| root.read_identity(&resolved.join(&entry.name)).is_ok())
                .map(|entry| {
                    serde_json::json!({
                        "name": entry.name.to_string_lossy(),
                        "dir": entry.is_dir && !entry.is_link,
                    })
                })
                .collect();
            entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            ToolOutput::success(serde_json::json!({
                "path": root.relativize(&resolved),
                "entries": entries,
            }))
        })
        .await
        .map_err(|error| {
            deepagent_core::error::CoreError::other(format!("directory read join failed: {error}"))
        })
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
        let pattern = pattern.to_owned();
        let root = self.root.clone();
        let max_results = self.max_results;
        tokio::task::spawn_blocking(move || {
            let mut matches = Vec::new();
            if let Err(error) = walk_collect(&root, &pattern, &mut matches, max_results) {
                return ToolOutput::failure(format!("glob traversal failed: {error}"));
            }
            matches.sort();
            ToolOutput::success(serde_json::json!({
                "pattern": pattern,
                "matches": matches,
                "truncated": matches.len() >= max_results,
            }))
        })
        .await
        .map_err(|error| {
            deepagent_core::error::CoreError::other(format!("glob traversal join failed: {error}"))
        })
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
                "Search file contents for a literal substring. Args: { query, glob? (default **) }. For code symbols, prefer codegraph_search first; use grep for comments, config text, non-symbol literals, or when codegraph is unavailable."
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
        let pattern = arg_str(&args, "glob").unwrap_or("**").to_owned();
        let root = self.root.clone();
        let files = tokio::task::spawn_blocking(move || {
            let mut files = Vec::new();
            walk_collect(&root, &pattern, &mut files, 5000)?;
            Ok::<_, std::io::Error>(files)
        })
        .await
        .map_err(|error| {
            deepagent_core::error::CoreError::other(format!("grep traversal join failed: {error}"))
        })?;
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                return Ok(ToolOutput::failure(format!(
                    "grep traversal failed: {error}"
                )))
            }
        };
        let mut hits = Vec::new();
        for rel in &files {
            if hits.len() >= self.max_results {
                break;
            }
            let root = self.root.clone();
            let path = root.path().join(rel);
            // Keep the established per-file async boundary: abandoning grep
            // must not leave a worker reading all remaining file contents.
            let content = tokio::task::spawn_blocking(move || {
                let mut file = root.open_read(&path)?;
                let mut content = String::new();
                file.read_to_string(&mut content)?;
                Ok::<_, std::io::Error>(content)
            })
            .await
            .map_err(|error| {
                deepagent_core::error::CoreError::other(format!("grep read join failed: {error}"))
            })?;
            // Preserve the established skip-unreadable/non-UTF8 policy.
            let Ok(content) = content else { continue };
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

/// Depth-first discovery using the same physical authorization and directory
/// handles as explicit reads. Track only active ancestry so normal aliases are
/// still visible, but junction loops cannot recurse forever. Use an explicit
/// stack rather than the Rust call stack for deep workspace trees.
fn walk_collect(
    root: &WorkspaceRoot,
    pattern: &str,
    out: &mut Vec<String>,
    limit: usize,
) -> std::io::Result<()> {
    enum Step {
        Visit(std::path::PathBuf, bool, bool),
        Leave(std::path::PathBuf),
    }
    let mut pending = vec![Step::Visit(root.path().to_owned(), true, false)];
    let mut ancestry = HashSet::new();
    while let Some(step) = pending.pop() {
        if out.len() >= limit {
            break;
        }
        let (path, is_dir, is_link) = match step {
            Step::Leave(identity) => {
                ancestry.remove(&identity);
                continue;
            }
            Step::Visit(path, is_dir, is_link) => (path, is_dir, is_link),
        };
        let is_root = path == root.path();
        let physical = match root.read_identity(&path) {
            Ok(physical) => physical,
            Err(error) if is_root => return Err(error),
            Err(_) => continue,
        };
        let is_dir =
            is_dir || (is_link && std::fs::metadata(&physical).is_ok_and(|meta| meta.is_dir()));
        if is_dir {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if !is_root && (SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.')) {
                continue;
            }
            let mut snapshot = match root.read_directory(&path) {
                Ok(snapshot) => snapshot,
                Err(error) if is_root => return Err(error),
                Err(_) => continue,
            };
            if !ancestry.insert(snapshot.identity.clone()) {
                continue;
            }
            pending.push(Step::Leave(snapshot.identity));
            snapshot.entries.sort_by(|a, b| a.name.cmp(&b.name));
            for entry in snapshot.entries.into_iter().rev() {
                pending.push(Step::Visit(
                    path.join(entry.name),
                    entry.is_dir,
                    entry.is_link,
                ));
            }
        } else {
            let rel = root.relativize(&path);
            if glob_match(pattern, &rel) {
                out.push(rel);
            }
        }
    }
    Ok(())
}

/// Register all file tools into a registry over the given root (helper).
pub fn file_tools(root: WorkspaceRoot) -> Vec<Arc<dyn Tool>> {
    let cache = new_shared_cache();
    vec![
        Arc::new(ReadFileTool::with_cache(root.clone(), cache.clone())),
        Arc::new(WriteFileTool::with_cache(root.clone(), cache.clone())),
        Arc::new(EditFileTool::with_cache(root.clone(), cache.clone())),
        Arc::new(MultiEditTool::with_cache(root.clone(), cache.clone())),
        Arc::new(DeletePathTool::with_cache(root.clone(), cache.clone())),
        Arc::new(MovePathTool::with_cache(root.clone(), cache.clone())),
        Arc::new(ListDirTool::new(root.clone())),
        Arc::new(GlobTool::new(root.clone())),
        Arc::new(GrepTool::new(root)),
    ]
}

/// `read_file` + `edit_file` over one shared cache, for a run that may touch
/// exactly one file (session-note extraction).
///
/// The pair has to be built together: `edit_file` enforces read-before-edit
/// through the shared cache, so a run given only `edit_file` could never edit
/// legally, and separate caches would make the read invisible to the edit.
pub fn note_edit_tools(root: WorkspaceRoot) -> Vec<Arc<dyn Tool>> {
    let cache = new_shared_cache();
    vec![
        Arc::new(ReadFileTool::with_cache(root.clone(), cache.clone())),
        Arc::new(EditFileTool::with_cache(root, cache)),
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

    #[test]
    fn format_with_line_numbers_pads_to_six_chars() {
        // Single-digit, three-digit, and a deliberately empty line all get the
        // 6-wide right-aligned line number plus a TAB separator.
        let lines: Vec<&str> = vec!["alpha", "", "beta"];
        let rendered = format_with_line_numbers(&lines, 1);
        assert_eq!(rendered, "     1\talpha\n     2\t\n     3\tbeta");
    }

    #[test]
    fn format_with_line_numbers_uses_offset_for_first_line_number() {
        // When the caller is rendering an offset window starting at line 100,
        // the first emitted line carries 100, not 1.
        let lines: Vec<&str> = vec!["x", "y"];
        let rendered = format_with_line_numbers(&lines, 100);
        assert_eq!(rendered, "   100\tx\n   101\ty");
    }

    #[test]
    fn format_with_line_numbers_does_not_clip_long_lines() {
        // The format prefixes a header but never modifies the line content
        // itself, even when the line is far longer than 80 chars.
        let long_line = "x".repeat(500);
        let rendered = format_with_line_numbers(std::slice::from_ref(&long_line), 7);
        assert_eq!(rendered, format!("     7\t{}", long_line));
    }

    #[tokio::test]
    async fn write_then_read_roundtrip() {
        let (_d, root) = temp_root();
        let w = WriteFileTool::new(root.clone());
        let out = w
            .invoke(serde_json::json!({"path": "src/a.txt", "content": "hello"}))
            .await
            .unwrap();
        assert!(out.ok, "{out:?}");

        let r = ReadFileTool::new(root.clone());
        let out = r
            .invoke(serde_json::json!({"path": "src/a.txt"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["content"], "     1\thello");
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
    async fn read_large_file_defaults_to_first_2000_lines() {
        let (_d, root) = temp_root();
        let content = (1..=3000)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "large.txt", "content": content}))
            .await
            .unwrap();

        let out = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "large.txt"}))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["next_offset"], 2001);
        assert_eq!(out.value["truncated"], true);
        assert_eq!(out.value["content"].as_str().unwrap().lines().count(), 2000);
        // Each rendered line carries the cat -n prefix; the last line in the
        // default window should appear with its line number 2000.
        assert!(out.value["content"]
            .as_str()
            .unwrap()
            .contains("  2000\tline 2000"));
        assert!(out.value["message"]
            .as_str()
            .unwrap()
            .contains("output truncated"));
    }

    #[tokio::test]
    async fn read_offset_and_limit_returns_requested_window() {
        let (_d, root) = temp_root();
        let content = (1..=250)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "window.txt", "content": content}))
            .await
            .unwrap();

        let out = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({
                "path": "window.txt",
                "offset": 101,
                "limit": 5
            }))
            .await
            .unwrap();

        assert!(out.ok);
        assert_eq!(out.value["lines"], 250);
        assert_eq!(out.value["offset"], 101);
        assert_eq!(out.value["limit"], 5);
        assert_eq!(out.value["returned_lines"], 5);
        assert_eq!(out.value["next_offset"], 106);
        assert_eq!(
            out.value["content"],
            "   101\tline 101\n   102\tline 102\n   103\tline 103\n   104\tline 104\n   105\tline 105"
        );
    }

    #[tokio::test]
    async fn read_small_file_returns_full_content() {
        let (_d, root) = temp_root();
        WriteFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "small.txt", "content": "a\nb\nc"}))
            .await
            .unwrap();

        let out = ReadFileTool::new(root.clone())
            .invoke(serde_json::json!({"path": "small.txt"}))
            .await
            .unwrap();

        assert!(out.ok);
        assert_eq!(out.value["content"], "     1\ta\n     2\tb\n     3\tc");
        assert_eq!(out.value["returned_lines"], 3);
        assert_eq!(out.value["truncated"], false);
        assert!(out.value["next_offset"].is_null());
    }

    #[tokio::test]
    async fn repeated_read_uses_shared_cache() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "cached.txt", "content": "first\nsecond"}))
            .await
            .unwrap();
        let r = ReadFileTool::with_cache(root.clone(), cache);

        let first = r
            .invoke(serde_json::json!({"path": "cached.txt"}))
            .await
            .unwrap();
        let second = r
            .invoke(serde_json::json!({"path": "cached.txt"}))
            .await
            .unwrap();

        assert!(first.ok);
        assert!(second.ok);
        assert_eq!(first.value["cache_hit"], false);
        // Second read with same args hits the cache *and* triggers the
        // FILE_UNCHANGED_STUB path: cache_hit is true, content is the stub.
        assert_eq!(second.value["cache_hit"], true);
        assert_eq!(second.value["unchanged_stub"], true);
        assert_eq!(first.value["content_hash"], second.value["content_hash"]);
    }

    #[tokio::test]
    async fn repeat_read_returns_file_unchanged_stub() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "stub.txt", "content": "alpha\nbeta\ngamma"}))
            .await
            .unwrap();
        let r = ReadFileTool::with_cache(root.clone(), cache);

        let first = r
            .invoke(serde_json::json!({"path": "stub.txt"}))
            .await
            .unwrap();
        assert!(first.ok);
        assert_eq!(first.value["unchanged_stub"], serde_json::Value::Null);
        let body = first.value["content"].as_str().unwrap();
        assert!(body.contains("alpha"));

        let second = r
            .invoke(serde_json::json!({"path": "stub.txt"}))
            .await
            .unwrap();
        assert!(second.ok);
        assert_eq!(second.value["unchanged_stub"], true);
        assert_eq!(second.value["returned_lines"], 0);
        let stub_body = second.value["content"].as_str().unwrap();
        assert!(stub_body.starts_with("<FILE_UNCHANGED_STUB"));
        // The file's line count is still reported so the model knows the size.
        assert_eq!(second.value["lines"], 3);
        // No content_hash mutation across stubbed reads.
        assert_eq!(first.value["content_hash"], second.value["content_hash"]);
    }

    #[tokio::test]
    async fn repeat_read_with_different_offset_skips_stub() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({
                "path": "win.txt",
                "content": "a\nb\nc\nd\ne",
            }))
            .await
            .unwrap();
        let r = ReadFileTool::with_cache(root.clone(), cache);

        let first = r
            .invoke(serde_json::json!({"path": "win.txt", "offset": 1, "limit": 2}))
            .await
            .unwrap();
        let second_diff_offset = r
            .invoke(serde_json::json!({"path": "win.txt", "offset": 3, "limit": 2}))
            .await
            .unwrap();
        let third_same_as_first = r
            .invoke(serde_json::json!({"path": "win.txt", "offset": 1, "limit": 2}))
            .await
            .unwrap();

        // first and second use different windows → both return content.
        assert_eq!(first.value["unchanged_stub"], serde_json::Value::Null);
        assert_eq!(
            second_diff_offset.value["unchanged_stub"],
            serde_json::Value::Null
        );
        // third repeats first's window — but second already overwrote the
        // recorded last_read with (3, 2), so third does NOT match and returns
        // content again.
        assert_eq!(
            third_same_as_first.value["unchanged_stub"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn repeat_read_returns_content_after_write() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        let w = WriteFileTool::with_cache(root.clone(), cache.clone());
        let r = ReadFileTool::with_cache(root.clone(), cache);

        w.invoke(serde_json::json!({"path": "live.txt", "content": "old"}))
            .await
            .unwrap();
        let _first = r
            .invoke(serde_json::json!({"path": "live.txt"}))
            .await
            .unwrap();
        let second_stub = r
            .invoke(serde_json::json!({"path": "live.txt"}))
            .await
            .unwrap();
        assert_eq!(second_stub.value["unchanged_stub"], true);

        // Write invalidates the cache → next read returns full content again.
        w.invoke(serde_json::json!({"path": "live.txt", "content": "new"}))
            .await
            .unwrap();
        let after_write = r
            .invoke(serde_json::json!({"path": "live.txt"}))
            .await
            .unwrap();
        assert_eq!(after_write.value["unchanged_stub"], serde_json::Value::Null);
        assert_eq!(after_write.value["content"], "     1\tnew");
    }

    #[tokio::test]
    async fn first_read_never_returns_stub() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "f.txt", "content": "x\ny"}))
            .await
            .unwrap();
        let r = ReadFileTool::with_cache(root.clone(), cache);
        let out = r
            .invoke(serde_json::json!({"path": "f.txt"}))
            .await
            .unwrap();
        assert_eq!(out.value["unchanged_stub"], serde_json::Value::Null);
        assert!(out.value["content"].as_str().unwrap().contains("x"));
    }

    #[tokio::test]
    async fn write_invalidates_shared_read_cache() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        let w = WriteFileTool::with_cache(root.clone(), cache.clone());
        let r = ReadFileTool::with_cache(root.clone(), cache);

        w.invoke(serde_json::json!({"path": "cached.txt", "content": "old"}))
            .await
            .unwrap();
        r.invoke(serde_json::json!({"path": "cached.txt"}))
            .await
            .unwrap();
        let cached = r
            .invoke(serde_json::json!({"path": "cached.txt"}))
            .await
            .unwrap();
        assert_eq!(cached.value["cache_hit"], true);

        w.invoke(serde_json::json!({"path": "cached.txt", "content": "new"}))
            .await
            .unwrap();
        let fresh = r
            .invoke(serde_json::json!({"path": "cached.txt"}))
            .await
            .unwrap();
        assert_eq!(fresh.value["cache_hit"], false);
        assert_eq!(fresh.value["content"], "     1\tnew");
    }

    #[tokio::test]
    async fn edit_replaces_unique_string() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "foo bar baz"}))
            .await
            .unwrap();
        let r = ReadFileTool::with_cache(root.clone(), cache.clone());
        r.invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache.clone());
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "bar", "new": "QUX"}))
            .await
            .unwrap();
        assert!(out.ok);
        // Edit invalidates the cache → read again to satisfy invariant for
        // subsequent reads (here we just verify content from disk via tool).
        let content = ReadFileTool::with_cache(root.clone(), cache)
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        assert_eq!(content.value["content"], "     1\tfoo QUX baz");
    }

    #[tokio::test]
    async fn edit_matches_lf_old_against_crlf_file() {
        let (dir, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({
                "path": "application.yml",
                "content": "logging:\r\n  level:\r\n    root: info\r\n"
            }))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "application.yml"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache);
        let out = e
            .invoke(serde_json::json!({
                "path": "application.yml",
                "old": "logging:\n  level:\n    root: info",
                "new": "logging:\n  level:\n    root: debug"
            }))
            .await
            .unwrap();
        assert!(out.ok);
        let content = tokio::fs::read_to_string(dir.path().join("application.yml"))
            .await
            .unwrap();
        assert_eq!(content, "logging:\r\n  level:\r\n    root: debug\r\n");
    }

    #[tokio::test]
    async fn edit_ambiguous_match_requires_replace_all() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "a a a"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache.clone());
        // Without replace_all, 3 matches => failure.
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "a", "new": "b"}))
            .await
            .unwrap();
        assert!(!out.ok);
        // With replace_all, succeeds. (Edit invalidates the cache; read again
        // to satisfy the invariant for the second edit call.)
        ReadFileTool::with_cache(root.clone(), cache)
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
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
    async fn grep_sensitive_policy_matches_direct_reads_in_every_access_mode() {
        use crate::fs_guard::FsAccess;

        let (_dir, root) = temp_root();
        let protected_paths = [
            ".env",
            "credentials",
            "certs/server.key",
            "nested/secret-store/data.txt",
            "nested/.ENV.Local",
        ];
        for path in protected_paths {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "protected policy fixture").unwrap();
        }
        std::fs::write(root.path().join("safe.txt"), "public policy fixture").unwrap();
        for access in [
            FsAccess::ReadOnly,
            FsAccess::Workspace,
            FsAccess::ReadAnywhere,
            FsAccess::Full,
        ] {
            let root = root.clone().with_access(access);
            let reader = ReadFileTool::new(root.clone());
            for path in protected_paths {
                let read = reader
                    .invoke(serde_json::json!({"path":path}))
                    .await
                    .unwrap();
                assert!(!read.ok, "{access:?}/{path}: {read:?}");
            }
            let grep = GrepTool::new(root);
            let output = grep
                .invoke(serde_json::json!({"query":"policy fixture"}))
                .await
                .unwrap();
            assert!(output.ok, "{access:?}: {output:?}");
            let hits = output.value["hits"].as_array().unwrap();
            assert_eq!(hits.len(), 1, "{access:?}: {output:?}");
            assert_eq!(hits[0]["file"], "safe.txt");
            assert_eq!(hits[0]["text"], "public policy fixture");
            let explicit = grep
                .invoke(serde_json::json!({"query":"policy fixture", "glob":"**/*.key"}))
                .await
                .unwrap();
            assert!(explicit.ok, "{access:?}: {explicit:?}");
            assert!(explicit.value["hits"].as_array().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn list_dir_sensitive_policy_matches_direct_reads_in_every_access_mode() {
        use crate::fs_guard::FsAccess;

        let (_dir, root) = temp_root();
        let protected = [
            ".env",
            "credentials",
            ".ENV.Local",
            "secret-store/data.txt",
            "certs/server.key",
        ];
        for path in protected {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "synthetic policy fixture").unwrap();
        }
        for path in [".hidden", "visible.txt", "certs/public.txt"] {
            std::fs::write(root.path().join(path), "ordinary fixture").unwrap();
        }
        std::fs::create_dir(root.path().join("empty")).unwrap();
        for access in [
            FsAccess::ReadOnly,
            FsAccess::Workspace,
            FsAccess::ReadAnywhere,
            FsAccess::Full,
        ] {
            let root = root.clone().with_access(access);
            let reader = ReadFileTool::new(root.clone());
            for path in protected {
                let read = reader
                    .invoke(serde_json::json!({"path":path}))
                    .await
                    .unwrap();
                assert!(!read.ok, "{access:?}/{path}: {read:?}");
            }
            let listing = ListDirTool::new(root);
            let output = listing.invoke(serde_json::json!({})).await.unwrap();
            assert!(output.ok, "{access:?}: {output:?}");
            assert_eq!(
                output.value["entries"],
                serde_json::json!([
                    {"name":".hidden", "dir":false},
                    {"name":"certs", "dir":true},
                    {"name":"empty", "dir":true},
                    {"name":"visible.txt", "dir":false},
                ]),
                "{access:?}"
            );
            let nested = listing
                .invoke(serde_json::json!({"path":"certs"}))
                .await
                .unwrap();
            assert!(nested.ok, "{access:?}: {nested:?}");
            assert_eq!(
                nested.value["entries"],
                serde_json::json!([{"name":"public.txt", "dir":false}])
            );
            let denied = listing
                .invoke(serde_json::json!({"path":"secret-store"}))
                .await
                .unwrap();
            assert!(!denied.ok, "{access:?}: {denied:?}");
            let empty = listing
                .invoke(serde_json::json!({"path":"empty"}))
                .await
                .unwrap();
            assert!(empty.ok);
            assert_eq!(empty.value["entries"], serde_json::json!([]));
        }
    }

    #[cfg(any(windows, unix))]
    #[tokio::test]
    async fn list_dir_authorizes_link_targets_before_returning_names() {
        use crate::fs_guard::FsAccess;

        let (_dir, root) = temp_root();
        let outside = tempfile::tempdir().unwrap();
        for name in ["real", "secret-store"] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        for (name, target) in [
            ("inward", root.path().join("real")),
            ("outward", outside.path().to_path_buf()),
            ("alias", root.path().join("secret-store")),
        ] {
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(target, root.path().join(name)).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, root.path().join(name)).unwrap();
        }
        for access in [
            FsAccess::ReadOnly,
            FsAccess::Workspace,
            FsAccess::ReadAnywhere,
            FsAccess::Full,
        ] {
            let root = root.clone().with_access(access);
            let external_allowed = matches!(access, FsAccess::ReadAnywhere | FsAccess::Full);
            assert!(root.resolve_read("alias").is_err());
            assert_eq!(root.resolve_read("outward").is_ok(), external_allowed);
            let output = ListDirTool::new(root)
                .invoke(serde_json::json!({}))
                .await
                .unwrap();
            assert!(output.ok, "{access:?}: {output:?}");
            let mut expected = vec![serde_json::json!({"name":"inward", "dir":false})];
            if external_allowed {
                expected.push(serde_json::json!({"name":"outward", "dir":false}));
            }
            expected.push(serde_json::json!({"name":"real", "dir":true}));
            assert_eq!(
                output.value["entries"],
                serde_json::json!(expected),
                "{access:?}"
            );
        }
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

    #[tokio::test]
    async fn discovery_preserves_sorted_depth_first_limits_and_reports_invalid_root() {
        let (_d, root) = temp_root();
        std::fs::create_dir(root.path().join("a")).unwrap();
        for path in ["b.txt", "a/z.txt", "a/a.txt"] {
            std::fs::write(root.path().join(path), "MATCH fixture").unwrap();
        }
        for directory in [".hidden", "target", "node_modules"] {
            std::fs::create_dir(root.path().join(directory)).unwrap();
            std::fs::write(
                root.path().join(directory).join("hidden.txt"),
                "MATCH fixture",
            )
            .unwrap();
        }
        let glob = GlobTool {
            root: root.clone(),
            max_results: 2,
        }
        .invoke(serde_json::json!({"pattern": "**/*.txt"}))
        .await
        .unwrap();
        assert!(glob.ok);
        assert_eq!(
            glob.value["matches"],
            serde_json::json!(["a/a.txt", "a/z.txt"])
        );
        assert_eq!(glob.value["truncated"], true);
        let grep = GrepTool {
            root: root.clone(),
            max_results: 2,
        }
        .invoke(serde_json::json!({"query": "MATCH"}))
        .await
        .unwrap();
        assert!(grep.ok);
        assert_eq!(grep.value["hits"][0]["file"], "a/a.txt");
        assert_eq!(grep.value["hits"][1]["file"], "a/z.txt");
        assert_eq!(grep.value["truncated"], true);
        let invalid = WorkspaceRoot::new(root.path().join("b.txt"));
        assert!(
            !GlobTool::new(invalid.clone())
                .invoke(serde_json::json!({"pattern":"**"}))
                .await
                .unwrap()
                .ok
        );
        assert!(
            !GrepTool::new(invalid)
                .invoke(serde_json::json!({"query":"MATCH"}))
                .await
                .unwrap()
                .ok
        );
    }

    #[test]
    fn file_tools_helper_returns_all() {
        let (_d, root) = temp_root();
        let names = file_tools(root)
            .into_iter()
            .map(|tool| tool.descriptor().name)
            .collect::<Vec<_>>();
        assert_eq!(names.len(), 9);
        assert!(names.contains(&"delete_path".to_string()));
        assert!(names.contains(&"move_path".to_string()));
    }

    #[tokio::test]
    async fn multi_edit_applies_all_atomically() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "alpha beta gamma"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let m = MultiEditTool::with_cache(root.clone(), cache.clone());
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
        let content = ReadFileTool::with_cache(root.clone(), cache)
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        assert_eq!(content.value["content"], "     1\tONE beta THREE");
    }

    #[tokio::test]
    async fn multi_edit_is_atomic_on_failure() {
        let (dir, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "keep this"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let m = MultiEditTool::with_cache(root.clone(), cache);
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
        // Verify the file on disk directly — atomic failure means the partial
        // first edit was NOT persisted. We read from the filesystem to bypass
        // the FILE_UNCHANGED_STUB path that would shadow content on a repeat
        // read with identical args.
        let on_disk = tokio::fs::read_to_string(dir.path().join("x.txt"))
            .await
            .unwrap();
        assert_eq!(on_disk, "keep this");
    }

    #[tokio::test]
    async fn multi_edit_sequential_edits_see_prior_results() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "a"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let m = MultiEditTool::with_cache(root.clone(), cache.clone());
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
        let content = ReadFileTool::with_cache(root.clone(), cache)
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        assert_eq!(content.value["content"], "     1\tc");
    }

    // ----- Read-before-edit invariant (Phase 2C) -----

    #[tokio::test]
    async fn edit_without_prior_read_fails() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        // Set up a file directly on disk; no read_file call has happened.
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "hi"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache);
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "hi", "new": "yo"}))
            .await
            .unwrap();
        assert!(!out.ok);
        let err = out.value["error"].as_str().unwrap();
        assert!(err.contains("read_file"), "got: {err}");
    }

    #[tokio::test]
    async fn edit_after_read_succeeds() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "hi"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache);
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "hi", "new": "yo"}))
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn multi_edit_without_prior_read_fails() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "a b"}))
            .await
            .unwrap();
        let m = MultiEditTool::with_cache(root.clone(), cache);
        let out = m
            .invoke(serde_json::json!({
                "path": "x.txt",
                "edits": [{"old": "a", "new": "A"}],
            }))
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.value["error"].as_str().unwrap().contains("read_file"));
    }

    #[tokio::test]
    async fn write_to_new_path_does_not_require_read() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        let w = WriteFileTool::with_cache(root.clone(), cache);
        let out = w
            .invoke(serde_json::json!({"path": "fresh.txt", "content": "hi"}))
            .await
            .unwrap();
        // Brand-new file: no prior read needed.
        assert!(out.ok);
    }

    #[tokio::test]
    async fn write_overwrite_without_prior_read_fails() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        let w = WriteFileTool::with_cache(root.clone(), cache.clone());
        // First write creates the file (allowed).
        w.invoke(serde_json::json!({"path": "x.txt", "content": "v1"}))
            .await
            .unwrap();
        // Second write would CLOBBER existing content — must read first.
        let out = w
            .invoke(serde_json::json!({"path": "x.txt", "content": "v2"}))
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.value["error"].as_str().unwrap().contains("read_file"));
    }

    #[tokio::test]
    async fn write_overwrite_after_read_succeeds() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        let w = WriteFileTool::with_cache(root.clone(), cache.clone());
        let r = ReadFileTool::with_cache(root.clone(), cache);
        w.invoke(serde_json::json!({"path": "x.txt", "content": "v1"}))
            .await
            .unwrap();
        r.invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let out = w
            .invoke(serde_json::json!({"path": "x.txt", "content": "v2"}))
            .await
            .unwrap();
        assert!(out.ok);
    }

    #[tokio::test]
    async fn edit_after_write_invalidation_requires_re_read() {
        // Write invalidates the cache → a previous read no longer counts.
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        let w = WriteFileTool::with_cache(root.clone(), cache.clone());
        let r = ReadFileTool::with_cache(root.clone(), cache.clone());
        let e = EditFileTool::with_cache(root.clone(), cache);

        w.invoke(serde_json::json!({"path": "x.txt", "content": "first"}))
            .await
            .unwrap();
        r.invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        // First edit is allowed.
        let ok = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "first", "new": "second"}))
            .await
            .unwrap();
        assert!(ok.ok);
        // Edit invalidated the cache. A second edit without re-reading fails.
        let blocked = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "second", "new": "third"}))
            .await
            .unwrap();
        assert!(!blocked.ok);
        assert!(blocked.value["error"]
            .as_str()
            .unwrap()
            .contains("read_file"));
    }

    // ----- replace_all parameter (Phase 2D) -----

    #[tokio::test]
    async fn edit_with_replace_all_true_on_unique_match_works() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "only one match here"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache);
        let out = e
            .invoke(serde_json::json!({
                "path": "x.txt",
                "old": "match",
                "new": "MATCH",
                "replace_all": true,
            }))
            .await
            .unwrap();
        assert!(out.ok);
        assert_eq!(out.value["replacements"], 1);
    }

    #[tokio::test]
    async fn edit_ambiguous_failure_message_mentions_replace_all_and_count() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt", "content": "x x x x"}))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let e = EditFileTool::with_cache(root.clone(), cache);
        let out = e
            .invoke(serde_json::json!({"path": "x.txt", "old": "x", "new": "y"}))
            .await
            .unwrap();
        assert!(!out.ok);
        let err = out.value["error"].as_str().unwrap();
        // Failure message must surface the count, point to replace_all, and
        // suggest more context — the model uses these three signals to choose
        // between extending old_string or flipping the flag.
        assert!(err.contains("4"), "missing count: {err}");
        assert!(err.contains("replace_all"), "missing flag hint: {err}");
        assert!(
            err.to_lowercase().contains("context"),
            "missing context hint: {err}"
        );
    }

    #[tokio::test]
    async fn multi_edit_per_edit_replace_all_works() {
        let (_d, root) = temp_root();
        let cache = new_shared_cache();
        WriteFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({
                "path": "x.txt",
                "content": "FOO BAR FOO BAR",
            }))
            .await
            .unwrap();
        ReadFileTool::with_cache(root.clone(), cache.clone())
            .invoke(serde_json::json!({"path": "x.txt"}))
            .await
            .unwrap();
        let m = MultiEditTool::with_cache(root.clone(), cache);
        let out = m
            .invoke(serde_json::json!({
                "path": "x.txt",
                "edits": [
                    {"old": "FOO", "new": "foo", "replace_all": true},
                    {"old": "BAR", "new": "bar", "replace_all": true},
                ],
            }))
            .await
            .unwrap();
        assert!(out.ok);
        // 2 + 2 = 4 replacements across the two edits.
        assert_eq!(out.value["replacements"], 4);
    }

    #[tokio::test]
    async fn delete_path_requires_recursive_and_verifies_absence() {
        let (dir, root) = temp_root();
        let target = dir.path().join("remove-me");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("nested.txt"), "x").unwrap();
        let tool = DeletePathTool::new(root);

        let refused = tool
            .invoke(serde_json::json!({"path": "remove-me"}))
            .await
            .unwrap();
        assert!(!refused.ok);
        assert!(target.exists());

        let deleted = tool
            .invoke(serde_json::json!({"path": "remove-me", "recursive": true}))
            .await
            .unwrap();
        assert!(deleted.ok, "{deleted:?}");
        assert_eq!(deleted.value["verified_absent"], true);
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn delete_path_never_deletes_workspace_root() {
        let (dir, root) = temp_root();
        let tool = DeletePathTool::new(root);
        let output = tool
            .invoke(serde_json::json!({"path": ".", "recursive": true}))
            .await
            .unwrap();
        assert!(!output.ok);
        assert!(dir.path().exists());
    }

    #[tokio::test]
    async fn move_path_verifies_both_ends() {
        let (dir, root) = temp_root();
        std::fs::write(dir.path().join("before.txt"), "content").unwrap();
        let tool = MovePathTool::new(root);
        let output = tool
            .invoke(serde_json::json!({
                "source": "before.txt",
                "destination": "nested/after.txt"
            }))
            .await
            .unwrap();

        assert!(output.ok, "{output:?}");
        assert!(!dir.path().join("before.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested/after.txt")).unwrap(),
            "content"
        );
        assert_eq!(output.value["verified"], true);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn move_path_locked_source_restores_existing_destination() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        let (dir, root) = temp_root();
        let source = dir.path().join("locked.txt");
        let destination = dir.path().join("target.txt");
        std::fs::write(&source, "source bytes").unwrap();
        std::fs::write(&destination, "previous bytes").unwrap();
        // Omit FILE_SHARE_DELETE: the real OS refuses publication, after the
        // unrelated target has been staged, exercising the production rollback.
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&source)
            .unwrap();
        let output = MovePathTool::new(root)
            .invoke(serde_json::json!({
                "source": "locked.txt", "destination": "target.txt", "overwrite": true,
            }))
            .await
            .unwrap();
        assert!(!output.ok, "{output:?}");
        assert_eq!(output.value["destination_restored"], true);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "source bytes");
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "previous bytes"
        );
        drop(handle);
    }
}
