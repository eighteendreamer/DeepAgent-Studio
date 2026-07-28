//! Factual completion requirements derived from explicit task intent.

use std::path::{Path, PathBuf};

use crate::checkpoint::{MutationEvidence, MutationKind};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompletionPolicy {
    pub require_create_or_modify: bool,
    pub require_delete: bool,
    pub require_move: bool,
    pub required_paths: Vec<RequiredPathEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredPathEffect {
    pub path: PathBuf,
    pub effect: RequiredEffect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredEffect {
    CreateOrModify,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionFailure {
    pub reason: String,
    pub required_effects: Vec<String>,
}

impl CompletionPolicy {
    /// Infer only clear imperative side-effect requests. Ambiguous/read-only
    /// prompts intentionally produce an empty policy.
    pub fn from_prompt(prompt: &str) -> Self {
        let lower = prompt.to_lowercase();
        let delete_negated = contains_any(
            &lower,
            &[
                "do not delete",
                "don't delete",
                "不要删除",
                "不要删",
                "勿删",
            ],
        );
        let move_negated = contains_any(
            &lower,
            &["do not move", "do not rename", "不要移动", "不要重命名"],
        );
        let write_negated = contains_any(
            &lower,
            &["do not create", "do not edit", "不要创建", "不要修改"],
        );

        let require_create_or_modify = !write_negated
            && contains_any(
                &lower,
                &[
                    "create ", "write ", "edit ", "modify ", "新增", "新建", "创建", "写入",
                    "修改", "编辑",
                ],
            );
        let require_delete = !delete_negated
            && contains_any(
                &lower,
                &["delete ", "remove ", "删除", "删掉", "删了", "移除", "削除"],
            );
        let require_move =
            !move_negated && contains_any(&lower, &["rename ", "move ", "重命名", "改名", "移动"]);
        let mentioned_paths = extract_path_mentions(prompt);
        let mut required_paths = Vec::new();
        if require_move && mentioned_paths.len() >= 2 {
            required_paths.push(RequiredPathEffect {
                path: mentioned_paths[0].clone(),
                effect: RequiredEffect::Delete,
            });
            required_paths.push(RequiredPathEffect {
                path: mentioned_paths[1].clone(),
                effect: RequiredEffect::CreateOrModify,
            });
        } else {
            if require_delete {
                for path in &mentioned_paths {
                    push_required_path(&mut required_paths, path.clone(), RequiredEffect::Delete);
                }
            }
            if require_create_or_modify {
                for path in &mentioned_paths {
                    push_required_path(
                        &mut required_paths,
                        path.clone(),
                        RequiredEffect::CreateOrModify,
                    );
                }
            }
        }

        Self {
            require_create_or_modify,
            require_delete,
            require_move,
            required_paths,
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.require_create_or_modify && !self.require_delete && !self.require_move
    }

    pub fn validate(
        &self,
        evidence: &[MutationEvidence],
    ) -> std::result::Result<(), CompletionFailure> {
        if self.is_empty() {
            return Ok(());
        }
        let has_created = evidence
            .iter()
            .any(|item| item.kind == MutationKind::Created);
        let has_modified = evidence
            .iter()
            .any(|item| item.kind == MutationKind::Modified);
        let has_deleted = evidence
            .iter()
            .any(|item| item.kind == MutationKind::Deleted);
        let mut missing = Vec::new();
        if self.require_create_or_modify && !(has_created || has_modified) {
            missing.push("created_or_modified_path".to_string());
        }
        if self.require_delete && !has_deleted {
            missing.push("deleted_path".to_string());
        }
        if self.require_move && !(has_deleted && (has_created || has_modified)) {
            missing.push("move_source_and_destination".to_string());
        }
        for requirement in &self.required_paths {
            if !requirement_satisfied(requirement, evidence) {
                missing.push(format!(
                    "{}:{}",
                    requirement.effect.label(),
                    requirement.path.display()
                ));
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CompletionFailure {
                reason: format!(
                    "completion evidence is missing required filesystem effect(s): {}",
                    missing.join(", ")
                ),
                required_effects: missing,
            })
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn push_required_path(
    requirements: &mut Vec<RequiredPathEffect>,
    path: PathBuf,
    effect: RequiredEffect,
) {
    let item = RequiredPathEffect { path, effect };
    if !requirements.contains(&item) {
        requirements.push(item);
    }
}

impl RequiredEffect {
    const fn label(self) -> &'static str {
        match self {
            Self::CreateOrModify => "created_or_modified_path",
            Self::Delete => "deleted_path",
        }
    }
}

fn requirement_satisfied(requirement: &RequiredPathEffect, evidence: &[MutationEvidence]) -> bool {
    evidence.iter().any(|item| {
        path_matches(&requirement.path, &item.path)
            && match requirement.effect {
                RequiredEffect::CreateOrModify => {
                    matches!(item.kind, MutationKind::Created | MutationKind::Modified)
                }
                RequiredEffect::Delete => item.kind == MutationKind::Deleted,
            }
    })
}

fn extract_path_mentions(prompt: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for quote in ['`', '"', '\''] {
        collect_quoted_paths(prompt, quote, &mut out);
    }
    // Segment on any non-path character instead of whitespace alone: CJK
    // prose attaches full-width punctuation (：，；（）) and even verbs
    // directly to file names ("工程：Calculator.java", "删除del_dir.py"),
    // and whitespace tokenization turned those into garbage required paths
    // that no real mutation could ever satisfy (manual acceptance finding,
    // 2026-07-28). Path-legal characters are ASCII alnum plus . _ - / \ :
    for segment in prompt.split(|ch: char| !is_path_char(ch)) {
        let token = trim_path_token(segment);
        if looks_like_path(token) {
            push_path(&mut out, token);
        }
    }
    for keyword in [
        "delete", "remove", "create", "write", "edit", "modify", "rename", "move",
    ] {
        if let Some(token) = token_after_keyword(prompt, keyword) {
            push_path(&mut out, token);
        }
    }
    out
}

/// Characters that may legitimately appear inside a mentioned path. ASCII
/// colon stays legal for Windows drive letters; the full-width variants
/// (：，。) are NOT path characters and act as separators.
fn is_path_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/' | '\\' | ':' | '~')
}

fn collect_quoted_paths(prompt: &str, quote: char, out: &mut Vec<PathBuf>) {
    let mut start = None;
    for (index, ch) in prompt.char_indices() {
        if ch != quote {
            continue;
        }
        if let Some(open) = start.take() {
            let candidate = prompt[open + quote.len_utf8()..index].trim();
            if candidate.len() <= 260 && !candidate.contains('\n') && !candidate.is_empty() {
                push_path(out, candidate);
            }
        } else {
            start = Some(index);
        }
    }
}

fn token_after_keyword<'a>(prompt: &'a str, keyword: &str) -> Option<&'a str> {
    let lower = prompt.to_lowercase();
    let index = lower.find(keyword)?;
    let tail = &prompt[index + keyword.len()..];
    // Same path-character segmentation as extract_path_mentions: full-width
    // punctuation and CJK prose must never fuse into the extracted token.
    tail.split(|ch: char| !is_path_char(ch))
        .map(trim_path_token)
        .find(|token| !token.is_empty() && !is_path_stopword(token))
}

fn trim_path_token(token: &str) -> &str {
    token.trim_matches(|ch: char| {
        matches!(
            ch,
            ',' | ';'
                | ':'
                | '.'
                | '!'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
                | '<'
                | '>'
                | '"'
                | '\''
                | '`'
                | '“'
                | '”'
                | '‘'
                | '’'
        )
    })
}

fn looks_like_path(token: &str) -> bool {
    if token.is_empty() || is_path_stopword(token) {
        return false;
    }
    token.contains('/')
        || token.contains('\\')
        || token.starts_with('.')
        || token.get(1..3).is_some_and(|slice| slice == ":\\")
        || token
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|last| last.contains('.') && !last.ends_with('.'))
}

fn is_path_stopword(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "the"
            | "a"
            | "an"
            | "this"
            | "that"
            | "entire"
            | "whole"
            | "first"
            | "last"
            | "directory"
            | "folder"
            | "file"
            | "recursively"
            | "to"
            | "from"
    )
}

fn push_path(out: &mut Vec<PathBuf>, value: &str) {
    let path = PathBuf::from(value);
    if !out.contains(&path) {
        out.push(path);
    }
}

fn path_matches(expected: &Path, actual: &Path) -> bool {
    let expected = normalized_path_text(expected);
    let actual = normalized_path_text(actual);
    if expected.is_empty() || actual.is_empty() {
        return false;
    }
    actual == expected || actual.ends_with(&format!("/{expected}"))
}

fn normalized_path_text(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_matches('/')
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn evidence(kind: MutationKind) -> MutationEvidence {
        evidence_at("target", kind)
    }

    fn evidence_at(path: impl Into<PathBuf>, kind: MutationKind) -> MutationEvidence {
        MutationEvidence {
            path: path.into(),
            kind,
        }
    }

    #[test]
    fn recognizes_chinese_delete_without_matching_negation() {
        assert!(CompletionPolicy::from_prompt("删掉第一个目录").require_delete);
        assert!(!CompletionPolicy::from_prompt("不要删除这个目录").require_delete);
    }

    #[test]
    fn cjk_fullwidth_punctuation_never_fuses_into_required_paths() {
        // Regression: manual acceptance 2026-07-28 — this exact prompt
        // produced required paths "工程：Calculator.java" and
        // "Main.java，实现加减法并用", which no real mutation could satisfy,
        // failing runs whose files WERE created.
        let policy = CompletionPolicy::from_prompt(
            "创建一个带 git 的 Java 工程：Calculator.java + Main.java，实现加减法并用 javac 编译验证；然后新增乘法（跨文件改动）并重新编译",
        );
        let required: Vec<String> = policy
            .required_paths
            .iter()
            .map(|item| item.path.to_string_lossy().to_string())
            .collect();
        assert!(
            required.contains(&"Calculator.java".to_string()),
            "required: {required:?}"
        );
        assert!(
            required.contains(&"Main.java".to_string()),
            "required: {required:?}"
        );
        for path in &required {
            assert!(
                path.is_ascii(),
                "CJK prose leaked into a required path: {path}"
            );
        }
        // The real evidence from the failed run now satisfies the policy.
        policy
            .validate(&[
                evidence_at(
                    "G:\\managed-test\\calc\\Calculator.java",
                    MutationKind::Created,
                ),
                evidence_at("G:\\managed-test\\calc\\Main.java", MutationKind::Created),
            ])
            .expect("created files must satisfy the extracted requirements");
    }

    #[test]
    fn cjk_verb_glued_to_filename_extracts_clean_path() {
        // Regression sample from logs: "删除del_dir.py" became the required
        // deleted path verbatim (verb included).
        let policy = CompletionPolicy::from_prompt("删除del_dir.py");
        let required: Vec<String> = policy
            .required_paths
            .iter()
            .map(|item| item.path.to_string_lossy().to_string())
            .collect();
        assert_eq!(required, vec!["del_dir.py".to_string()]);
        policy
            .validate(&[evidence_at("ws/del_dir.py", MutationKind::Deleted)])
            .expect("deleting the actual file must satisfy the policy");
    }

    #[test]
    fn fullwidth_colon_after_path_is_a_separator() {
        // Regression sample from logs: "src/main.rs：新增一个..." fused.
        let policy = CompletionPolicy::from_prompt("修改 src/main.rs：新增一个 add 函数并编译");
        let required: Vec<String> = policy
            .required_paths
            .iter()
            .map(|item| item.path.to_string_lossy().to_string())
            .collect();
        assert!(
            required.contains(&"src/main.rs".to_string()),
            "required: {required:?}"
        );
        policy
            .validate(&[evidence_at("ws/src/main.rs", MutationKind::Modified)])
            .expect("modifying the actual file must satisfy the policy");
    }

    #[test]
    fn delete_requires_actual_deleted_evidence() {
        let policy = CompletionPolicy::from_prompt("Delete old-dir recursively.");
        assert!(policy.validate(&[]).is_err());
        assert!(policy
            .validate(&[evidence(MutationKind::Unchanged)])
            .is_err());
        assert!(policy
            .validate(&[evidence_at("old-dir", MutationKind::Deleted)])
            .is_ok());
    }

    #[test]
    fn rename_requires_source_and_destination_effects() {
        let policy = CompletionPolicy::from_prompt("Rename before.txt to after.txt.");
        assert!(policy.validate(&[evidence(MutationKind::Deleted)]).is_err());
        assert!(policy
            .validate(&[
                evidence_at("before.txt", MutationKind::Deleted),
                evidence_at("after.txt", MutationKind::Created)
            ])
            .is_ok());
    }

    #[test]
    fn explicit_delete_path_requires_that_path() {
        let policy = CompletionPolicy::from_prompt("Delete `old-dir` recursively.");
        assert_eq!(policy.required_paths.len(), 1);
        assert!(policy
            .validate(&[evidence_at("workspace/other-dir", MutationKind::Deleted)])
            .is_err());
        assert!(policy
            .validate(&[evidence_at("workspace/old-dir", MutationKind::Deleted)])
            .is_ok());
    }

    #[test]
    fn explicit_create_path_requires_created_or_modified_target() {
        let policy = CompletionPolicy::from_prompt("Create src/main.rs with hello world.");
        assert!(policy
            .required_paths
            .iter()
            .any(|item| item.path == PathBuf::from("src/main.rs")
                && item.effect == RequiredEffect::CreateOrModify));
        assert!(policy
            .validate(&[evidence_at("src/lib.rs", MutationKind::Created)])
            .is_err());
        assert!(policy
            .validate(&[evidence_at("C:/repo/src/main.rs", MutationKind::Created)])
            .is_ok());
    }

    #[test]
    fn move_with_explicit_paths_checks_source_and_destination() {
        let policy = CompletionPolicy::from_prompt("Move ./before.txt to ./after.txt");
        assert!(policy
            .validate(&[
                evidence_at("before.txt", MutationKind::Deleted),
                evidence_at("other.txt", MutationKind::Created),
            ])
            .is_err());
        assert!(policy
            .validate(&[
                evidence_at("repo/before.txt", MutationKind::Deleted),
                evidence_at("repo/after.txt", MutationKind::Created),
            ])
            .is_ok());
    }

    #[test]
    fn read_only_prompts_do_not_require_mutation_evidence() {
        let policy = CompletionPolicy::from_prompt("Read src/main.rs and explain what it does.");
        assert!(policy.is_empty());
        assert!(policy.validate(&[]).is_ok());
    }
}
