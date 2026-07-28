//! Factual completion requirements from an EXPLICIT policy.
//!
//! Prompt-derived requirements (keyword-grep the user's prompt for
//! create/delete/move + path extraction) were removed on 2026-07-28: guessing
//! intent from prompt text is an anti-pattern that no upstream agent
//! (Claude Code / codex / grok) uses — they judge completion via the model's
//! own self-report, Stop hooks, and optional LLM verification. This module now
//! only holds the `validate` machinery for a policy constructed EXPLICITLY
//! (e.g. by a future tool/config source). A `default()` policy is empty and
//! never blocks completion.

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

    fn required(path: &str, effect: RequiredEffect) -> RequiredPathEffect {
        RequiredPathEffect {
            path: PathBuf::from(path),
            effect,
        }
    }

    // NOTE: `CompletionPolicy` is no longer derived from the user's prompt —
    // the keyword/path-extraction anti-pattern was removed (intent-layer
    // cleanup 2026-07-28; upstream does completion via model self-report +
    // Stop hook + optional LLM verifier, never by grepping the prompt).
    // These tests exercise the dormant `validate` machinery against EXPLICIT
    // policies, the only supported way to construct one now.

    #[test]
    fn default_policy_is_empty_and_always_passes() {
        let policy = CompletionPolicy::default();
        assert!(policy.is_empty());
        assert!(policy.validate(&[]).is_ok());
    }

    #[test]
    fn delete_requirement_needs_actual_deleted_evidence() {
        let policy = CompletionPolicy {
            require_delete: true,
            ..Default::default()
        };
        assert!(policy.validate(&[]).is_err());
        assert!(policy
            .validate(&[evidence(MutationKind::Unchanged)])
            .is_err());
        assert!(policy
            .validate(&[evidence_at("old-dir", MutationKind::Deleted)])
            .is_ok());
    }

    #[test]
    fn move_requirement_needs_source_and_destination_effects() {
        let policy = CompletionPolicy {
            require_move: true,
            ..Default::default()
        };
        assert!(policy.validate(&[evidence(MutationKind::Deleted)]).is_err());
        assert!(policy
            .validate(&[
                evidence_at("before.txt", MutationKind::Deleted),
                evidence_at("after.txt", MutationKind::Created)
            ])
            .is_ok());
    }

    #[test]
    fn explicit_path_requirement_matches_by_suffix() {
        let policy = CompletionPolicy {
            require_delete: true,
            required_paths: vec![required("old-dir", RequiredEffect::Delete)],
            ..Default::default()
        };
        assert!(policy
            .validate(&[evidence_at("workspace/other-dir", MutationKind::Deleted)])
            .is_err());
        assert!(policy
            .validate(&[evidence_at("workspace/old-dir", MutationKind::Deleted)])
            .is_ok());
    }

    #[test]
    fn explicit_create_path_requires_created_or_modified_target() {
        let policy = CompletionPolicy {
            require_create_or_modify: true,
            required_paths: vec![required("src/main.rs", RequiredEffect::CreateOrModify)],
            ..Default::default()
        };
        assert!(policy
            .validate(&[evidence_at("src/lib.rs", MutationKind::Created)])
            .is_err());
        assert!(policy
            .validate(&[evidence_at("C:/repo/src/main.rs", MutationKind::Created)])
            .is_ok());
    }
}
