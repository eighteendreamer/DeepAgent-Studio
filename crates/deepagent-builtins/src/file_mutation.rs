//! One observed-version/commit boundary for write, edit and multi-edit.

use std::io::{self, Write};
use std::path::Path;

use deepagent_persistence::fs_commit::{lock_path_operation, rename_no_replace};
use deepagent_tools::{ToolExecutionContext, ToolOutput};

use crate::file_cache::{ObservedFile, SharedFileStateCache};
use crate::fs_guard::WorkspaceRoot;

pub(crate) const READ_BEFORE_EDIT_MSG: &str =
    "You must call read_file on this path before editing it.";
pub(crate) const STALE_READ_MSG: &str =
    "FILE_CHANGED_SINCE_READ: file changed or disappeared since read_file; read it again and regenerate the edit. No content was written.";
const UNSEEN_RANGE_MSG: &str =
    "FILE_RANGE_NOT_READ: call read_file for every range being changed; replacing the whole file requires reading the whole file. No content was written.";

pub(crate) struct Replacement {
    pub old: String,
    pub new: String,
    pub all: bool,
}

pub(crate) enum Mutation {
    Write(String),
    Edit {
        edits: Vec<Replacement>,
        multi: bool,
    },
}

pub(crate) async fn apply(
    root: WorkspaceRoot,
    cache: SharedFileStateCache,
    path: std::path::PathBuf,
    mutation: Mutation,
    context: ToolExecutionContext,
) -> deepagent_core::error::Result<ToolOutput> {
    tokio::task::spawn_blocking(move || {
        if context.is_cancelled() {
            return ToolOutput::failure("FILE_OPERATION_CANCELLED: no content was written");
        }
        // Only new-file writes may create parents. Checking an existing read
        // or a stale deletion must not turn metadata errors into "new file".
        if matches!(mutation, Mutation::Write(_))
            && !cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .has_been_read(&path)
        {
            if let Some(parent) = path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    return ToolOutput::failure(format!("mkdir failed: {error}"));
                }
            }
        }
        let _operation = match lock_path_operation(&path) {
            Ok(guard) => guard,
            Err(error) => {
                return ToolOutput::failure(format!("file operation lock failed: {error}"))
            }
        };
        if context.is_cancelled() {
            return ToolOutput::failure("FILE_OPERATION_CANCELLED: no content was written");
        }
        // All cooperating read/mutate paths take the path gate before the
        // cache mutex. This guard spans validation, staging, publication and
        // invalidation with no async yield between them.
        let observed = cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .observed(&path);
        let before = match read_current(&path) {
            Ok(content) => content,
            Err(error) => return ToolOutput::failure(format!("read failed: {error}")),
        };
        if observed
            .as_ref()
            .is_some_and(|seen| before.as_deref() != Some(&seen.content))
        {
            return ToolOutput::failure(STALE_READ_MSG);
        }
        if (before.is_some() || matches!(mutation, Mutation::Edit { .. })) && observed.is_none() {
            return ToolOutput::failure(READ_BEFORE_EDIT_MSG);
        }
        let (content, mut output) = match prepare(mutation, observed) {
            Ok(prepared) => prepared,
            Err(error) => return ToolOutput::failure(error),
        };
        let old_lines = before.as_deref().unwrap_or_default().lines().count();
        let new_lines = content.lines().count();
        match publish_checked(
            &path,
            before.as_deref(),
            content.as_bytes(),
            || {
                if context.is_cancelled() {
                    Err(io::Error::other(
                        "FILE_OPERATION_CANCELLED: no content was written",
                    ))
                } else {
                    Ok(())
                }
            },
            publish,
        ) {
            Ok(()) => {
                cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .invalidate(&path);
                output["path"] = serde_json::json!(root.relativize(&path));
                output["lines_added"] = serde_json::json!(new_lines.saturating_sub(old_lines));
                output["lines_removed"] = serde_json::json!(old_lines.saturating_sub(new_lines));
                ToolOutput::success(output)
            }
            Err(error) => ToolOutput::failure(format!("write failed: {error}")),
        }
    })
    .await
    .map_err(|error| {
        deepagent_core::error::CoreError::other(format!("file mutation join failed: {error}"))
    })
}

fn prepare(
    mutation: Mutation,
    observed: Option<ObservedFile>,
) -> Result<(String, serde_json::Value), String> {
    match mutation {
        Mutation::Write(content) => {
            if observed
                .as_ref()
                .is_some_and(|seen| !seen.covers(&(0..seen.content.len())))
            {
                return Err(UNSEEN_RANGE_MSG.into());
            }
            let bytes = content.len();
            Ok((content, serde_json::json!({"bytes": bytes})))
        }
        Mutation::Edit { edits, multi } => {
            let mut seen = observed.ok_or(READ_BEFORE_EDIT_MSG)?;
            let mut replacements = 0;
            for (index, edit) in edits.iter().enumerate() {
                replacements += apply_replacement(&mut seen, edit).map_err(|error| {
                    if multi {
                        format!("edit[{index}]: {error} (no edits applied)")
                    } else {
                        error
                    }
                })?;
            }
            let mut output = serde_json::json!({"replacements": replacements});
            if multi {
                output["edits_applied"] = serde_json::json!(edits.len());
            }
            Ok((seen.content, output))
        }
    }
}

fn apply_replacement(seen: &mut ObservedFile, edit: &Replacement) -> Result<usize, String> {
    let newline = if seen.content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let normalize = |value: &str| {
        value
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', newline)
    };
    let old = normalize(&edit.old);
    let new = normalize(&edit.new);
    let mut matches: Vec<_> = seen
        .content
        .match_indices(&old)
        .map(|(offset, _)| offset..offset + old.len())
        .collect();
    if matches.is_empty() {
        return Err("the 'old' string was not found after normalizing line endings".into());
    }
    if matches.len() > 1 && !edit.all {
        return Err(format!("old_string appears {} times in this file. Provide more context or pass replace_all: true.", matches.len()));
    }
    if matches.iter().any(|range| !seen.covers(range)) {
        return Err(UNSEEN_RANGE_MSG.into());
    }
    let count = matches.len();
    // Reverse order preserves original offsets. Track delivered ranges through
    // the in-memory edit so later multi-edits may target newly introduced text
    // without acquiring authority over any previously unseen suffix.
    for range in matches.drain(..).rev() {
        seen.content.replace_range(range.clone(), &new);
        for coverage in &mut seen.ranges {
            if coverage.start <= range.start && coverage.end >= range.end {
                coverage.end = coverage.end - old.len() + new.len();
            } else if coverage.start >= range.end {
                coverage.start = coverage.start - old.len() + new.len();
                coverage.end = coverage.end - old.len() + new.len();
            }
        }
    }
    Ok(count)
}

fn read_current(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn publish_checked(
    path: &Path,
    expected: Option<&str>,
    bytes: &[u8],
    before_commit: impl FnOnce() -> io::Result<()>,
    commit: impl FnOnce(&Path, &Path, bool) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("file path has no parent"))?;
    let mut temp = tempfile::Builder::new()
        .prefix(".deepagent-write-")
        .tempfile_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    let temp_path = temp.into_temp_path();
    before_commit()?;
    // Recheck after potentially slow preparation, not only before staging.
    // No await separates this comparison from publication. This is not a
    // filesystem CAS against non-cooperating writers in the final syscall gap.
    if read_current(path)?.as_deref() != expected {
        return Err(io::Error::other(STALE_READ_MSG));
    }
    commit(&temp_path, path, expected.is_some())
}

fn publish(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    if !replace {
        return rename_no_replace(source, destination);
    }
    replace_atomically(source, destination)
}

#[cfg(not(windows))]
fn replace_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
fn replace_atomically(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[path = "file_mutation_tests.rs"]
mod tests;
