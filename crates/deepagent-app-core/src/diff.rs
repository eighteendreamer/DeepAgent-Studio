//! Line-based diff for the Codex-style Diff view.
//!
//! Computes a unified-style diff between two text blobs using the classic LCS
//! (longest common subsequence) algorithm, producing [`DiffLine`]s tagged as
//! context / added / removed. The UI renders these with the familiar green/red
//! gutters. Kept here (app-core) because the result is a serializable DTO the
//! frontend consumes directly.

use serde::{Deserialize, Serialize};

/// The kind of a diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    /// Unchanged line present in both sides.
    Context,
    /// Line added on the new side.
    Added,
    /// Line removed from the old side.
    Removed,
}

/// One line of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    /// Whether the line is context / added / removed.
    pub kind: DiffKind,
    /// Line number on the old side (None for added lines).
    pub old_line: Option<usize>,
    /// Line number on the new side (None for removed lines).
    pub new_line: Option<usize>,
    /// The line content (without trailing newline).
    pub content: String,
}

/// A computed diff plus summary counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffResult {
    /// The diff lines in order.
    pub lines: Vec<DiffLine>,
    /// Number of added lines.
    pub added: usize,
    /// Number of removed lines.
    pub removed: usize,
}

/// Compute a line-based diff from `old` to `new`.
pub fn diff_lines(old: &str, new: &str) -> DiffResult {
    let old_lines: Vec<&str> = split_lines(old);
    let new_lines: Vec<&str> = split_lines(new);

    let lcs = lcs_table(&old_lines, &new_lines);
    let mut lines = Vec::new();
    let mut added = 0;
    let mut removed = 0;

    // Walk the LCS table backwards to build the edit script, then reverse.
    let mut i = old_lines.len();
    let mut j = new_lines.len();
    let mut rev: Vec<DiffLine> = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            rev.push(DiffLine {
                kind: DiffKind::Context,
                old_line: Some(i),
                new_line: Some(j),
                content: old_lines[i - 1].to_string(),
            });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || lcs[i][j - 1] >= lcs[i - 1][j]) {
            rev.push(DiffLine {
                kind: DiffKind::Added,
                old_line: None,
                new_line: Some(j),
                content: new_lines[j - 1].to_string(),
            });
            added += 1;
            j -= 1;
        } else {
            rev.push(DiffLine {
                kind: DiffKind::Removed,
                old_line: Some(i),
                new_line: None,
                content: old_lines[i - 1].to_string(),
            });
            removed += 1;
            i -= 1;
        }
    }
    rev.reverse();
    lines.extend(rev);

    DiffResult {
        lines,
        added,
        removed,
    }
}

fn split_lines(s: &str) -> Vec<&str> {
    if s.is_empty() {
        Vec::new()
    } else {
        s.split('\n').collect()
    }
}

/// Build the LCS length table for two line sequences.
fn lcs_table(a: &[&str], b: &[&str]) -> Vec<Vec<usize>> {
    let n = a.len();
    let m = b.len();
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in 1..=n {
        for j in 1..=m {
            table[i][j] = if a[i - 1] == b[j - 1] {
                table[i - 1][j - 1] + 1
            } else {
                table[i - 1][j].max(table[i][j - 1])
            };
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_is_all_context() {
        let d = diff_lines("a\nb\nc", "a\nb\nc");
        assert_eq!(d.added, 0);
        assert_eq!(d.removed, 0);
        assert!(d.lines.iter().all(|l| l.kind == DiffKind::Context));
    }

    #[test]
    fn detects_added_line() {
        let d = diff_lines("a\nb", "a\nx\nb");
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 0);
        let added = d.lines.iter().find(|l| l.kind == DiffKind::Added).unwrap();
        assert_eq!(added.content, "x");
        assert_eq!(added.old_line, None);
        assert!(added.new_line.is_some());
    }

    #[test]
    fn detects_removed_line() {
        let d = diff_lines("a\nb\nc", "a\nc");
        assert_eq!(d.removed, 1);
        assert_eq!(d.added, 0);
        let removed = d
            .lines
            .iter()
            .find(|l| l.kind == DiffKind::Removed)
            .unwrap();
        assert_eq!(removed.content, "b");
    }

    #[test]
    fn detects_replacement() {
        let d = diff_lines("hello\nworld", "hello\nrust");
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
        // Order preserves context first.
        assert_eq!(d.lines[0].kind, DiffKind::Context);
        assert_eq!(d.lines[0].content, "hello");
    }

    #[test]
    fn from_empty_is_all_added() {
        let d = diff_lines("", "a\nb");
        assert_eq!(d.added, 2);
        assert_eq!(d.removed, 0);
    }

    #[test]
    fn to_empty_is_all_removed() {
        let d = diff_lines("a\nb", "");
        assert_eq!(d.removed, 2);
        assert_eq!(d.added, 0);
    }

    #[test]
    fn line_numbers_are_consistent() {
        let d = diff_lines("a\nb\nc", "a\nB\nc");
        // a: context (1,1); b removed (2,-); B added (-,2); c context (3,3).
        let ctx_a = &d.lines[0];
        assert_eq!(ctx_a.old_line, Some(1));
        assert_eq!(ctx_a.new_line, Some(1));
        let last = d.lines.last().unwrap();
        assert_eq!(last.content, "c");
        assert_eq!(last.old_line, Some(3));
        assert_eq!(last.new_line, Some(3));
    }
}
