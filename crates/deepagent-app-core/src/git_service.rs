//! Read-only Git service for the desktop Git Workbench.
//!
//! This is the UI-facing Git boundary for branch chips, changes panels, branch
//! comparison, and lightweight multi-project commit/upload flows.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use deepagent_core::error::Result;

use crate::dto::{
    GitBatchCommitPreviewItemDto, GitBatchCommitTargetDto, GitBatchProjectResultDto, GitBranchDto,
    GitChangedFileDto, GitChangesDto, GitCommitFileDto, GitCommitMessageDraftDto,
    GitCompareCommitDto, GitDiffDto, GitLogEntryDto, GitOperationResultDto, GitProjectStatusDto,
    GitPushCommitDto, GitPushPreviewDto, GitPushRiskItemDto, GitPushRiskScanDto, GitRefCompareDto,
    GitWorktreeDto,
};
use crate::project_service::ProjectService;
use std::sync::Arc;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
const MAX_DIFF_BYTES: usize = 96_000;

/// Read-only Git inspection service.
pub struct GitService {
    projects: Arc<ProjectService>,
}

impl GitService {
    pub fn new(projects: Arc<ProjectService>) -> Self {
        Self { projects }
    }

    /// Status for one project path. Non-repositories return a negative status
    /// rather than an error so the UI can simply hide Git affordances.
    pub fn project_status(&self, project_path: &str) -> Result<GitProjectStatusDto> {
        Ok(self.status_for(project_path))
    }

    /// Status for all opened projects, or for an explicit path list.
    pub fn projects_status(&self, paths: Option<Vec<String>>) -> Result<Vec<GitProjectStatusDto>> {
        let paths = match paths {
            Some(paths) => paths,
            None => self.projects.list()?.into_iter().map(|p| p.path).collect(),
        };
        Ok(paths
            .into_iter()
            .map(|path| self.status_for(&path))
            .collect())
    }

    /// Branch rows for the popup. Non-repositories return an empty list.
    pub fn branch_list(&self, project_path: &str) -> Result<Vec<GitBranchDto>> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(Vec::new());
        };
        let current_ref = git_output(&repo_root, &["symbolic-ref", "-q", "HEAD"]);
        let current_short = git_output(&repo_root, &["branch", "--show-current"]);
        let worktree_by_branch = self
            .worktrees(project_path)?
            .into_iter()
            .filter_map(|wt| wt.branch.map(|branch| (branch, wt.path)))
            .collect::<HashMap<_, _>>();

        let fmt =
            "%(refname)%1f%(refname:short)%1f%(objectname:short)%1f%(subject)%1f%(upstream:short)%1f%(upstream:track)";
        let output = git_output(
            &repo_root,
            &[
                "for-each-ref",
                "--sort=-committerdate",
                &format!("--format={fmt}"),
                "refs/heads",
                "refs/remotes",
            ],
        )
        .unwrap_or_default();

        let mut branches = Vec::new();
        for line in output.lines() {
            let mut parts = line.split('\x1f');
            let full_name = parts.next().unwrap_or("").trim().to_string();
            let short = parts.next().unwrap_or("").trim().to_string();
            if full_name.is_empty() || short.is_empty() || short.ends_with("/HEAD") {
                continue;
            }
            let kind = if full_name.starts_with("refs/remotes/") {
                "remote"
            } else {
                "local"
            };
            let commit = empty_to_none(parts.next().map(str::trim).unwrap_or(""));
            let subject = empty_to_none(parts.next().map(str::trim).unwrap_or(""));
            let upstream = empty_to_none(parts.next().map(str::trim).unwrap_or(""));
            let (ahead, behind) = parse_upstream_track(parts.next().map(str::trim).unwrap_or(""));
            let current = current_ref.as_deref() == Some(full_name.as_str())
                || (kind == "local" && current_short.as_deref() == Some(short.as_str()));
            branches.push(GitBranchDto {
                name: short.clone(),
                full_name: full_name.clone(),
                kind: kind.to_string(),
                current,
                upstream,
                ahead,
                behind,
                commit,
                subject,
                worktree_path: worktree_by_branch.get(&full_name).cloned(),
            });
        }

        branches.sort_by_key(|b| (!b.current, b.kind != "local", b.name.to_lowercase()));
        Ok(branches)
    }

    /// Changed files and +/- summary. Non-repositories return an empty result.
    pub fn changes(&self, project_path: &str) -> Result<GitChangesDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitChangesDto {
                project_path: project_path.to_string(),
                repo_root: None,
                is_repo: false,
                files: Vec::new(),
                additions: 0,
                deletions: 0,
            });
        };

        let status =
            git_stdout(&repo_root, &["status", "--porcelain=v1", "-z"]).unwrap_or_default();
        let numstat =
            git_output(&repo_root, &["diff", "--numstat", "HEAD", "--"]).unwrap_or_default();
        let stats = parse_numstat(&numstat);
        let mut files = parse_status_z(&status, &stats);
        files.sort_by_key(|f| (category_order(&f.category), f.path.to_lowercase()));
        let additions = files.iter().map(|f| f.additions).sum();
        let deletions = files.iter().map(|f| f.deletions).sum();

        Ok(GitChangesDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            is_repo: true,
            files,
            additions,
            deletions,
        })
    }

    /// Unified diff for one changed file. Non-repositories return an empty diff.
    pub fn diff(&self, project_path: &str, file_path: &str, staged: bool) -> Result<GitDiffDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitDiffDto {
                project_path: project_path.to_string(),
                repo_root: None,
                file_path: file_path.to_string(),
                staged,
                is_repo: false,
                text: String::new(),
                truncated: false,
            });
        };

        let mut args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            "--".to_string(),
            file_path.to_string(),
        ];
        if staged {
            args.insert(1, "--staged".to_string());
        }
        let raw = git_stdout_owned(&repo_root, &args).unwrap_or_default();
        let raw = if raw.is_empty() && !staged && is_untracked_file(&repo_root, file_path) {
            synthetic_untracked_diff(&repo_root, file_path).unwrap_or_default()
        } else {
            raw
        };
        let (text, truncated) = truncate_diff(raw);

        Ok(GitDiffDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            file_path: file_path.to_string(),
            staged,
            is_repo: true,
            text,
            truncated,
        })
    }

    /// Recent commit log with changed files for the Log view.
    pub fn log(&self, project_path: &str, limit: Option<u32>) -> Result<Vec<GitLogEntryDto>> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(200).clamp(1, 1000).to_string();
        let args = vec![
            "log".to_string(),
            format!("--max-count={limit}"),
            "--date=iso-strict".to_string(),
            "--pretty=format:\x1e%h\x1f%H\x1f%P\x1f%an\x1f%ae\x1f%ad\x1f%D\x1f%s".to_string(),
            "--numstat".to_string(),
        ];
        let output = git_stdout_owned(&repo_root, &args).unwrap_or_default();
        Ok(parse_log_entries(&output))
    }

    /// Diff for one file within a commit.
    pub fn commit_diff(
        &self,
        project_path: &str,
        commit: &str,
        file_path: &str,
    ) -> Result<GitDiffDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitDiffDto {
                project_path: project_path.to_string(),
                repo_root: None,
                file_path: file_path.to_string(),
                staged: false,
                is_repo: false,
                text: String::new(),
                truncated: false,
            });
        };
        let args = vec![
            "show".to_string(),
            "--format=".to_string(),
            "--no-ext-diff".to_string(),
            commit.to_string(),
            "--".to_string(),
            file_path.to_string(),
        ];
        let raw = git_stdout_owned(&repo_root, &args).unwrap_or_default();
        let (text, truncated) = truncate_diff(raw);
        Ok(GitDiffDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            file_path: file_path.to_string(),
            staged: false,
            is_repo: true,
            text,
            truncated,
        })
    }

    /// Stage one or more paths.
    pub fn stage(&self, project_path: &str, files: &[String]) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure("git add", "not a git repository"));
        };
        if files.is_empty() {
            return Ok(operation_failure("git add", "no files selected"));
        }
        let mut args = vec!["add".to_string(), "--".to_string()];
        args.extend(files.iter().cloned());
        Ok(run_git_operation(&repo_root, args))
    }

    /// Unstage one or more paths.
    pub fn unstage(&self, project_path: &str, files: &[String]) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure(
                "git restore --staged",
                "not a git repository",
            ));
        };
        if files.is_empty() {
            return Ok(operation_failure(
                "git restore --staged",
                "no files selected",
            ));
        }
        let mut args = vec![
            "restore".to_string(),
            "--staged".to_string(),
            "--".to_string(),
        ];
        args.extend(files.iter().cloned());
        Ok(run_git_operation(&repo_root, args))
    }

    /// Apply one unified diff hunk to the index. When `staged` is false this
    /// stages the hunk; when true it reverses a staged hunk back out of the
    /// index. The working tree is intentionally left untouched.
    pub fn apply_hunk(
        &self,
        project_path: &str,
        file_path: &str,
        patch: &str,
        staged: bool,
    ) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure(
                "git apply --cached",
                "not a git repository",
            ));
        };
        if safe_repo_file_path(&repo_root, file_path).is_none() {
            return Ok(operation_failure(
                "git apply --cached",
                "invalid repository-relative path",
            ));
        }
        if !validate_single_file_hunk_patch(file_path, patch) {
            return Ok(operation_failure(
                "git apply --cached",
                "patch must contain hunks for the selected file only",
            ));
        }
        let status = self.status_for(project_path);
        if status.merge_state {
            return Ok(operation_failure(
                "git apply --cached",
                "finish or abort the active merge before staging hunks",
            ));
        }
        if status.rebase_state.is_some() {
            return Ok(operation_failure(
                "git apply --cached",
                "finish or abort the active rebase before staging hunks",
            ));
        }

        let mut args = vec!["apply".to_string(), "--cached".to_string()];
        if staged {
            args.push("--reverse".to_string());
        }
        Ok(run_git_operation_with_stdin(&repo_root, args, patch))
    }

    /// Switch to an existing local branch, or create a tracking branch for a
    /// remote branch. This intentionally never uses force checkout.
    pub fn checkout_branch(
        &self,
        project_path: &str,
        branch: &str,
    ) -> Result<GitOperationResultDto> {
        let branch = branch.trim();
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure("git switch", "not a git repository"));
        };
        if branch.is_empty() {
            return Ok(operation_failure("git switch", "branch must not be empty"));
        }
        let status = self.status_for(project_path);
        if status.merge_state {
            return Ok(operation_failure(
                "git switch",
                "finish or abort the active merge before switching branches",
            ));
        }
        if status.rebase_state.is_some() {
            return Ok(operation_failure(
                "git switch",
                "finish or abort the active rebase before switching branches",
            ));
        }
        let normalized = normalize_branch_ref(branch);
        if !ref_exists(&repo_root, &normalized) {
            return Ok(operation_failure("git switch", "branch does not exist"));
        }
        if normalized.starts_with("refs/remotes/") || remote_branch_exists(&repo_root, &normalized)
        {
            let remote_ref = if normalized.starts_with("refs/remotes/") {
                normalized.trim_start_matches("refs/remotes/").to_string()
            } else {
                normalized.to_string()
            };
            return Ok(run_git_operation(
                &repo_root,
                vec!["switch".to_string(), "--track".to_string(), remote_ref],
            ));
        }
        Ok(run_git_operation(
            &repo_root,
            vec!["switch".to_string(), normalized],
        ))
    }

    /// Create a local branch at `start_point` and switch to it. The branch name
    /// is validated by Git before any mutation happens.
    pub fn create_branch(
        &self,
        project_path: &str,
        name: &str,
        start_point: Option<&str>,
    ) -> Result<GitOperationResultDto> {
        let name = name.trim();
        let start_point = start_point.and_then(empty_to_none);
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure("git switch -c", "not a git repository"));
        };
        if name.is_empty() {
            return Ok(operation_failure(
                "git switch -c",
                "branch name must not be empty",
            ));
        }
        if !valid_branch_name(&repo_root, name) {
            return Ok(operation_failure("git switch -c", "invalid branch name"));
        }
        if ref_exists(&repo_root, name) || ref_exists(&repo_root, &format!("refs/heads/{name}")) {
            return Ok(operation_failure("git switch -c", "branch already exists"));
        }
        let status = self.status_for(project_path);
        if status.merge_state {
            return Ok(operation_failure(
                "git switch -c",
                "finish or abort the active merge before creating a branch",
            ));
        }
        if status.rebase_state.is_some() {
            return Ok(operation_failure(
                "git switch -c",
                "finish or abort the active rebase before creating a branch",
            ));
        }
        let mut args = vec!["switch".to_string(), "-c".to_string(), name.to_string()];
        if let Some(start) = start_point {
            let start = normalize_branch_ref(&start);
            if !ref_exists(&repo_root, &start) {
                return Ok(operation_failure(
                    "git switch -c",
                    "start point does not exist",
                ));
            }
            args.push(start);
        }
        Ok(run_git_operation(&repo_root, args))
    }

    /// Create a local commit from the current index. The UI is responsible for
    /// user confirmation before invoking this operation.
    pub fn commit(&self, project_path: &str, message: &str) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure("git commit", "not a git repository"));
        };
        let message = message.trim();
        if message.is_empty() {
            return Ok(operation_failure(
                "git commit",
                "commit message must not be empty",
            ));
        }
        Ok(run_git_operation(
            &repo_root,
            vec!["commit".to_string(), "-m".to_string(), message.to_string()],
        ))
    }

    /// Produce a commit-message draft from staged changes, falling back to the
    /// whole working tree when nothing is staged. This is deliberately read-only
    /// and does not create a commit.
    pub fn commit_message_draft(&self, project_path: &str) -> Result<GitCommitMessageDraftDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitCommitMessageDraftDto {
                project_path: project_path.to_string(),
                repo_root: None,
                is_repo: false,
                source: "none".to_string(),
                title: String::new(),
                body: String::new(),
                files: Vec::new(),
                blocked_reason: Some("not a git repository".to_string()),
            });
        };

        let changes = self.changes(project_path)?;
        let staged: Vec<GitChangedFileDto> = changes
            .files
            .iter()
            .filter(|file| file.category == "staged")
            .cloned()
            .collect();
        let (source, files) = if staged.is_empty() {
            ("working_tree", changes.files.clone())
        } else {
            ("staged", staged)
        };
        if files.is_empty() {
            return Ok(GitCommitMessageDraftDto {
                project_path: project_path.to_string(),
                repo_root: Some(repo_root.to_string_lossy().into_owned()),
                is_repo: true,
                source: source.to_string(),
                title: String::new(),
                body: String::new(),
                files,
                blocked_reason: Some("no Git changes to summarize".to_string()),
            });
        }
        let (title, body) = build_commit_message_draft(&files);
        Ok(GitCommitMessageDraftDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            is_repo: true,
            source: source.to_string(),
            title,
            body,
            files,
            blocked_reason: None,
        })
    }

    /// Preview what a normal push would send to the configured upstream.
    pub fn push_preview(&self, project_path: &str) -> Result<GitPushPreviewDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitPushPreviewDto {
                project_path: project_path.to_string(),
                repo_root: None,
                is_repo: false,
                current_branch: None,
                upstream: None,
                remote: None,
                remote_branch: None,
                ahead: 0,
                behind: 0,
                commits: Vec::new(),
                blocked_reason: Some("not a git repository".to_string()),
            });
        };

        let status = self.status_for(project_path);
        let (remote, remote_branch) = status
            .upstream
            .as_deref()
            .and_then(parse_upstream)
            .unwrap_or((None, None));
        let mut blocked_reason = None;
        if status.detached_head {
            blocked_reason = Some("detached HEAD cannot be pushed from this view".to_string());
        } else if status.merge_state {
            blocked_reason = Some("finish or abort the active merge before pushing".to_string());
        } else if status.rebase_state.is_some() {
            blocked_reason = Some("finish or abort the active rebase before pushing".to_string());
        } else if status.upstream.is_none() {
            blocked_reason = Some("no upstream branch is configured".to_string());
        } else if status.behind > 0 {
            blocked_reason = Some("upstream has new commits; update before pushing".to_string());
        } else if status.ahead == 0 {
            blocked_reason = Some("nothing to push".to_string());
        }

        let commits = if status.upstream.is_some() && status.ahead > 0 {
            let output = git_output(
                &repo_root,
                &[
                    "log",
                    "--reverse",
                    "--date=iso-strict",
                    "--pretty=format:%h%x1f%H%x1f%an%x1f%ad%x1f%s",
                    "@{u}..HEAD",
                ],
            )
            .unwrap_or_default();
            parse_push_commits(&output)
        } else {
            Vec::new()
        };

        Ok(GitPushPreviewDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            is_repo: true,
            current_branch: status.current_branch,
            upstream: status.upstream,
            remote,
            remote_branch,
            ahead: status.ahead,
            behind: status.behind,
            commits,
            blocked_reason,
        })
    }

    /// Read-only pre-push risk scan over outgoing commits and patches.
    pub fn push_risk_scan(&self, project_path: &str) -> Result<GitPushRiskScanDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitPushRiskScanDto {
                project_path: project_path.to_string(),
                repo_root: None,
                is_repo: false,
                current_branch: None,
                upstream: None,
                ahead: 0,
                scanned_files: 0,
                risks: Vec::new(),
                blocked_reason: Some("not a git repository".to_string()),
            });
        };
        let preview = self.push_preview(project_path)?;
        if let Some(reason) = preview.blocked_reason.as_deref() {
            return Ok(GitPushRiskScanDto {
                project_path: project_path.to_string(),
                repo_root: Some(repo_root.to_string_lossy().into_owned()),
                is_repo: true,
                current_branch: preview.current_branch,
                upstream: preview.upstream,
                ahead: preview.ahead,
                scanned_files: 0,
                risks: Vec::new(),
                blocked_reason: Some(reason.to_string()),
            });
        }

        let range = "@{u}..HEAD";
        let changed = git_stdout_owned(
            &repo_root,
            &[
                "diff".to_string(),
                "--name-status".to_string(),
                range.to_string(),
                "--".to_string(),
            ],
        )
        .unwrap_or_default();
        let changed_files = parse_name_status_paths(&changed);
        let mut risks = Vec::new();
        for file in &changed_files {
            risks.extend(risks_for_file_name(file));
            if let Some(size) = git_blob_size(&repo_root, "HEAD", file) {
                if size > 5 * 1024 * 1024 {
                    risks.push(GitPushRiskItemDto {
                        severity: "high".to_string(),
                        category: "large_file".to_string(),
                        title: "Large file in outgoing commit".to_string(),
                        detail: format!("{} is {:.1} MB in HEAD.", file, size as f64 / 1_048_576.0),
                        file_path: Some(file.clone()),
                    });
                }
            }
        }

        let numstat = git_stdout_owned(
            &repo_root,
            &[
                "diff".to_string(),
                "--numstat".to_string(),
                range.to_string(),
                "--".to_string(),
            ],
        )
        .unwrap_or_default();
        risks.extend(binary_file_risks(&numstat));

        let patch = git_stdout_owned(
            &repo_root,
            &[
                "diff".to_string(),
                "--no-ext-diff".to_string(),
                "--unified=0".to_string(),
                range.to_string(),
                "--".to_string(),
            ],
        )
        .unwrap_or_default();
        risks.extend(scan_patch_risks(&patch));
        dedupe_risks(&mut risks);

        Ok(GitPushRiskScanDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            is_repo: true,
            current_branch: preview.current_branch,
            upstream: preview.upstream,
            ahead: preview.ahead,
            scanned_files: changed_files.len() as u32,
            risks,
            blocked_reason: None,
        })
    }

    /// Execute a normal push to the configured or explicitly supplied target.
    /// Force-push is intentionally not exposed here; the UI should add a
    /// separate, heavily confirmed flow if it ever needs that operation.
    pub fn push(
        &self,
        project_path: &str,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure("git push", "not a git repository"));
        };
        let preview = self.push_preview(project_path)?;
        if let Some(reason) = preview.blocked_reason.as_deref() {
            return Ok(operation_failure("git push", reason));
        }
        let remote = remote
            .and_then(empty_to_none)
            .or(preview.remote)
            .unwrap_or_else(|| "origin".to_string());
        let branch = branch
            .and_then(empty_to_none)
            .or(preview.remote_branch)
            .or(preview.current_branch)
            .unwrap_or_default();
        if branch.is_empty() {
            return Ok(operation_failure("git push", "no target branch resolved"));
        }
        Ok(run_git_operation(
            &repo_root,
            vec!["push".to_string(), remote, format!("HEAD:{branch}")],
        ))
    }

    /// Fetch remotes for the repository containing this project.
    pub fn fetch(&self, project_path: &str, all: bool) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure("git fetch", "not a git repository"));
        };
        let mut args = vec!["fetch".to_string()];
        if all {
            args.push("--all".to_string());
        }
        args.push("--prune".to_string());
        Ok(run_git_operation(&repo_root, args))
    }

    /// Update the current branch from upstream using fast-forward only.
    pub fn pull_update(&self, project_path: &str) -> Result<GitOperationResultDto> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(operation_failure(
                "git pull --ff-only",
                "not a git repository",
            ));
        };
        let status = self.status_for(project_path);
        if status.detached_head {
            return Ok(operation_failure(
                "git pull --ff-only",
                "detached HEAD cannot be updated from this view",
            ));
        }
        if status.merge_state {
            return Ok(operation_failure(
                "git pull --ff-only",
                "finish or abort the active merge before updating",
            ));
        }
        if status.rebase_state.is_some() {
            return Ok(operation_failure(
                "git pull --ff-only",
                "finish or abort the active rebase before updating",
            ));
        }
        if status.has_changes {
            return Ok(operation_failure(
                "git pull --ff-only",
                "commit or stash local changes before updating",
            ));
        }
        if status.upstream.is_none() {
            return Ok(operation_failure(
                "git pull --ff-only",
                "no upstream branch is configured",
            ));
        }
        Ok(run_git_operation(
            &repo_root,
            vec!["pull".to_string(), "--ff-only".to_string()],
        ))
    }

    /// Worktrees belonging to the current repository.
    pub fn worktrees(&self, project_path: &str) -> Result<Vec<GitWorktreeDto>> {
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(Vec::new());
        };
        let output =
            git_output(&repo_root, &["worktree", "list", "--porcelain"]).unwrap_or_default();
        Ok(parse_worktrees(&output))
    }

    /// Compare two refs without changing the working tree. `ahead` is measured
    /// as commits reachable from `target_ref` but not `base_ref`.
    pub fn compare_refs(
        &self,
        project_path: &str,
        base_ref: &str,
        target_ref: &str,
    ) -> Result<GitRefCompareDto> {
        let base_ref = base_ref.trim();
        let target_ref = target_ref.trim();
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitRefCompareDto {
                project_path: project_path.to_string(),
                repo_root: None,
                is_repo: false,
                base_ref: base_ref.to_string(),
                target_ref: target_ref.to_string(),
                merge_base: None,
                ahead: 0,
                behind: 0,
                commits: Vec::new(),
                files: Vec::new(),
                blocked_reason: Some("not a git repository".to_string()),
            });
        };

        let mut blocked_reason = None;
        if base_ref.is_empty() {
            blocked_reason = Some("choose a base ref to compare".to_string());
        } else if target_ref.is_empty() {
            blocked_reason = Some("choose a target ref to compare".to_string());
        } else if !ref_exists(&repo_root, base_ref) {
            blocked_reason = Some("base ref does not exist".to_string());
        } else if !ref_exists(&repo_root, target_ref) {
            blocked_reason = Some("target ref does not exist".to_string());
        }

        let range = format!("{base_ref}...{target_ref}");
        let merge_base = if blocked_reason.is_none() {
            git_output(&repo_root, &["merge-base", base_ref, target_ref])
        } else {
            None
        };
        let (behind, ahead) = if blocked_reason.is_none() {
            let counts = git_stdout_owned(
                &repo_root,
                &[
                    "rev-list".to_string(),
                    "--left-right".to_string(),
                    "--count".to_string(),
                    range.clone(),
                ],
            )
            .unwrap_or_default();
            parse_rev_list_counts(&counts)
        } else {
            (0, 0)
        };
        let commits = if blocked_reason.is_none() {
            let output = git_stdout_owned(
                &repo_root,
                &[
                    "log".to_string(),
                    "--left-right".to_string(),
                    "--cherry-pick".to_string(),
                    "--date=iso-strict".to_string(),
                    "--pretty=format:%m%x1f%h%x1f%H%x1f%an%x1f%ad%x1f%s".to_string(),
                    range.clone(),
                ],
            )
            .unwrap_or_default();
            parse_compare_commits(&output)
        } else {
            Vec::new()
        };
        let files = if blocked_reason.is_none() {
            let output = git_stdout_owned(
                &repo_root,
                &[
                    "diff".to_string(),
                    "--numstat".to_string(),
                    range,
                    "--".to_string(),
                ],
            )
            .unwrap_or_default();
            parse_merge_files(&output)
        } else {
            Vec::new()
        };

        Ok(GitRefCompareDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            is_repo: true,
            base_ref: base_ref.to_string(),
            target_ref: target_ref.to_string(),
            merge_base,
            ahead,
            behind,
            commits,
            files,
            blocked_reason,
        })
    }

    /// Diff between two refs, optionally limited to one repository-relative file.
    pub fn ref_diff(
        &self,
        project_path: &str,
        base_ref: &str,
        target_ref: &str,
        file_path: Option<&str>,
    ) -> Result<GitDiffDto> {
        let base_ref = base_ref.trim();
        let target_ref = target_ref.trim();
        let file_path = file_path.and_then(empty_to_none);
        let display_path = file_path.clone().unwrap_or_default();
        let Some(repo_root) = repo_root(project_path) else {
            return Ok(GitDiffDto {
                project_path: project_path.to_string(),
                repo_root: None,
                file_path: display_path,
                staged: false,
                is_repo: false,
                text: String::new(),
                truncated: false,
            });
        };
        let error = if base_ref.is_empty() {
            Some("choose a base ref to compare")
        } else if target_ref.is_empty() {
            Some("choose a target ref to compare")
        } else if !ref_exists(&repo_root, base_ref) {
            Some("base ref does not exist")
        } else if !ref_exists(&repo_root, target_ref) {
            Some("target ref does not exist")
        } else if file_path
            .as_deref()
            .is_some_and(|path| safe_repo_file_path(&repo_root, path).is_none())
        {
            Some("invalid repository-relative path")
        } else {
            None
        };
        if let Some(error) = error {
            return Ok(GitDiffDto {
                project_path: project_path.to_string(),
                repo_root: Some(repo_root.to_string_lossy().into_owned()),
                file_path: display_path,
                staged: false,
                is_repo: true,
                text: error.to_string(),
                truncated: false,
            });
        }

        let mut args = vec![
            "diff".to_string(),
            "--no-ext-diff".to_string(),
            format!("{base_ref}...{target_ref}"),
            "--".to_string(),
        ];
        if let Some(path) = file_path.as_deref() {
            args.push(path.to_string());
        }
        let raw = git_stdout_owned(&repo_root, &args).unwrap_or_default();
        let (text, truncated) = truncate_diff(raw);
        Ok(GitDiffDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            file_path: display_path,
            staged: false,
            is_repo: true,
            text,
            truncated,
        })
    }

    /// Preview project targets selected for batch commit/upload.
    pub fn batch_commit_preview(
        &self,
        targets: &[String],
    ) -> Result<Vec<GitBatchCommitPreviewItemDto>> {
        Ok(targets
            .iter()
            .map(|project_path| self.batch_preview_item(project_path))
            .collect())
    }

    pub fn batch_commit(
        &self,
        targets: &[GitBatchCommitTargetDto],
        message: &str,
        stage_all: bool,
    ) -> Result<Vec<GitBatchProjectResultDto>> {
        Ok(targets
            .iter()
            .map(|target| self.commit_batch_target(target, message, stage_all))
            .collect())
    }

    pub fn batch_push(&self, targets: &[String]) -> Result<Vec<GitBatchProjectResultDto>> {
        Ok(targets
            .iter()
            .map(|project_path| self.push_batch_target(project_path))
            .collect())
    }

    pub fn batch_commit_and_push(
        &self,
        targets: &[GitBatchCommitTargetDto],
        message: &str,
        stage_all: bool,
    ) -> Result<Vec<GitBatchProjectResultDto>> {
        Ok(targets
            .iter()
            .map(|target| {
                let mut result = self.commit_batch_target(target, message, stage_all);
                if result.ok && result.committed {
                    match self.push_risk_scan(&target.project_path) {
                        Ok(scan) => {
                            if let Some(reason) = scan.blocked_reason {
                                result.ok = false;
                                result.message =
                                    format!("Committed, but push risk scan failed: {reason}");
                                return result;
                            }
                            if !scan.risks.is_empty() {
                                result.ok = false;
                                result.message = format!(
                                    "Committed, but push blocked by {} risk finding(s)",
                                    scan.risks.len()
                                );
                                return result;
                            }
                        }
                        Err(error) => {
                            result.ok = false;
                            result.message =
                                format!("Committed, but push risk scan failed: {error}");
                            return result;
                        }
                    }
                    let push = self.push_batch_target(&target.project_path);
                    result.pushed = push.pushed;
                    result.push_result = push.push_result;
                    result.ok = push.ok;
                    if !push.ok {
                        result.message = format!("Committed, but push failed: {}", push.message);
                    } else {
                        result.message = "Committed and pushed".to_string();
                    }
                }
                result
            })
            .collect())
    }

    fn batch_preview_item(&self, project_path: &str) -> GitBatchCommitPreviewItemDto {
        let status = self.status_for(project_path);
        let changes = self.changes(project_path).ok();
        let staged_files = changes
            .as_ref()
            .map(|changes| {
                changes
                    .files
                    .iter()
                    .filter(|file| file.category == "staged")
                    .count() as u32
            })
            .unwrap_or(0);
        let blocked_reason = batch_commit_blocked_reason(&status, changes.as_ref(), false);
        GitBatchCommitPreviewItemDto {
            project_path: project_path.to_string(),
            repo_root: status.repo_root,
            is_repo: status.is_repo,
            current_branch: status.current_branch,
            files_changed: status.files_changed,
            staged_files,
            additions: status.additions,
            deletions: status.deletions,
            ahead: status.ahead,
            behind: status.behind,
            blocked_reason,
        }
    }

    fn commit_batch_target(
        &self,
        target: &GitBatchCommitTargetDto,
        default_message: &str,
        stage_all: bool,
    ) -> GitBatchProjectResultDto {
        let status = self.status_for(&target.project_path);
        let branch = status.current_branch.clone();
        let message = target
            .message
            .as_deref()
            .and_then(empty_to_none)
            .unwrap_or_else(|| default_message.trim().to_string());
        if message.trim().is_empty() {
            return batch_result_failure(
                &target.project_path,
                branch,
                "commit message must not be empty",
            );
        }
        let changes = self.changes(&target.project_path).ok();
        if let Some(reason) = batch_commit_blocked_reason(&status, changes.as_ref(), stage_all) {
            return batch_result_failure(&target.project_path, branch, &reason);
        }
        if stage_all {
            let Some(repo_root) = repo_root(&target.project_path) else {
                return batch_result_failure(&target.project_path, branch, "not a git repository");
            };
            let stage = run_git_operation(&repo_root, vec!["add".to_string(), "-A".to_string()]);
            if !stage.ok {
                return GitBatchProjectResultDto {
                    project_path: target.project_path.clone(),
                    current_branch: branch,
                    ok: false,
                    committed: false,
                    pushed: false,
                    skipped: false,
                    message: stage.stderr.clone(),
                    commit_result: Some(stage),
                    push_result: None,
                };
            }
        }
        let commit = self
            .commit(&target.project_path, &message)
            .unwrap_or_else(|error| operation_failure("git commit", &error.to_string()));
        GitBatchProjectResultDto {
            project_path: target.project_path.clone(),
            current_branch: branch,
            ok: commit.ok,
            committed: commit.ok,
            pushed: false,
            skipped: false,
            message: if commit.ok {
                "Committed".to_string()
            } else {
                commit.stderr.clone()
            },
            commit_result: Some(commit),
            push_result: None,
        }
    }

    fn push_batch_target(&self, project_path: &str) -> GitBatchProjectResultDto {
        let status = self.status_for(project_path);
        let branch = status.current_branch.clone();
        let push = self
            .push(project_path, None, None)
            .unwrap_or_else(|error| operation_failure("git push", &error.to_string()));
        GitBatchProjectResultDto {
            project_path: project_path.to_string(),
            current_branch: branch,
            ok: push.ok,
            committed: false,
            pushed: push.ok,
            skipped: false,
            message: if push.ok {
                "Pushed".to_string()
            } else {
                push.stderr.clone()
            },
            commit_result: None,
            push_result: Some(push),
        }
    }

    fn status_for(&self, project_path: &str) -> GitProjectStatusDto {
        let Some(repo_root) = repo_root(project_path) else {
            return GitProjectStatusDto {
                project_path: project_path.to_string(),
                repo_root: None,
                repo_id: None,
                is_repo: false,
                current_branch: None,
                detached_head: false,
                upstream: None,
                ahead: 0,
                behind: 0,
                has_changes: false,
                files_changed: 0,
                additions: 0,
                deletions: 0,
                rebase_state: None,
                merge_state: false,
                gh_available: gh_available(project_path),
            };
        };

        let branch = git_output(&repo_root, &["branch", "--show-current"]).unwrap_or_default();
        let detached_head = branch.trim().is_empty();
        let current_branch = if detached_head {
            git_output(&repo_root, &["rev-parse", "--short", "HEAD"])
        } else {
            Some(branch.trim().to_string())
        };
        let upstream = git_output(
            &repo_root,
            &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        );
        let (ahead, behind) = ahead_behind(&repo_root, upstream.as_deref());
        let changes = self.changes(project_path).unwrap_or(GitChangesDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            is_repo: true,
            files: Vec::new(),
            additions: 0,
            deletions: 0,
        });
        let git_dir = git_common_dir(&repo_root).unwrap_or_else(|| repo_root.join(".git"));
        let rebase_state = detect_rebase_state(&git_dir);
        let merge_state = git_dir.join("MERGE_HEAD").exists();

        GitProjectStatusDto {
            project_path: project_path.to_string(),
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            repo_id: Some(repo_id(&repo_root)),
            is_repo: true,
            current_branch: current_branch.and_then(|s| empty_to_none(s.trim())),
            detached_head,
            upstream: upstream.and_then(|s| empty_to_none(s.trim())),
            ahead,
            behind,
            has_changes: !changes.files.is_empty(),
            files_changed: changes.files.len() as u32,
            additions: changes.additions,
            deletions: changes.deletions,
            rebase_state,
            merge_state,
            gh_available: gh_available(project_path),
        }
    }
}

fn repo_root(path: impl AsRef<Path>) -> Option<PathBuf> {
    let path = path.as_ref();
    git_output(path, &["rev-parse", "--show-toplevel"])
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
}

fn git_common_dir(repo_root: &Path) -> Option<PathBuf> {
    let raw = git_output(repo_root, &["rev-parse", "--git-common-dir"])?;
    let p = PathBuf::from(raw);
    Some(if p.is_absolute() {
        p
    } else {
        repo_root.join(p)
    })
}

fn repo_id(repo_root: &Path) -> String {
    let common = git_common_dir(repo_root).unwrap_or_else(|| repo_root.join(".git"));
    let remote =
        git_output(repo_root, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    format!(
        "{}|{}",
        normalize_path(&common),
        remote.trim().to_lowercase()
    )
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn ahead_behind(repo_root: &Path, upstream: Option<&str>) -> (u32, u32) {
    if upstream.is_none() {
        return (0, 0);
    }
    let out = git_output(
        repo_root,
        &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
    )
    .unwrap_or_default();
    let mut parts = out.split_whitespace();
    let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (ahead, behind)
}

fn parse_rev_list_counts(output: &str) -> (u32, u32) {
    let mut parts = output.split_whitespace();
    let left = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let right = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (left, right)
}

fn parse_upstream(upstream: &str) -> Option<(Option<String>, Option<String>)> {
    let (remote, branch) = upstream.split_once('/')?;
    Some((empty_to_none(remote), empty_to_none(branch)))
}

fn parse_upstream_track(track: &str) -> (u32, u32) {
    let mut ahead = 0;
    let mut behind = 0;
    let normalized = track.trim().trim_start_matches('[').trim_end_matches(']');
    for part in normalized.split(',') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("ahead ") {
            ahead = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = part.strip_prefix("behind ") {
            behind = value.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn detect_rebase_state(git_dir: &Path) -> Option<String> {
    if git_dir.join("rebase-merge").exists() {
        Some("merge".to_string())
    } else if git_dir.join("rebase-apply").exists() {
        Some("apply".to_string())
    } else {
        None
    }
}

fn gh_available(cwd: &str) -> bool {
    run_command(Path::new(cwd), "gh", &["--version"])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn git_output(cwd: impl AsRef<Path>, args: &[&str]) -> Option<String> {
    git_stdout(cwd, args).map(|s| s.trim().to_string())
}

fn git_stdout(cwd: impl AsRef<Path>, args: &[&str]) -> Option<String> {
    let output = run_command(cwd.as_ref(), "git", args).ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_stdout_owned(cwd: impl AsRef<Path>, args: &[String]) -> Option<String> {
    let output = run_command_owned(cwd.as_ref(), "git", args).ok()?;
    // `git diff --no-index` can use exit 1 for "different"; regular
    // `git diff` should exit 0. Preserve stdout on either 0 or 1 to keep this
    // helper future-friendly for untracked diff rendering.
    match output.status.code() {
        Some(0) | Some(1) => Some(String::from_utf8_lossy(&output.stdout).to_string()),
        _ => None,
    }
}

fn command_success_owned(cwd: impl AsRef<Path>, args: Vec<String>) -> bool {
    run_command_owned(cwd.as_ref(), "git", &args)
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn ref_exists(repo_root: &Path, name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }
    command_success_owned(
        repo_root,
        vec![
            "rev-parse".to_string(),
            "--verify".to_string(),
            format!("{name}^{{commit}}"),
        ],
    )
}

fn remote_branch_exists(repo_root: &Path, name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.starts_with("refs/remotes/") {
        return false;
    }
    ref_exists(repo_root, &format!("refs/remotes/{name}"))
}

fn valid_branch_name(repo_root: &Path, name: &str) -> bool {
    command_success_owned(
        repo_root,
        vec![
            "check-ref-format".to_string(),
            "--branch".to_string(),
            name.trim().to_string(),
        ],
    )
}

fn normalize_branch_ref(name: &str) -> String {
    let name = name.trim();
    name.strip_prefix("refs/heads/").unwrap_or(name).to_string()
}

fn is_untracked_file(repo_root: &Path, file_path: &str) -> bool {
    let Some(path) = safe_repo_file_path(repo_root, file_path) else {
        return false;
    };
    if !path.is_file() {
        return false;
    }
    !command_success_owned(
        repo_root,
        vec![
            "ls-files".to_string(),
            "--error-unmatch".to_string(),
            "--".to_string(),
            file_path.to_string(),
        ],
    )
}

fn synthetic_untracked_diff(repo_root: &Path, file_path: &str) -> Option<String> {
    let path = safe_repo_file_path(repo_root, file_path)?;
    let content = fs::read_to_string(path).ok()?;
    Some(build_new_file_diff(file_path, &content))
}

fn build_new_file_diff(file_path: &str, content: &str) -> String {
    let normalized_path = file_path.replace('\\', "/");
    let line_count = if content.is_empty() {
        0
    } else {
        content.lines().count()
    };
    let mut out = String::new();
    out.push_str("diff --git a/");
    out.push_str(&normalized_path);
    out.push_str(" b/");
    out.push_str(&normalized_path);
    out.push('\n');
    out.push_str("new file mode 100644\n");
    out.push_str("--- /dev/null\n");
    out.push_str("+++ b/");
    out.push_str(&normalized_path);
    out.push('\n');
    out.push_str("@@ -0,0 +1,");
    out.push_str(&line_count.to_string());
    out.push_str(" @@\n");
    for line in content.lines() {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    if !content.ends_with('\n') && !content.is_empty() {
        out.push_str("\\ No newline at end of file\n");
    }
    out
}

fn safe_repo_file_path(repo_root: &Path, file_path: &str) -> Option<PathBuf> {
    let rel = Path::new(file_path);
    if rel.is_absolute() {
        return None;
    }
    if rel.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return None;
    }
    Some(repo_root.join(rel))
}

fn validate_single_file_hunk_patch(file_path: &str, patch: &str) -> bool {
    if patch.trim().is_empty() || patch.len() > MAX_DIFF_BYTES {
        return false;
    }
    let normalized = file_path.replace('\\', "/");
    if normalized.trim().is_empty()
        || normalized.starts_with('/')
        || normalized.contains("../")
        || normalized == ".."
    {
        return false;
    }
    let mut saw_hunk = false;
    let mut saw_file_header = false;
    for line in patch.lines() {
        if line.starts_with("@@ ") || line.starts_with("@@-") {
            saw_hunk = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let mut parts = rest.split_whitespace();
            let left = parts.next().unwrap_or("");
            let right = parts.next().unwrap_or("");
            if !diff_path_matches(left, &normalized, Some("a/"))
                || !diff_path_matches(right, &normalized, Some("b/"))
                || parts.next().is_some()
            {
                return false;
            }
            saw_file_header = true;
            continue;
        }
        if let Some(path) = line.strip_prefix("--- ") {
            if path != "/dev/null" && !diff_path_matches(path, &normalized, Some("a/")) {
                return false;
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("+++ ") {
            if path != "/dev/null" && !diff_path_matches(path, &normalized, Some("b/")) {
                return false;
            }
        }
    }
    saw_file_header && saw_hunk
}

fn diff_path_matches(path: &str, file_path: &str, prefix: Option<&str>) -> bool {
    let path = path.trim();
    let path = path.split('\t').next().unwrap_or(path);
    let Some(prefix) = prefix else {
        return path == file_path;
    };
    path.strip_prefix(prefix) == Some(file_path)
}

fn run_command(cwd: &Path, program: &str, args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd);
    configure_hidden_process(&mut cmd);
    cmd.output()
}

fn run_command_owned(
    cwd: &Path,
    program: &str,
    args: &[String],
) -> std::io::Result<std::process::Output> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd);
    configure_hidden_process(&mut cmd);
    cmd.output()
}

fn run_git_operation(repo_root: &Path, args: Vec<String>) -> GitOperationResultDto {
    let command = format!("git {}", args.join(" "));
    match run_command_owned(repo_root, "git", &args) {
        Ok(output) => GitOperationResultDto {
            ok: output.status.success(),
            command,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(error) => GitOperationResultDto {
            ok: false,
            command,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

fn run_git_operation_with_stdin(
    repo_root: &Path,
    args: Vec<String>,
    stdin: &str,
) -> GitOperationResultDto {
    let command = format!("git {}", args.join(" "));
    let mut cmd = Command::new("git");
    cmd.args(&args)
        .current_dir(repo_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_hidden_process(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return GitOperationResultDto {
                ok: false,
                command,
                stdout: String::new(),
                stderr: error.to_string(),
            }
        }
    };
    if let Some(mut child_stdin) = child.stdin.take() {
        if let Err(error) = child_stdin.write_all(stdin.as_bytes()) {
            return GitOperationResultDto {
                ok: false,
                command,
                stdout: String::new(),
                stderr: error.to_string(),
            };
        }
    }
    match child.wait_with_output() {
        Ok(output) => GitOperationResultDto {
            ok: output.status.success(),
            command,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(error) => GitOperationResultDto {
            ok: false,
            command,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

fn operation_failure(command: &str, stderr: &str) -> GitOperationResultDto {
    GitOperationResultDto {
        ok: false,
        command: command.to_string(),
        stdout: String::new(),
        stderr: stderr.to_string(),
    }
}

fn batch_commit_blocked_reason(
    status: &GitProjectStatusDto,
    changes: Option<&GitChangesDto>,
    stage_all: bool,
) -> Option<String> {
    if !status.is_repo {
        return Some("not a git repository".to_string());
    }
    if status.merge_state {
        return Some("finish or abort the active merge before committing".to_string());
    }
    if status.rebase_state.is_some() {
        return Some("finish or abort the active rebase before committing".to_string());
    }
    if status.files_changed == 0 {
        return Some("no Git changes to commit".to_string());
    }
    if !stage_all {
        let staged = changes
            .map(|changes| changes.files.iter().any(|file| file.category == "staged"))
            .unwrap_or(false);
        if !staged {
            return Some("no staged files; enable stage all or stage files first".to_string());
        }
    }
    None
}

fn batch_result_failure(
    project_path: &str,
    current_branch: Option<String>,
    message: &str,
) -> GitBatchProjectResultDto {
    GitBatchProjectResultDto {
        project_path: project_path.to_string(),
        current_branch,
        ok: false,
        committed: false,
        pushed: false,
        skipped: false,
        message: message.to_string(),
        commit_result: None,
        push_result: None,
    }
}

fn configure_hidden_process(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

fn empty_to_none(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn parse_numstat(output: &str) -> HashMap<String, (u32, u32)> {
    let mut map = HashMap::new();
    for line in output.lines() {
        let mut parts = line.split('\t');
        let add = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let del = parts.next().unwrap_or("0").parse().unwrap_or(0);
        if let Some(path) = parts.next() {
            map.insert(path.to_string(), (add, del));
        }
    }
    map
}

fn parse_status_z(output: &str, stats: &HashMap<String, (u32, u32)>) -> Vec<GitChangedFileDto> {
    let mut files = Vec::new();
    let mut entries = output.split('\0').filter(|s| !s.is_empty()).peekable();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let xy = &entry[..2];
        let mut path = entry[3..].to_string();
        let mut old_path = None;
        if xy.starts_with('R') || xy.starts_with('C') {
            if let Some(next) = entries.peek().copied() {
                old_path = Some(next.to_string());
                entries.next();
            }
        }
        if path.contains(" -> ") {
            let mut parts = path.splitn(2, " -> ");
            old_path = parts.next().map(|s| s.to_string());
            path = parts.next().unwrap_or("").to_string();
        }
        let (additions, deletions) = stats.get(&path).copied().unwrap_or((0, 0));
        files.push(GitChangedFileDto {
            path,
            old_path,
            status: xy.to_string(),
            category: status_category(xy).to_string(),
            additions,
            deletions,
        });
    }
    files
}

fn status_category(xy: &str) -> &'static str {
    if xy == "??" {
        return "untracked";
    }
    if xy.contains('U') || xy == "AA" || xy == "DD" {
        return "conflicted";
    }
    let bytes = xy.as_bytes();
    if bytes.first().copied().unwrap_or(b' ') != b' ' {
        "staged"
    } else {
        "unstaged"
    }
}

fn category_order(category: &str) -> u8 {
    match category {
        "conflicted" => 0,
        "staged" => 1,
        "unstaged" => 2,
        "untracked" => 3,
        _ => 4,
    }
}

fn parse_worktrees(output: &str) -> Vec<GitWorktreeDto> {
    let mut out = Vec::new();
    let mut current: Option<GitWorktreeDto> = None;
    for line in output.lines() {
        if line.trim().is_empty() {
            if let Some(wt) = current.take() {
                out.push(wt);
            }
            continue;
        }
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                out.push(wt);
            }
            current = Some(GitWorktreeDto {
                path: path.to_string(),
                head: None,
                branch: None,
                detached: false,
                bare: false,
            });
        } else if let Some(wt) = current.as_mut() {
            if let Some(head) = line.strip_prefix("HEAD ") {
                wt.head = Some(head.to_string());
            } else if let Some(branch) = line.strip_prefix("branch ") {
                wt.branch = Some(branch.to_string());
            } else if line == "detached" {
                wt.detached = true;
            } else if line == "bare" {
                wt.bare = true;
            }
        }
    }
    if let Some(wt) = current.take() {
        out.push(wt);
    }
    out
}

fn parse_push_commits(output: &str) -> Vec<GitPushCommitDto> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            let hash = parts.next()?.trim().to_string();
            let full_hash = parts.next()?.trim().to_string();
            if hash.is_empty() || full_hash.is_empty() {
                return None;
            }
            Some(GitPushCommitDto {
                hash,
                full_hash,
                author_name: parts.next().unwrap_or("").to_string(),
                date: parts.next().unwrap_or("").to_string(),
                subject: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn parse_compare_commits(output: &str) -> Vec<GitCompareCommitDto> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\x1f');
            let marker = parts.next()?.trim();
            let side = match marker {
                "<" => "base",
                ">" => "target",
                _ => return None,
            };
            let hash = parts.next()?.trim().to_string();
            let full_hash = parts.next()?.trim().to_string();
            if hash.is_empty() || full_hash.is_empty() {
                return None;
            }
            Some(GitCompareCommitDto {
                side: side.to_string(),
                hash,
                full_hash,
                author_name: parts.next().unwrap_or("").to_string(),
                date: parts.next().unwrap_or("").to_string(),
                subject: parts.next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

fn parse_name_status_paths(output: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in output.lines() {
        let mut parts = line.split('\t');
        let status = parts.next().unwrap_or("");
        if status.starts_with('R') || status.starts_with('C') {
            let _old = parts.next();
        }
        if let Some(path) = parts.next() {
            if !path.trim().is_empty() {
                paths.push(path.to_string());
            }
        }
    }
    paths
}

fn git_blob_size(repo_root: &Path, rev: &str, file_path: &str) -> Option<u64> {
    if file_path.trim().is_empty() {
        return None;
    }
    git_output(
        repo_root,
        &["cat-file", "-s", &format!("{rev}:{file_path}")],
    )?
    .parse()
    .ok()
}

fn risks_for_file_name(file_path: &str) -> Vec<GitPushRiskItemDto> {
    let lower = file_path.to_lowercase();
    let mut risks = Vec::new();
    if lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with("/.env")
        || lower.ends_with("\\.env")
        || lower.contains("/.env.")
        || lower.contains("\\.env.")
    {
        risks.push(GitPushRiskItemDto {
            severity: "high".to_string(),
            category: "secret_file".to_string(),
            title: "Environment file in outgoing commit".to_string(),
            detail: "Environment files often contain secrets. Verify this file is safe to publish."
                .to_string(),
            file_path: Some(file_path.to_string()),
        });
    }
    if lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
    {
        risks.push(GitPushRiskItemDto {
            severity: "high".to_string(),
            category: "secret_file".to_string(),
            title: "Key or certificate file in outgoing commit".to_string(),
            detail:
                "Private keys and certificates should not be pushed unless intentionally public."
                    .to_string(),
            file_path: Some(file_path.to_string()),
        });
    }
    risks
}

fn binary_file_risks(numstat: &str) -> Vec<GitPushRiskItemDto> {
    numstat
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let additions = parts.next().unwrap_or("");
            let deletions = parts.next().unwrap_or("");
            let path = parts.next().unwrap_or("");
            if additions == "-" && deletions == "-" && !path.is_empty() {
                Some(GitPushRiskItemDto {
                    severity: "medium".to_string(),
                    category: "binary_file".to_string(),
                    title: "Binary file changed".to_string(),
                    detail: "Binary files are harder to review. Verify this change is expected."
                        .to_string(),
                    file_path: Some(path.to_string()),
                })
            } else {
                None
            }
        })
        .collect()
}

fn scan_patch_risks(patch: &str) -> Vec<GitPushRiskItemDto> {
    let mut risks = Vec::new();
    let mut current_file: Option<String> = None;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current_file = Some(path.to_string());
            continue;
        }
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        let added = &line[1..];
        if looks_like_secret(added) {
            risks.push(GitPushRiskItemDto {
                severity: "high".to_string(),
                category: "secret".to_string(),
                title: "Possible secret added".to_string(),
                detail: redact_risk_line(added),
                file_path: current_file.clone(),
            });
        } else if looks_like_debug_log(added) {
            risks.push(GitPushRiskItemDto {
                severity: "low".to_string(),
                category: "debug_log".to_string(),
                title: "Debug output added".to_string(),
                detail: added.trim().chars().take(160).collect(),
                file_path: current_file.clone(),
            });
        }
    }
    risks
}

fn looks_like_secret(line: &str) -> bool {
    let lower = line.to_lowercase();
    if lower.contains("-----begin ") && lower.contains("private key") {
        return true;
    }
    let keys = [
        "api_key",
        "apikey",
        "access_token",
        "auth_token",
        "secret_key",
        "client_secret",
        "password",
        "passwd",
        "token",
    ];
    let has_key = keys.iter().any(|key| lower.contains(key));
    let has_assignment = lower.contains('=') || lower.contains(':');
    let has_long_value = line
        .split(['=', ':'])
        .nth(1)
        .map(|value| value.trim_matches(['"', '\'', ' ', ',']).len() >= 12)
        .unwrap_or(false);
    has_key && has_assignment && has_long_value
}

fn looks_like_debug_log(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("console.log(")
        || lower.contains("debugger;")
        || lower.contains("println!(")
        || lower.contains("dbg!(")
        || lower.contains("todo!(")
}

fn redact_risk_line(line: &str) -> String {
    let trimmed = line.trim();
    if let Some((key, _)) = trimmed.split_once('=') {
        return format!("{}=<redacted>", key.trim());
    }
    if let Some((key, _)) = trimmed.split_once(':') {
        return format!("{}: <redacted>", key.trim());
    }
    "<redacted possible secret>".to_string()
}

fn dedupe_risks(risks: &mut Vec<GitPushRiskItemDto>) {
    let mut seen = std::collections::HashSet::new();
    risks.retain(|risk| {
        seen.insert(format!(
            "{}:{}:{}:{}",
            risk.severity,
            risk.category,
            risk.title,
            risk.file_path.as_deref().unwrap_or("")
        ))
    });
}

fn build_commit_message_draft(files: &[GitChangedFileDto]) -> (String, String) {
    let change_type = infer_commit_type(files);
    let scope = infer_commit_scope(files);
    let action = infer_commit_action(files);
    let file_label = summarize_file_targets(files);
    let mut title = if let Some(scope) = scope {
        format!("{change_type}({scope}): {action} {file_label}")
    } else {
        format!("{change_type}: {action} {file_label}")
    };
    title = compact_spaces(&title);
    title = truncate_title(title, 72);

    let additions: u32 = files.iter().map(|file| file.additions).sum();
    let deletions: u32 = files.iter().map(|file| file.deletions).sum();
    let mut lines = Vec::new();
    lines.push(format!(
        "- Update {} file(s) (+{}, -{})",
        files.len(),
        additions,
        deletions
    ));
    for file in files.iter().take(6) {
        lines.push(format!(
            "- {} {} (+{}, -{})",
            file.status.trim(),
            file.path,
            file.additions,
            file.deletions
        ));
    }
    if files.len() > 6 {
        lines.push(format!("- Include {} more file(s)", files.len() - 6));
    }
    (title, lines.join("\n"))
}

fn infer_commit_type(files: &[GitChangedFileDto]) -> &'static str {
    if files.iter().all(|file| is_doc_path(&file.path)) {
        return "docs";
    }
    if files.iter().any(|file| is_test_path(&file.path)) {
        return "test";
    }
    if files.iter().all(|file| is_config_path(&file.path)) {
        return "chore";
    }
    if files
        .iter()
        .any(|file| file.status.contains('A') || file.status == "??")
    {
        return "feat";
    }
    if files.iter().any(|file| file.status.contains('D')) {
        return "fix";
    }
    "chore"
}

fn infer_commit_scope(files: &[GitChangedFileDto]) -> Option<String> {
    let mut parts = files
        .iter()
        .filter_map(|file| file.path.split(['/', '\\']).next())
        .filter(|part| !part.is_empty());
    let first = parts.next()?.to_string();
    if parts.all(|part| part == first) {
        Some(scope_from_path_part(&first))
    } else {
        None
    }
}

fn infer_commit_action(files: &[GitChangedFileDto]) -> &'static str {
    if files
        .iter()
        .all(|file| file.status == "??" || file.status.contains('A'))
    {
        "add"
    } else if files.iter().all(|file| file.status.contains('D')) {
        "remove"
    } else {
        "update"
    }
}

fn summarize_file_targets(files: &[GitChangedFileDto]) -> String {
    if files.len() == 1 {
        return display_file_name(&files[0].path);
    }
    if let Some(scope) = infer_commit_scope(files) {
        return format!("{scope} files");
    }
    format!("{} files", files.len())
}

fn display_file_name(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .last()
        .unwrap_or(path)
        .to_string()
}

fn scope_from_path_part(part: &str) -> String {
    part.trim_matches('.')
        .replace(['_', ' '], "-")
        .to_lowercase()
}

fn compact_spaces(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_title(mut title: String, max_len: usize) -> String {
    if title.len() <= max_len {
        return title;
    }
    let mut end = max_len.saturating_sub(3);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    title.truncate(end);
    title.push_str("...");
    title
}

fn is_doc_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.ends_with(".md")
        || path.ends_with(".mdx")
        || path.ends_with(".rst")
        || path.starts_with("docs/")
        || path.starts_with("docs\\")
}

fn is_test_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.contains("/test")
        || path.contains("\\test")
        || path.contains(".test.")
        || path.contains(".spec.")
        || path.ends_with("_test.rs")
}

fn is_config_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.ends_with(".json")
        || path.ends_with(".toml")
        || path.ends_with(".yaml")
        || path.ends_with(".yml")
        || path.ends_with(".lock")
        || path.ends_with(".config.js")
        || path.ends_with(".config.ts")
}

fn parse_merge_files(output: &str) -> Vec<GitCommitFileDto> {
    output
        .lines()
        .filter_map(|line| {
            let mut cols = line.split('\t');
            let additions = cols.next().unwrap_or("0").parse().unwrap_or(0);
            let deletions = cols.next().unwrap_or("0").parse().unwrap_or(0);
            let path = cols.next()?.to_string();
            if path.is_empty() {
                return None;
            }
            Some(GitCommitFileDto {
                path,
                old_path: None,
                status: "M".to_string(),
                additions,
                deletions,
            })
        })
        .collect()
}

fn parse_log_entries(output: &str) -> Vec<GitLogEntryDto> {
    let mut entries = Vec::new();
    for raw_record in output.split('\x1e') {
        let record = raw_record.trim_start_matches('\n');
        if record.trim().is_empty() {
            continue;
        }
        let mut lines = record.lines();
        let Some(header) = lines.next() else {
            continue;
        };
        let mut parts = header.split('\x1f');
        let hash = parts.next().unwrap_or("").trim().to_string();
        let full_hash = parts.next().unwrap_or("").trim().to_string();
        if hash.is_empty() || full_hash.is_empty() {
            continue;
        }
        let parents = parts
            .next()
            .unwrap_or("")
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        let author_name = parts.next().unwrap_or("").to_string();
        let author_email = parts.next().unwrap_or("").to_string();
        let date = parts.next().unwrap_or("").to_string();
        let refs = parts
            .next()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        let subject = parts.next().unwrap_or("").to_string();
        let mut files = Vec::new();
        for line in lines {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            let mut cols = line.split('\t');
            let additions = cols.next().unwrap_or("0").parse().unwrap_or(0);
            let deletions = cols.next().unwrap_or("0").parse().unwrap_or(0);
            let Some(path) = cols.next() else {
                continue;
            };
            files.push(GitCommitFileDto {
                path: path.to_string(),
                old_path: None,
                status: "M".to_string(),
                additions,
                deletions,
            });
        }
        entries.push(GitLogEntryDto {
            hash,
            full_hash,
            parents,
            author_name,
            author_email,
            date,
            refs,
            subject,
            files,
        });
    }
    entries
}

fn truncate_diff(mut text: String) -> (String, bool) {
    if text.len() <= MAX_DIFF_BYTES {
        return (text, false);
    }
    let mut end = MAX_DIFF_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("\n... diff truncated ...\n");
    (text, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat() {
        let stats = parse_numstat("2\t1\tsrc/a.rs\n-\t-\tbin.dat\n");
        assert_eq!(stats.get("src/a.rs"), Some(&(2, 1)));
        assert_eq!(stats.get("bin.dat"), Some(&(0, 0)));
    }

    #[test]
    fn parses_status_z() {
        let stats = HashMap::from([("src/a.rs".to_string(), (3, 1))]);
        let files = parse_status_z(" M src/a.rs\0?? new.txt\0", &stats);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].category, "unstaged");
        assert_eq!(files[0].additions, 3);
        assert_eq!(files[1].category, "untracked");
    }

    #[test]
    fn parses_worktree_porcelain() {
        let out = "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo-wt\nHEAD def\ndetached\n";
        let wts = parse_worktrees(out);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[0].branch.as_deref(), Some("refs/heads/main"));
        assert!(wts[1].detached);
    }

    #[test]
    fn validates_single_file_hunk_patch() {
        let patch = "diff --git a/src/app.rs b/src/app.rs\nindex 111..222 100644\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1,2 +1,2 @@\n fn main() {\n-    old();\n+    new();\n }\n";
        assert!(validate_single_file_hunk_patch("src/app.rs", patch));
    }

    #[test]
    fn rejects_cross_file_hunk_patch() {
        let patch = "diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/src/other.rs b/src/other.rs\n--- a/src/other.rs\n+++ b/src/other.rs\n@@ -1 +1 @@\n-old\n+new\n";
        assert!(!validate_single_file_hunk_patch("src/app.rs", patch));
    }

    #[test]
    fn rejects_header_only_hunk_patch() {
        let patch = "diff --git a/src/app.rs b/src/app.rs\n--- a/src/app.rs\n+++ b/src/app.rs\n";
        assert!(!validate_single_file_hunk_patch("src/app.rs", patch));
    }

    #[test]
    fn truncates_diff_on_char_boundary() {
        let (text, truncated) = truncate_diff("a".repeat(MAX_DIFF_BYTES + 10));
        assert!(truncated);
        assert!(text.ends_with("... diff truncated ...\n"));
    }

    #[test]
    fn parses_upstream_remote_and_branch() {
        let (remote, branch) = parse_upstream("origin/feature/git-workbench").unwrap();
        assert_eq!(remote.as_deref(), Some("origin"));
        assert_eq!(branch.as_deref(), Some("feature/git-workbench"));
        assert!(parse_upstream("main").is_none());
    }

    #[test]
    fn parses_branch_upstream_tracking_counts() {
        assert_eq!(parse_upstream_track("[ahead 2]"), (2, 0));
        assert_eq!(parse_upstream_track("[behind 3]"), (0, 3));
        assert_eq!(parse_upstream_track("[ahead 2, behind 3]"), (2, 3));
        assert_eq!(parse_upstream_track(""), (0, 0));
    }

    #[test]
    fn parses_push_commits() {
        let raw = "abc\x1ffull-abc\x1fAda\x1f2026-01-01T00:00:00Z\x1fAdd git push\n";
        let commits = parse_push_commits(raw);
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "abc");
        assert_eq!(commits[0].subject, "Add git push");
    }

    #[test]
    fn parses_rev_list_counts_as_left_and_right() {
        assert_eq!(parse_rev_list_counts("2\t3\n"), (2, 3));
        assert_eq!(parse_rev_list_counts("bad"), (0, 0));
    }

    #[test]
    fn parses_compare_commits_with_sides() {
        let raw = "<\x1faaa\x1ffull-a\x1fAda\x1f2026-01-01T00:00:00Z\x1fBase only\n>\x1fbbb\x1ffull-b\x1fBen\x1f2026-01-02T00:00:00Z\x1fTarget only\n";
        let commits = parse_compare_commits(raw);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].side, "base");
        assert_eq!(commits[0].hash, "aaa");
        assert_eq!(commits[1].side, "target");
        assert_eq!(commits[1].subject, "Target only");
    }

    #[test]
    fn normalizes_local_branch_refs_for_switch() {
        assert_eq!(
            normalize_branch_ref("refs/heads/feature/git"),
            "feature/git"
        );
        assert_eq!(normalize_branch_ref("origin/main"), "origin/main");
        assert_eq!(normalize_branch_ref("  main  "), "main");
    }

    #[test]
    fn builds_commit_message_draft_for_single_added_file() {
        let files = vec![GitChangedFileDto {
            path: "apps/desktop/src/components/git/GitAssistant.tsx".to_string(),
            old_path: None,
            status: "A ".to_string(),
            category: "staged".to_string(),
            additions: 25,
            deletions: 0,
        }];
        let (title, body) = build_commit_message_draft(&files);
        assert_eq!(title, "feat(apps): add GitAssistant.tsx");
        assert!(body.contains("A apps/desktop/src/components/git/GitAssistant.tsx"));
    }

    #[test]
    fn builds_commit_message_draft_for_docs_scope() {
        let files = vec![
            GitChangedFileDto {
                path: "docs/index.md".to_string(),
                old_path: None,
                status: " M".to_string(),
                category: "unstaged".to_string(),
                additions: 3,
                deletions: 1,
            },
            GitChangedFileDto {
                path: "docs/guide.md".to_string(),
                old_path: None,
                status: " M".to_string(),
                category: "unstaged".to_string(),
                additions: 2,
                deletions: 0,
            },
        ];
        let (title, body) = build_commit_message_draft(&files);
        assert_eq!(title, "docs(docs): update docs files");
        assert!(body.contains("Update 2 file(s) (+5, -1)"));
    }

    #[test]
    fn truncates_commit_message_title() {
        let title = truncate_title("x".repeat(100), 72);
        assert_eq!(title.len(), 72);
        assert!(title.ends_with("..."));
    }

    #[test]
    fn builds_synthetic_new_file_diff() {
        let diff = build_new_file_diff("src\\new.rs", "fn main() {}\nprintln!();");
        assert!(diff.contains("diff --git a/src/new.rs b/src/new.rs"));
        assert!(diff.contains("new file mode 100644"));
        assert!(diff.contains("@@ -0,0 +1,2 @@"));
        assert!(diff.contains("+fn main() {}"));
        assert!(diff.contains("\\ No newline at end of file"));
    }

    #[test]
    fn parses_name_status_paths_with_renames() {
        let paths = parse_name_status_paths("M\tsrc/a.rs\nR100\told.rs\tnew.rs\n");
        assert_eq!(paths, vec!["src/a.rs", "new.rs"]);
    }

    #[test]
    fn detects_push_patch_risks() {
        let patch =
            "+++ b/src/app.ts\n+const api_key = \"123456789012345\";\n+console.log('debug');\n";
        let risks = scan_patch_risks(patch);
        assert_eq!(risks.len(), 2);
        assert_eq!(risks[0].category, "secret");
        assert_eq!(risks[0].file_path.as_deref(), Some("src/app.ts"));
        assert_eq!(risks[1].category, "debug_log");
    }

    #[test]
    fn detects_binary_file_risks() {
        let risks = binary_file_risks("-\t-\tassets/logo.png\n2\t1\tsrc/main.rs\n");
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].file_path.as_deref(), Some("assets/logo.png"));
    }

    #[test]
    fn detects_secret_file_names() {
        let risks = risks_for_file_name(".env.production");
        assert_eq!(risks.len(), 1);
        assert_eq!(risks[0].severity, "high");
    }

    #[test]
    fn parses_merge_files() {
        let files = parse_merge_files("4\t2\tsrc/a.rs\n-\t-\tassets/logo.png\n");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/a.rs");
        assert_eq!(files[0].additions, 4);
        assert_eq!(files[1].additions, 0);
    }

    #[test]
    fn rejects_unsafe_repo_paths() {
        let repo = Path::new("/repo");
        assert!(safe_repo_file_path(repo, "src/main.rs").is_some());
        assert!(safe_repo_file_path(repo, "../secret.txt").is_none());
        assert!(safe_repo_file_path(repo, "/tmp/secret.txt").is_none());
    }

    #[test]
    fn parses_log_entries() {
        let raw = "\x1eabc\x1ffull\x1fparent\x1fAda\x1fada@example.test\x1f2026-01-01T00:00:00Z\x1fHEAD -> main, tag: v1\x1fInitial\n2\t1\tsrc/main.rs\n\n";
        let entries = parse_log_entries(raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hash, "abc");
        assert_eq!(entries[0].refs, vec!["HEAD -> main", "tag: v1"]);
        assert_eq!(entries[0].files[0].path, "src/main.rs");
        assert_eq!(entries[0].files[0].additions, 2);
    }
}
