//! One observed-version/commit boundary for write, edit and multi-edit.

use deepagent_persistence::fs_commit::lock_path_operation;
use deepagent_tools::{ToolExecutionContext, ToolOutput};
use std::io::{self, Write};

use crate::file_cache::{ObservedFile, SharedFileStateCache};
use crate::fs_guard::{StagedWrite, WorkspaceRoot, WriteTarget};

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
            .observed(&path, context.observation());
        let target = match root.write_target(
            &path,
            matches!(mutation, Mutation::Write(_)) && observed.is_none(),
        ) {
            Ok(target) => target,
            Err(error) => {
                return ToolOutput::failure(format!("write authorization failed: {error}"))
            }
        };
        let before = match target.read_current() {
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
            &target,
            before.as_deref(),
            content.as_bytes(),
            || {
                if context.is_cancelled() {
                    Err(io::Error::other(
                        "FILE_OPERATION_CANCELLED: no content was written",
                    ))
                } else if !context.observation_is_current() {
                    Err(io::Error::other(
                        "FILE_CONTEXT_REPLACED: read_file in the current model context before editing. No content was written",
                    ))
                } else {
                    Ok(())
                }
            },
            StagedWrite::publish,
        ) {
            Ok(()) => {
                cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .invalidate(&path);
                output["path"] = serde_json::json!(root.relativize(&path));
                output["lines_added"] = serde_json::json!(new_lines.saturating_sub(old_lines));
                output["lines_removed"] = serde_json::json!(old_lines.saturating_sub(new_lines));
                let mut result = ToolOutput::success(output);
                result.client_metadata = Some(serde_json::json!({
                    "file_change": change_preview(before.as_deref().unwrap_or_default(), &content)
                }));
                result
            }
            Err(error) => ToolOutput::failure(format!("write failed: {error}")),
        }
    })
    .await
    .map_err(|error| {
        deepagent_core::error::CoreError::other(format!("file mutation join failed: {error}"))
    })
}

/// A bounded display receipt of this commit, never the workspace's Git diff.
/// Trim only common outer lines, retaining three lines of context. Distant
/// edits may exceed the preview budget; the write still succeeds without it.
fn change_preview(before: &str, after: &str) -> serde_json::Value {
    const MAX_BYTES: usize = 128 * 1024;
    const MAX_LINES: usize = 1200;
    let old: Vec<_> = before.split_inclusive('\n').collect();
    let new: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let start = prefix.saturating_sub(3);
    let old_end = old.len() - suffix.saturating_sub(3);
    let new_end = new.len() - suffix.saturating_sub(3);
    let old_slice = &old[start..old_end];
    let new_slice = &new[start..new_end];
    if old_slice.len() >= MAX_LINES
        || new_slice.len() >= MAX_LINES
        || old_slice
            .iter()
            .chain(new_slice)
            .map(|line| line.len())
            .sum::<usize>()
            > MAX_BYTES
    {
        return serde_json::json!({"version": 1, "omitted": true, "reason": "preview_budget"});
    }
    serde_json::json!({
        "version": 1, "before": old_slice.concat(), "after": new_slice.concat(),
        "start_line": start + 1
    })
}

#[cfg(test)]
mod preview_tests {
    use super::change_preview;

    #[test]
    fn file_change_preview_keeps_actual_offsets_and_line_endings() {
        let prefix = "unchanged\r\n".repeat(100);
        let before = format!("{prefix}old\r\n{}", "tail\r\n".repeat(10));
        let after = format!("{prefix}new\r\n{}", "tail\r\n".repeat(10));
        let preview = change_preview(&before, &after);
        assert_eq!(preview["start_line"], 98);
        assert_eq!(
            preview["before"],
            "unchanged\r\n".repeat(3) + "old\r\n" + &"tail\r\n".repeat(3)
        );
        assert_eq!(
            preview["after"],
            "unchanged\r\n".repeat(3) + "new\r\n" + &"tail\r\n".repeat(3)
        );
    }

    #[test]
    fn file_change_preview_is_bounded_without_clipping_a_change() {
        for (before, after) in [
            ("a\n".repeat(1300), "b\n".repeat(1300)),
            ("a".repeat(128 * 1024), "b".repeat(128 * 1024)),
        ] {
            let preview = change_preview(&before, &after);
            assert_eq!(preview["omitted"], true);
            assert!(preview.get("before").is_none());
        }
        assert_eq!(change_preview("", "新文件\n")["before"], "");
    }
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

fn publish_checked(
    target: &WriteTarget,
    expected: Option<&str>,
    bytes: &[u8],
    before_commit: impl FnOnce() -> io::Result<()>,
    commit: impl FnOnce(StagedWrite, &WriteTarget, bool) -> io::Result<()>,
) -> io::Result<()> {
    let mut temp = target.stage()?;
    temp.file_mut().write_all(bytes)?;
    temp.file_mut().flush()?;
    temp.file_mut().sync_all()?;
    before_commit()?;
    // Recheck after potentially slow preparation, not only before staging.
    // No await separates this comparison from publication. This is not a
    // filesystem CAS against non-cooperating writers in the final syscall gap.
    if target.read_current()?.as_deref() != expected {
        return Err(io::Error::other(STALE_READ_MSG));
    }
    commit(temp, target, expected.is_some())
}

#[cfg(test)]
#[path = "file_mutation_tests.rs"]
mod tests;
