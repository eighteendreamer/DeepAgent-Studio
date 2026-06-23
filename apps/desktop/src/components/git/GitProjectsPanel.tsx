import { useCallback, useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  gitBatchCommit,
  gitBatchPush,
  gitBatchCommitPreview,
  gitCompareRefs,
  gitFetch,
  gitPullUpdate,
  gitPush,
  gitPushPreview,
  gitPushRiskScan,
  gitRefDiff,
} from "../../api";
import { useGitProjects } from "../../hooks/useGitProjects";
import type {
  GitBatchCommitPreviewItem,
  GitBatchProjectResult,
  GitDiff,
  GitOperationResult,
  GitProjectStatus,
  GitPushRiskScan,
  GitRefCompare,
  Project,
} from "../../types";
import { getGitUiSettings } from "./gitSettings";
import { DiffText } from "./GitDiffViewer";

interface Props {
  activeProjectPath: string;
  onRefresh?: () => Promise<void> | void;
}

interface ProjectRow {
  project: Project | null;
  status: GitProjectStatus;
}

interface BranchGroup {
  key: string;
  branch: string;
  rows: ProjectRow[];
}

interface RepoGroup {
  key: string;
  label: string;
  branches: BranchGroup[];
}

interface BatchPushRiskSummary {
  branch: string;
  scannedTargets: number;
  risks: Array<{
    projectPath: string;
    severity: string;
    category: string;
    title: string;
    detail: string;
    filePath: string | null;
  }>;
}

export function GitProjectsPanel({ activeProjectPath, onRefresh }: Props) {
  const { loading, projects, statuses, error, refresh } = useGitProjects();
  const [busyGroup, setBusyGroup] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [operationResult, setOperationResult] = useState<string | null>(null);
  const [pushRiskSummary, setPushRiskSummary] = useState<BatchPushRiskSummary | null>(null);
  const [compareResult, setCompareResult] = useState<{
    repoKey: string;
    title: string;
    result: GitRefCompare;
  } | null>(null);
  const [selectedTargets, setSelectedTargets] = useState<Record<string, boolean>>({});
  const [batchDialog, setBatchDialog] = useState<{
    mode: "commit" | "commit-push" | "step";
    targets: ProjectRow[];
  } | null>(null);

  const groups = useMemo(() => groupStatuses(projects, statuses), [projects, statuses]);
  const selectedRows = useMemo(() => {
    const selected = new Set(Object.entries(selectedTargets).filter(([, value]) => value).map(([path]) => path));
    return statuses
      .filter((status) => selected.has(status.project_path))
      .map((status) => ({
        project: projects.find((project) => project.path === status.project_path) ?? null,
        status,
      }));
  }, [projects, selectedTargets, statuses]);

  const setRowSelected = (projectPath: string, selected: boolean) => {
    setSelectedTargets((current) => ({
      ...current,
      [projectPath]: selected,
    }));
  };

  const openBatchDialog = (mode: "commit" | "commit-push" | "step", rows: ProjectRow[]) => {
    const unique = uniqueProjectRows(rows);
    if (unique.length === 0) {
      setOperationError("No project targets selected.");
      return;
    }
    setOperationError(null);
    setOperationResult(null);
    setBatchDialog({ mode, targets: unique });
  };

  const refreshAll = async () => {
    setOperationError(null);
    setOperationResult(null);
    await refresh();
    await onRefresh?.();
  };

  const pushBranchGroup = async (group: BranchGroup) => {
    const candidates = uniquePushCandidates(group.rows);
    if (candidates.length === 0) {
      setOperationError("No unique project targets in this branch group.");
      return;
    }
    setBusyGroup(group.key);
    setOperationError(null);
    setOperationResult(null);
    setPushRiskSummary(null);
    try {
      const previews = await Promise.all(
        candidates.map(async (row) => ({ row, preview: await gitPushPreview(row.status.project_path) })),
      );
      const pushable = previews.filter(({ preview }) => !preview.blocked_reason && preview.ahead > 0);
      const blocked = previews.filter(({ preview }) => preview.blocked_reason);
      if (pushable.length === 0) {
        setOperationError(blocked[0]?.preview.blocked_reason ?? "No outgoing commits to push.");
        return;
      }
      const riskScans = await Promise.all(
        pushable.map(async ({ row }) => ({
          row,
          scan: await gitPushRiskScan(row.status.project_path),
        })),
      );
      const scanBlocked = riskScans.find(({ scan }) => scan.blocked_reason);
      if (scanBlocked) {
        setOperationError(scanBlocked.scan.blocked_reason ?? "Push risk scan could not run.");
        return;
      }
      const risks = flattenBatchPushRisks(riskScans);
      setPushRiskSummary({
        branch: group.branch,
        scannedTargets: riskScans.length,
        risks,
      });
      const highRiskCount = risks.filter((risk) => risk.severity === "high").length;
      if (highRiskCount > 0) {
        const okRisk = window.confirm(
          `Push risk scan found ${highRiskCount} high-risk finding(s) across ${riskScans.length} target(s). Continue anyway?`,
        );
        if (!okRisk) return;
      } else if (risks.length > 0) {
        const okRisk = window.confirm(
          `Push risk scan found ${risks.length} finding(s) across ${riskScans.length} target(s). Continue?`,
        );
        if (!okRisk) return;
      }
      const totalCommits = pushable.reduce((sum, item) => sum + item.preview.ahead, 0);
      if (getGitUiSettings().confirmBeforePush) {
        const ok = window.confirm(
          `Push ${totalCommits} outgoing commit(s) from ${pushable.length} project target(s) on ${group.branch}?`,
        );
        if (!ok) return;
      }
      const results: GitOperationResult[] = [];
      for (const { row, preview } of pushable) {
        results.push(await gitPush(row.status.project_path, preview.remote, preview.remote_branch));
      }
      const failed = results.find((result) => !result.ok);
      if (failed) {
        setOperationError(failed.stderr || failed.stdout || "One push failed.");
      } else {
        setOperationResult(`Pushed ${pushable.length} target(s).`);
      }
      await refreshAll();
    } catch (err) {
      setOperationError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyGroup(null);
    }
  };

  const fetchAll = async () => {
    const candidates = uniqueRepoCandidates(statuses);
    if (candidates.length === 0) {
      setOperationError("No Git repositories to fetch.");
      return;
    }
    const ok = window.confirm(`Fetch ${candidates.length} unique repo(s)?`);
    if (!ok) return;
    setBusyGroup("fetch-all");
    setOperationError(null);
    setOperationResult(null);
    try {
      const results: GitOperationResult[] = [];
      for (const status of candidates) {
        results.push(await gitFetch(status.project_path, true));
      }
      const failed = results.find((result) => !result.ok);
      if (failed) {
        setOperationError(failed.stderr || failed.stdout || "One fetch failed.");
      } else {
        setOperationResult(`Fetched ${candidates.length} repo(s).`);
      }
      await refreshAll();
    } catch (err) {
      setOperationError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyGroup(null);
    }
  };

  const updateBranchGroup = async (group: BranchGroup) => {
    const allCandidates = uniqueUpdateCandidates(group.rows);
    if (allCandidates.length === 0) {
      setOperationError("No project targets in this branch group can be updated.");
      return;
    }
    const blocked = allCandidates.find((row) => updateRiskBlockedReason(row.status));
    if (blocked) {
      setOperationError(updateRiskBlockedReason(blocked.status));
      return;
    }
    const candidates = allCandidates.filter((row) => row.status.behind > 0);
    if (candidates.length === 0) {
      setOperationError("No behind commits to update.");
      return;
    }
    const ok = window.confirm(`Fast-forward update ${candidates.length} project target(s) on ${group.branch}?`);
    if (!ok) return;
    setBusyGroup(`update:${group.key}`);
    setOperationError(null);
    setOperationResult(null);
    try {
      const results: GitOperationResult[] = [];
      for (const row of candidates) {
        results.push(await gitPullUpdate(row.status.project_path));
      }
      const failed = results.find((result) => !result.ok);
      if (failed) {
        setOperationError(failed.stderr || failed.stdout || "One update failed.");
      } else {
        setOperationResult(`Updated ${candidates.length} target(s).`);
      }
      await refreshAll();
    } catch (err) {
      setOperationError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyGroup(null);
    }
  };

  const compareRepoRefs = async (repo: RepoGroup, baseRef: string, targetRef: string) => {
    const candidate = repo.branches[0]?.rows[0]?.status;
    if (!candidate) {
      setOperationError("No project target in this repository group.");
      return;
    }
    setBusyGroup(`compare:${repo.key}`);
    setOperationError(null);
    setOperationResult(null);
    try {
      const result = await gitCompareRefs(candidate.project_path, baseRef, targetRef);
      if (result.blocked_reason) {
        setCompareResult(null);
        setOperationError(result.blocked_reason);
        return;
      }
      setCompareResult({
        repoKey: repo.key,
        title: `${baseRef} ... ${targetRef}`,
        result,
      });
    } catch (err) {
      setOperationError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusyGroup(null);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-white">
      <div className="flex h-11 flex-shrink-0 items-center justify-between border-b border-border-theme px-4">
        <div className="flex min-w-0 items-center">
          <FontAwesomeIcon icon={["fas", "folder-tree"]} className="mr-2 text-text-secondary" />
          <span className="text-[14px] font-medium text-text-base">分支管理</span>
          <span className="ml-2 text-[12px] text-text-secondary">
            {statuses.filter((status) => status.is_repo).length} 个 Git 项目
          </span>
        </div>
        <div className="flex items-center gap-1.5">
          <button
            type="button"
            onClick={() => openBatchDialog("commit", selectedRows)}
            disabled={loading || busyGroup !== null || selectedRows.length === 0}
            className="inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium text-text-secondary hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "check"]} className="mr-1.5 text-[11px]" />
            提交选中
          </button>
          <button
            type="button"
            onClick={() => openBatchDialog("commit-push", selectedRows)}
            disabled={loading || busyGroup !== null || selectedRows.length === 0}
            className="inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium text-text-secondary hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "upload"]} className="mr-1.5 text-[11px]" />
            提交并上传
          </button>
          <button
            type="button"
            onClick={() => openBatchDialog("step", selectedRows)}
            disabled={loading || busyGroup !== null || selectedRows.length === 0}
            className="inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium text-text-secondary hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-1.5 text-[11px]" />
            逐个处理
          </button>
          <button
            type="button"
            onClick={() => void fetchAll()}
            disabled={loading || busyGroup !== null}
            className="inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium text-text-secondary hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "download"]} className="mr-1.5 text-[11px]" />
            {busyGroup === "fetch-all" ? "拉取中..." : "全部拉取"}
          </button>
          <button
            type="button"
            onClick={() => void refreshAll()}
            disabled={loading || busyGroup !== null}
            className="inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium text-text-secondary hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
          >
            <FontAwesomeIcon icon={["fas", "rotate-right"]} className="mr-1.5 text-[11px]" />
            刷新
          </button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto bg-gray-50/60 p-4">
        <div className="mx-auto flex max-w-5xl flex-col gap-3">
          {error && <Message tone="error">{error}</Message>}
          {operationError && <Message tone="error">{operationError}</Message>}
          {operationResult && <Message tone="success">{operationResult}</Message>}
          {pushRiskSummary && <BatchPushRiskCard summary={pushRiskSummary} />}
          {batchDialog && (
            <BatchCommitPanel
              mode={batchDialog.mode}
              targets={batchDialog.targets}
              onClose={() => setBatchDialog(null)}
              onDone={async () => {
                await refreshAll();
              }}
            />
          )}

          {loading ? (
            <div className="text-[13px] text-text-secondary">正在读取 Git 分支状态...</div>
          ) : groups.length === 0 ? (
            <div className="rounded-lg border border-border-theme bg-white p-4 text-[13px] text-text-secondary">
              没有检测到已打开的 Git 项目。
            </div>
          ) : (
            groups.map((repo) => (
              <div key={repo.key} className="overflow-hidden rounded-lg border border-border-theme bg-white">
                <div className="border-b border-border-theme px-3 py-2">
                  <div className="grid gap-2 md:grid-cols-[minmax(0,1fr)_auto] md:items-center">
                    <div className="min-w-0">
                      <div className="truncate text-[13px] font-medium text-text-base">{repo.label}</div>
                      <div className="mt-0.5 text-[11px] text-text-secondary">
                        {repo.branches.length} 个分支组
                      </div>
                    </div>
                    <RepoCompareForm
                      repo={repo}
                      busy={busyGroup !== null}
                      active={busyGroup === `compare:${repo.key}`}
                      onCompare={(baseRef, targetRef) => void compareRepoRefs(repo, baseRef, targetRef)}
                    />
                  </div>
                </div>
                {compareResult?.repoKey === repo.key && (
                  <CompareResultView title={compareResult.title} result={compareResult.result} />
                )}
                <div className="divide-y divide-border-theme">
                  {repo.branches.map((branch) => (
                    <div key={branch.key}>
                      <div className="flex flex-wrap items-center justify-between gap-2 bg-gray-50 px-3 py-2">
                        <div className="flex min-w-0 items-center">
                          <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-text-secondary" />
                          <span className="truncate text-[13px] font-medium text-text-base">
                            {branch.branch}
                          </span>
                          <span className="ml-2 text-[11px] text-text-secondary">
                            {branch.rows.length} 个项目
                          </span>
                        </div>
                        <div className="flex items-center gap-1.5">
                          <button
                            type="button"
                            disabled={busyGroup !== null}
                            onClick={() => openBatchDialog("commit", branch.rows)}
                            className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2.5 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                          >
                            <FontAwesomeIcon icon={["fas", "check"]} className="mr-1.5 text-[11px]" />
                            提交
                          </button>
                          <button
                            type="button"
                            disabled={busyGroup !== null}
                            onClick={() => openBatchDialog("commit-push", branch.rows)}
                            className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2.5 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                          >
                            <FontAwesomeIcon icon={["fas", "upload"]} className="mr-1.5 text-[11px]" />
                            提交并上传
                          </button>
                          <button
                            type="button"
                            disabled={busyGroup !== null}
                            onClick={() => openBatchDialog("step", branch.rows)}
                            className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2.5 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                          >
                            <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-1.5 text-[11px]" />
                            逐个处理
                          </button>
                          <button
                            type="button"
                            disabled={busyGroup !== null}
                            onClick={() => void updateBranchGroup(branch)}
                            className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2.5 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                          >
                            <FontAwesomeIcon icon={["fas", "download"]} className="mr-1.5 text-[11px]" />
                            {busyGroup === `update:${branch.key}` ? "更新中..." : "更新"}
                          </button>
                          <button
                            type="button"
                            disabled={busyGroup !== null}
                            onClick={() => void pushBranchGroup(branch)}
                            className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2.5 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                          >
                            <FontAwesomeIcon icon={["fas", "upload"]} className="mr-1.5 text-[11px]" />
                            {busyGroup === branch.key ? "上传中..." : "上传"}
                          </button>
                        </div>
                      </div>
                      <div className="divide-y divide-border-theme">
                        {branch.rows.map((row) => (
                          <ProjectStatusRow
                            key={row.status.project_path}
                            row={row}
                            active={row.status.project_path === activeProjectPath}
                            selected={!!selectedTargets[row.status.project_path]}
                            onSelectedChange={(selected) => setRowSelected(row.status.project_path, selected)}
                          />
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

function ProjectStatusRow({
  row,
  active,
  selected,
  onSelectedChange,
}: {
  row: ProjectRow;
  active: boolean;
  selected: boolean;
  onSelectedChange: (selected: boolean) => void;
}) {
  const { status, project } = row;
  return (
    <div className="grid gap-2 px-3 py-2 text-[12px] sm:grid-cols-[minmax(0,1fr)_auto]">
      <div className="flex min-w-0 items-start gap-2">
        <input
          type="checkbox"
          checked={selected}
          onChange={(event) => onSelectedChange(event.target.checked)}
          className="mt-1 h-3.5 w-3.5 rounded border-border-theme"
          aria-label={`Select ${status.project_path}`}
        />
        <div className="min-w-0">
          <div className="flex min-w-0 items-center">
            <span className="truncate font-medium text-text-base">
              {project?.name ?? status.project_path.split(/[\\/]/).pop() ?? status.project_path}
            </span>
            {active && <span className="ml-2 rounded bg-blue-50 px-1.5 py-0.5 text-[10px] text-blue-600">当前</span>}
          </div>
          <div className="mt-0.5 truncate text-[11px] text-text-secondary" title={status.project_path}>
            {status.project_path}
          </div>
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2 text-[11px] text-text-secondary">
        <Metric label="变更" value={`${status.files_changed}`} tone={status.has_changes ? "warn" : "muted"} />
        <Metric label="ahead" value={`${status.ahead}`} tone={status.ahead > 0 ? "good" : "muted"} />
        <Metric label="behind" value={`${status.behind}`} tone={status.behind > 0 ? "warn" : "muted"} />
        {status.merge_state && <Badge tone="warn">merge</Badge>}
        {status.rebase_state && <Badge tone="warn">rebase</Badge>}
        {status.detached_head && <Badge tone="warn">detached</Badge>}
      </div>
    </div>
  );
}

function RepoCompareForm({
  repo,
  busy,
  active,
  onCompare,
}: {
  repo: RepoGroup;
  busy: boolean;
  active: boolean;
  onCompare: (baseRef: string, targetRef: string) => void;
}) {
  const refs = repoRefOptions(repo);
  if (refs.length === 0) return null;
  const defaultBase = refs.find((ref) => ref === "origin/main") ?? refs.find((ref) => ref === "main") ?? refs[0];
  const defaultTarget =
    refs.find((ref) => ref !== defaultBase && !ref.startsWith("origin/")) ??
    refs.find((ref) => ref !== defaultBase) ??
    "HEAD";
  const listId = `git-compare-refs-${repo.key.replace(/[^a-zA-Z0-9_-]/g, "_")}`;

  return (
    <form
      className="flex flex-wrap items-center gap-1.5"
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const baseRef = String(data.get("baseRef") ?? "").trim();
        const targetRef = String(data.get("targetRef") ?? "").trim();
        if (!baseRef || !targetRef) return;
        onCompare(baseRef, targetRef);
      }}
    >
      <datalist id={listId}>
        {refs.map((ref) => (
          <option key={ref} value={ref} />
        ))}
      </datalist>
      <input
        name="baseRef"
        list={listId}
        defaultValue={defaultBase}
        className="h-8 w-28 rounded-md border border-border-theme bg-white px-2 text-[12px] text-text-base outline-none focus:border-blue-400"
        aria-label="Base ref"
      />
      <span className="text-[11px] text-text-secondary">...</span>
      <input
        name="targetRef"
        list={listId}
        defaultValue={defaultTarget}
        className="h-8 w-28 rounded-md border border-border-theme bg-white px-2 text-[12px] text-text-base outline-none focus:border-blue-400"
        aria-label="Target ref"
      />
      <button
        type="submit"
        disabled={busy}
        className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2.5 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
      >
        <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-1.5 text-[11px]" />
        {active ? "比较中..." : "比较"}
      </button>
    </form>
  );
}

function CompareResultView({ title, result }: { title: string; result: GitRefCompare }) {
  const targetCommits = result.commits.filter((commit) => commit.side === "target");
  const baseCommits = result.commits.filter((commit) => commit.side === "base");
  const [selectedFile, setSelectedFile] = useState(result.files[0]?.path ?? null);
  const [diff, setDiff] = useState<GitDiff | null>(null);
  const [loadingDiff, setLoadingDiff] = useState(false);

  useEffect(() => {
    setSelectedFile((current) =>
      current && result.files.some((file) => file.path === current)
        ? current
        : result.files[0]?.path ?? null,
    );
  }, [result]);

  useEffect(() => {
    if (!selectedFile) {
      setDiff(null);
      return;
    }
    let cancelled = false;
    setLoadingDiff(true);
    gitRefDiff(result.project_path, result.base_ref, result.target_ref, selectedFile)
      .then((next) => {
        if (!cancelled) setDiff(next);
      })
      .catch((err) => {
        if (!cancelled) {
          setDiff({
            project_path: result.project_path,
            repo_root: result.repo_root,
            file_path: selectedFile,
            staged: false,
            is_repo: result.is_repo,
            text: err instanceof Error ? err.message : String(err),
            truncated: false,
          });
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingDiff(false);
      });
    return () => {
      cancelled = true;
    };
  }, [result.base_ref, result.is_repo, result.project_path, result.repo_root, result.target_ref, selectedFile]);

  return (
    <div className="border-b border-border-theme bg-blue-50/40 px-3 py-2">
      <div className="flex flex-wrap items-center gap-2 text-[12px]">
        <FontAwesomeIcon icon={["fas", "code-branch"]} className="text-blue-600" />
        <span className="font-medium text-text-base">{title}</span>
        <Metric label="ahead" value={`${result.ahead}`} tone={result.ahead > 0 ? "good" : "muted"} />
        <Metric label="behind" value={`${result.behind}`} tone={result.behind > 0 ? "warn" : "muted"} />
        {result.merge_base && (
          <span className="text-[11px] text-text-secondary">base {shortHash(result.merge_base)}</span>
        )}
      </div>
      <div className="mt-2 grid gap-2 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(320px,1.5fr)]">
        <CompareCommitColumn title="Target only" commits={targetCommits} empty="No target-only commits" />
        <CompareCommitColumn title="Base only" commits={baseCommits} empty="No base-only commits" />
        <div className="grid min-h-[260px] min-w-0 grid-rows-[auto_120px_minmax(0,1fr)] overflow-hidden rounded-md border border-border-theme bg-white">
          <div className="px-2 py-2 text-[11px] font-medium uppercase text-text-secondary">
            Changed files
          </div>
          <div className="min-h-0 overflow-y-auto border-b border-border-theme px-2 pb-2">
            {result.files.length === 0 ? (
              <div className="text-[12px] text-text-secondary">No file changes</div>
            ) : (
              <div className="space-y-1">
                {result.files.map((file) => (
                  <button
                    type="button"
                    key={file.path}
                    onClick={() => setSelectedFile(file.path)}
                    className={`flex w-full min-w-0 items-center justify-between gap-2 rounded px-2 py-1 text-left text-[12px] ${
                      selectedFile === file.path
                        ? "bg-blue-50 text-blue-700"
                        : "text-text-base hover:bg-gray-50"
                    }`}
                  >
                    <span className="truncate" title={file.path}>
                      {file.path}
                    </span>
                    <span className="shrink-0 text-[11px] text-text-secondary">
                      +{file.additions} -{file.deletions}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
          <div className="min-h-0 overflow-auto bg-[#fbfbfb]">
            {loadingDiff ? (
              <div className="p-3 text-[12px] text-text-secondary">Loading diff...</div>
            ) : diff ? (
              <DiffText text={diff.text || "No diff for this file."} />
            ) : (
              <div className="p-3 text-[12px] text-text-secondary">Select a file to view diff.</div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function CompareCommitColumn({
  title,
  commits,
  empty,
}: {
  title: string;
  commits: GitRefCompare["commits"];
  empty: string;
}) {
  return (
    <div className="min-w-0 rounded-md border border-border-theme bg-white p-2">
      <div className="mb-1 text-[11px] font-medium uppercase text-text-secondary">{title}</div>
      {commits.length === 0 ? (
        <div className="text-[12px] text-text-secondary">{empty}</div>
      ) : (
        <div className="space-y-1">
          {commits.slice(0, 5).map((commit) => (
            <div key={commit.full_hash} className="min-w-0 text-[12px]">
              <span className="mr-1 font-mono text-[11px] text-text-secondary">{commit.hash}</span>
              <span className="text-text-base">{commit.subject}</span>
            </div>
          ))}
          {commits.length > 5 && (
            <div className="text-[11px] text-text-secondary">+{commits.length - 5} more</div>
          )}
        </div>
      )}
    </div>
  );
}

function BatchCommitPanel({
  mode,
  targets,
  onClose,
  onDone,
}: {
  mode: "commit" | "commit-push" | "step";
  targets: ProjectRow[];
  onClose: () => void;
  onDone: () => Promise<void> | void;
}) {
  const settings = useMemo(() => getGitUiSettings(), []);
  const [preview, setPreview] = useState<GitBatchCommitPreviewItem[]>([]);
  const [message, setMessage] = useState("");
  const [stageAll, setStageAll] = useState(settings.batchStageAll);
  const [overrides, setOverrides] = useState<Record<string, string>>({});
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [results, setResults] = useState<GitBatchProjectResult[] | null>(null);
  const paths = useMemo(() => targets.map((row) => row.status.project_path), [targets]);

  const loadPreview = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setPreview(await gitBatchCommitPreview(paths));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [paths]);

  useEffect(() => {
    void loadPreview();
  }, [loadPreview]);

  const mergeResults = (next: GitBatchProjectResult[]) => {
    setResults((current) => {
      if (!current) return next;
      const byPath = new Map(current.map((item) => [item.project_path, item]));
      for (const item of next) byPath.set(item.project_path, item);
      return Array.from(byPath.values());
    });
  };

  const pushCommittedResults = async (committed: GitBatchProjectResult[]): Promise<GitBatchProjectResult[]> => {
    const pushTargets = committed
      .filter((result) => result.ok && result.committed)
      .map((result) => result.project_path);
    if (pushTargets.length === 0) return committed;

    const scans = await Promise.all(pushTargets.map((path) => gitPushRiskScan(path)));
    const scanBlocked = scans.find((scan) => scan.blocked_reason);
    if (scanBlocked) {
      return committed.map((result) =>
        pushTargets.includes(result.project_path)
          ? {
              ...result,
              ok: false,
              pushed: false,
              message: `Committed, but push risk scan failed: ${scanBlocked.blocked_reason}`,
            }
          : result,
      );
    }
    const risks = scans.flatMap((scan) => scan.risks);
    const highRisks = risks.filter((risk) => risk.severity === "high").length;
    if (risks.length > 0) {
      const okRisk = window.confirm(
        highRisks > 0
          ? `Push risk scan found ${highRisks} high-risk finding(s). Continue pushing ${pushTargets.length} target(s)?`
          : `Push risk scan found ${risks.length} finding(s). Continue pushing ${pushTargets.length} target(s)?`,
      );
      if (!okRisk) {
        return committed.map((result) =>
          pushTargets.includes(result.project_path)
            ? { ...result, ok: false, pushed: false, message: "Committed, push cancelled after risk scan." }
            : result,
        );
      }
    }
    if (settings.confirmBeforePush) {
      const ok = window.confirm(`Push ${pushTargets.length} committed project target(s)?`);
      if (!ok) {
        return committed.map((result) =>
          pushTargets.includes(result.project_path)
            ? { ...result, ok: false, pushed: false, message: "Committed, push cancelled." }
            : result,
        );
      }
    }
    const pushed = await gitBatchPush(pushTargets);
    const pushedByPath = new Map(pushed.map((item) => [item.project_path, item]));
    return committed.map((result) => {
      const push = pushedByPath.get(result.project_path);
      if (!push) return result;
      return {
        ...result,
        ok: push.ok,
        pushed: push.pushed,
        push_result: push.push_result,
        message: push.ok ? "Committed and pushed" : `Committed, but push failed: ${push.message}`,
      };
    });
  };

  const commitTargets = async (rows: ProjectRow[], pushAfterCommit: boolean): Promise<GitBatchProjectResult[]> => {
    const trimmed = message.trim();
    const payload = rows.map((row) => ({
      project_path: row.status.project_path,
      message: overrides[row.status.project_path]?.trim() || null,
    }));
    const committed = await gitBatchCommit(payload, trimmed, stageAll);
    return pushAfterCommit ? pushCommittedResults(committed) : committed;
  };

  const run = async () => {
    const trimmed = message.trim();
    if (!trimmed) {
      setError("Commit message must not be empty.");
      return;
    }
    const pushAfterCommit = mode === "commit-push";
    if (pushAfterCommit) {
      const ok = window.confirm(`Commit ${targets.length} project target(s), then run push risk scan?`);
      if (!ok) return;
    }
    setBusy(true);
    setError(null);
    setResults(null);
    try {
      const next = await commitTargets(targets, pushAfterCommit);
      setResults(next);
      await onDone();
      await loadPreview();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const runSingle = async (projectPath: string, pushAfterCommit: boolean) => {
    const trimmed = message.trim();
    if (!trimmed) {
      setError("Commit message must not be empty.");
      return;
    }
    const row = targets.find((target) => target.status.project_path === projectPath);
    if (!row) return;
    setBusy(true);
    setError(null);
    try {
      const next = await commitTargets([row], pushAfterCommit);
      mergeResults(next);
      await onDone();
      await loadPreview();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const blockedCount = preview.filter((item) => effectiveBatchBlockedReason(item, stageAll)).length;
  const changedCount = preview.reduce((sum, item) => sum + item.files_changed, 0);

  return (
    <div className="overflow-hidden rounded-lg border border-border-theme bg-white">
      <div className="flex items-center justify-between border-b border-border-theme px-3 py-2">
        <div>
          <div className="text-[13px] font-medium text-text-base">
            {mode === "step" ? "逐个处理" : mode === "commit-push" ? "批量提交并上传" : "批量提交"}
          </div>
          <div className="text-[11px] text-text-secondary">
            {targets.length} 个项目，{changedCount} 个变更文件
          </div>
        </div>
        <button
          type="button"
          onClick={onClose}
          className="h-8 w-8 rounded-md text-text-secondary hover:bg-gray-100 hover:text-text-base"
          aria-label="Close batch commit"
        >
          <FontAwesomeIcon icon={["fas", "xmark"]} />
        </button>
      </div>
      <div className="grid gap-3 p-3 lg:grid-cols-[minmax(0,1fr)_300px]">
        <div className="min-w-0">
          <textarea
            value={message}
            onChange={(event) => setMessage(event.target.value)}
            className="h-24 w-full resize-none rounded-md border border-border-theme bg-white px-3 py-2 text-[13px] text-text-base outline-none focus:border-blue-400"
            placeholder={settings.commitInstructions || "Commit message"}
          />
          <label className="mt-2 flex items-center gap-2 text-[12px] text-text-secondary">
            <input
              type="checkbox"
              checked={stageAll}
              onChange={(event) => setStageAll(event.target.checked)}
              className="h-3.5 w-3.5 rounded border-border-theme"
            />
            自动暂存每个项目的全部变更
          </label>
          {settings.commitInstructions && (
            <div className="mt-2 rounded-md bg-gray-50 px-3 py-2 text-[11px] text-text-secondary">
              {settings.commitInstructions}
            </div>
          )}
          {error && <Message tone="error">{error}</Message>}
          {blockedCount > 0 && (
            <Message tone="warn">{`${blockedCount} 个项目当前会被阻止，执行后会逐项显示原因。`}</Message>
          )}
          <div className="mt-3 flex items-center gap-2">
            {mode !== "step" && (
              <button
                type="button"
                disabled={busy || loading || !message.trim()}
                onClick={run}
                className="inline-flex h-9 items-center rounded-md bg-text-base px-3 text-[12px] font-medium text-white hover:bg-primary disabled:cursor-not-allowed disabled:bg-gray-300"
              >
                <FontAwesomeIcon icon={mode === "commit-push" ? ["fas", "upload"] : ["fas", "check"]} className="mr-1.5 text-[11px]" />
                {busy ? "执行中..." : mode === "commit-push" ? "提交并上传" : "提交"}
              </button>
            )}
            <button
              type="button"
              disabled={busy || loading}
              onClick={() => void loadPreview()}
              className="inline-flex h-9 items-center rounded-md border border-border-theme bg-white px-3 text-[12px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
            >
              刷新预览
            </button>
          </div>
        </div>
        <div className="min-w-0 rounded-md border border-border-theme">
          <div className="border-b border-border-theme px-3 py-2 text-[12px] font-medium text-text-base">
            项目预览
          </div>
          <div className="max-h-[280px] overflow-y-auto">
            {loading ? (
              <div className="px-3 py-3 text-[12px] text-text-secondary">Loading...</div>
            ) : preview.length === 0 ? (
              <div className="px-3 py-3 text-[12px] text-text-secondary">No targets.</div>
            ) : (
              preview.map((item) => (
                <div key={item.project_path} className="border-b border-border-theme px-3 py-2 last:border-b-0">
                  <div className="truncate text-[12px] font-medium text-text-base" title={item.project_path}>
                    {item.project_path.split(/[\\/]/).pop() || item.project_path}
                  </div>
                  <div className="mt-1 flex flex-wrap gap-1.5 text-[11px] text-text-secondary">
                    <span>{item.current_branch ?? "HEAD"}</span>
                    <span>changed {item.files_changed}</span>
                    <span>staged {item.staged_files}</span>
                    <span className="text-green-600">+{item.additions}</span>
                    <span className="text-red-500">-{item.deletions}</span>
                  </div>
                  {effectiveBatchBlockedReason(item, stageAll) && (
                    <div className="mt-1 rounded bg-amber-50 px-2 py-1 text-[11px] text-amber-700">
                      {effectiveBatchBlockedReason(item, stageAll)}
                    </div>
                  )}
                  <input
                    value={overrides[item.project_path] ?? ""}
                    onChange={(event) =>
                      setOverrides((current) => ({
                        ...current,
                        [item.project_path]: event.target.value,
                      }))
                    }
                    className="mt-2 h-8 w-full rounded-md border border-border-theme px-2 text-[12px] outline-none focus:border-blue-400"
                    placeholder="单项目提交说明，可选"
                  />
                  {mode === "step" && (
                    <div className="mt-2 flex flex-wrap gap-1.5">
                      <button
                        type="button"
                        disabled={busy || loading || !message.trim()}
                        onClick={() => void runSingle(item.project_path, false)}
                        className="inline-flex h-7 items-center rounded-md border border-border-theme bg-white px-2 text-[11px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                      >
                        提交
                      </button>
                      <button
                        type="button"
                        disabled={busy || loading || !message.trim()}
                        onClick={() => void runSingle(item.project_path, true)}
                        className="inline-flex h-7 items-center rounded-md border border-border-theme bg-white px-2 text-[11px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
                      >
                        提交并上传
                      </button>
                    </div>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
      </div>
      {results && (
        <div className="border-t border-border-theme p-3">
          <div className="mb-2 text-[12px] font-medium text-text-base">执行结果</div>
          <div className="grid gap-2 md:grid-cols-2">
            {results.map((result) => (
              <div
                key={result.project_path}
                className={`rounded-md border px-3 py-2 text-[12px] ${
                  result.ok ? "border-green-100 bg-green-50 text-green-800" : "border-red-100 bg-red-50 text-red-700"
                }`}
              >
                <div className="truncate font-medium" title={result.project_path}>
                  {result.project_path}
                </div>
                <div className="mt-1 text-[11px] opacity-90">{result.message || (result.ok ? "OK" : "Failed")}</div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function BatchPushRiskCard({ summary }: { summary: BatchPushRiskSummary }) {
  const high = summary.risks.filter((risk) => risk.severity === "high").length;
  const medium = summary.risks.filter((risk) => risk.severity === "medium").length;
  const low = summary.risks.filter((risk) => risk.severity === "low").length;
  return (
    <div className="rounded-lg border border-amber-200 bg-amber-50 p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="text-[13px] font-medium text-amber-900">
          Batch push risk scan
        </div>
        <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
          <span className="rounded bg-white px-1.5 py-0.5 text-text-secondary">
            {summary.scannedTargets} target(s)
          </span>
          <span className="rounded bg-red-100 px-1.5 py-0.5 text-red-700">high {high}</span>
          <span className="rounded bg-amber-100 px-1.5 py-0.5 text-amber-700">medium {medium}</span>
          <span className="rounded bg-blue-100 px-1.5 py-0.5 text-blue-700">low {low}</span>
        </div>
      </div>
      {summary.risks.length === 0 ? (
        <div className="mt-2 text-[12px] text-amber-800">
          No local risk findings before pushing {summary.branch}.
        </div>
      ) : (
        <div className="mt-2 max-h-48 overflow-y-auto rounded-md border border-amber-200 bg-white">
          {summary.risks.slice(0, 12).map((risk, index) => (
            <div key={`${risk.projectPath}:${risk.title}:${index}`} className="border-b border-border-theme px-3 py-2 last:border-b-0">
              <div className="flex flex-wrap items-center gap-2 text-[12px]">
                <span className={`rounded px-1.5 py-0.5 text-[10px] font-medium ${riskSeverityClass(risk.severity)}`}>
                  {risk.severity}
                </span>
                <span className="font-medium text-text-base">{risk.title}</span>
                <span className="text-[11px] text-text-secondary">{risk.category}</span>
              </div>
              <div className="mt-1 truncate text-[11px] text-text-secondary" title={risk.projectPath}>
                {risk.projectPath}
              </div>
              <div className="mt-0.5 text-[11px] text-text-secondary">
                {risk.filePath ? `${risk.filePath}: ` : ""}
                {risk.detail}
              </div>
            </div>
          ))}
          {summary.risks.length > 12 && (
            <div className="px-3 py-2 text-[11px] text-text-secondary">
              +{summary.risks.length - 12} more finding(s)
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function groupStatuses(projects: Project[], statuses: GitProjectStatus[]): RepoGroup[] {
  const projectByPath = new Map(projects.map((project) => [project.path, project]));
  const repoMap = new Map<string, Map<string, ProjectRow[]>>();
  for (const status of statuses.filter((item) => item.is_repo)) {
    const repoKey = status.repo_id ?? status.repo_root ?? status.project_path;
    const branch = status.current_branch ?? "detached";
    if (!repoMap.has(repoKey)) repoMap.set(repoKey, new Map());
    const branchMap = repoMap.get(repoKey)!;
    if (!branchMap.has(branch)) branchMap.set(branch, []);
    branchMap.get(branch)!.push({
      project: projectByPath.get(status.project_path) ?? null,
      status,
    });
  }

  return Array.from(repoMap.entries()).map(([repoKey, branchMap]) => {
    const branches = Array.from(branchMap.entries()).map(([branch, rows]) => ({
      key: `${repoKey}:${branch}`,
      branch,
      rows: rows.sort((a, b) => a.status.project_path.localeCompare(b.status.project_path)),
    }));
    const first = branches[0]?.rows[0]?.status;
    return {
      key: repoKey,
      label: first?.repo_root ?? repoKey,
      branches: branches.sort((a, b) => a.branch.localeCompare(b.branch)),
    };
  });
}

function flattenBatchPushRisks(
  scans: Array<{ row: ProjectRow; scan: GitPushRiskScan }>,
): BatchPushRiskSummary["risks"] {
  return scans.flatMap(({ row, scan }) =>
    scan.risks.map((risk) => ({
      projectPath: row.status.project_path,
      severity: risk.severity,
      category: risk.category,
      title: risk.title,
      detail: risk.detail,
      filePath: risk.file_path,
    })),
  );
}

function repoRefOptions(repo: RepoGroup): string[] {
  const refs = new Set<string>();
  for (const branch of repo.branches) {
    if (branch.branch && branch.branch !== "detached") refs.add(branch.branch);
    for (const row of branch.rows) {
      if (row.status.current_branch && !row.status.detached_head) refs.add(row.status.current_branch);
      if (row.status.upstream) refs.add(row.status.upstream);
    }
  }
  return Array.from(refs).sort((a, b) => a.localeCompare(b));
}

function uniquePushCandidates(rows: ProjectRow[]): ProjectRow[] {
  const seen = new Set<string>();
  const out: ProjectRow[] = [];
  for (const row of rows) {
    const key = `${row.status.repo_root ?? row.status.project_path}:${row.status.current_branch ?? "detached"}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(row);
  }
  return out;
}

function uniqueProjectRows(rows: ProjectRow[]): ProjectRow[] {
  const seen = new Set<string>();
  const out: ProjectRow[] = [];
  for (const row of rows.filter((item) => item.status.is_repo)) {
    const key = row.status.project_path;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(row);
  }
  return out;
}

function effectiveBatchBlockedReason(item: GitBatchCommitPreviewItem, stageAll: boolean): string | null {
  if (stageAll && item.blocked_reason?.startsWith("no staged files")) return null;
  return item.blocked_reason;
}

function shortHash(hash: string): string {
  return hash.slice(0, 8);
}

function uniqueUpdateCandidates(rows: ProjectRow[]): ProjectRow[] {
  const seen = new Set<string>();
  const out: ProjectRow[] = [];
  for (const row of rows) {
    const key = `${row.status.repo_root ?? row.status.project_path}:${row.status.current_branch ?? "detached"}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(row);
  }
  return out;
}

function uniqueRepoCandidates(statuses: GitProjectStatus[]): GitProjectStatus[] {
  const seen = new Set<string>();
  const out: GitProjectStatus[] = [];
  for (const status of statuses.filter((item) => item.is_repo)) {
    const key = status.repo_root ?? status.project_path;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(status);
  }
  return out;
}

function updateRiskBlockedReason(status: GitProjectStatus): string | null {
  if (status.detached_head) return "Detached HEAD cannot be updated from this view.";
  if (status.merge_state) return "Finish or abort the active merge before updating.";
  if (status.rebase_state) return "Finish or abort the active rebase before updating.";
  if (status.has_changes) return "Commit or stash local changes before updating.";
  if (!status.upstream) return "No upstream branch is configured.";
  return null;
}

function Metric({ label, value, tone }: { label: string; value: string; tone: "good" | "warn" | "muted" }) {
  const cls =
    tone === "good" ? "text-green-600" : tone === "warn" ? "text-amber-700" : "text-text-secondary";
  return (
    <span className="rounded bg-gray-100 px-1.5 py-0.5">
      {label} <span className={`font-medium ${cls}`}>{value}</span>
    </span>
  );
}

function Badge({ children, tone }: { children: string; tone: "warn" }) {
  const cls = tone === "warn" ? "bg-amber-50 text-amber-700" : "bg-gray-100 text-text-secondary";
  return <span className={`rounded px-1.5 py-0.5 ${cls}`}>{children}</span>;
}

function riskSeverityClass(severity: string): string {
  return severity === "high"
    ? "bg-red-100 text-red-700"
    : severity === "medium"
      ? "bg-amber-100 text-amber-700"
      : "bg-blue-100 text-blue-700";
}

function Message({ children, tone }: { children: string; tone: "error" | "success" | "warn" }) {
  const cls =
    tone === "error"
      ? "bg-red-50 text-red-600"
      : tone === "warn"
        ? "bg-amber-50 text-amber-700"
        : "bg-green-50 text-green-700";
  return <div className={`rounded-md px-3 py-2 text-[12px] ${cls}`}>{children}</div>;
}
