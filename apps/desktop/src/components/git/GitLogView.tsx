import { useCallback, useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { gitCommitDiff, gitCreateBranch, gitLog } from "../../api";
import type { GitCommitFile, GitDiff, GitLogEntry } from "../../types";
import { DiffText } from "./GitDiffViewer";
import { getGitUiSettings } from "./gitSettings";
import { GitCreateBranchDialog } from "./GitCreateBranchDialog";

interface Props {
  projectPath: string;
  onRefresh?: () => Promise<void> | void;
}

type GitLogViewCache = {
  version: 1;
  projectPath: string;
  cachedAt: number;
  entries: GitLogEntry[];
  selectedHash: string | null;
  selectedFile: string | null;
  diffCommitHash: string | null;
  diffFilePath: string | null;
  diff: GitDiff | null;
};

const GIT_LOG_VIEW_CACHE_PREFIX = "deepagent:git-log-view:";

function gitLogViewCacheKey(projectPath: string): string {
  return `${GIT_LOG_VIEW_CACHE_PREFIX}${encodeURIComponent(projectPath)}`;
}

function readGitLogViewCache(projectPath: string): GitLogViewCache | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(gitLogViewCacheKey(projectPath));
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<GitLogViewCache>;
    if (parsed.version !== 1 || parsed.projectPath !== projectPath || !Array.isArray(parsed.entries)) return null;
    return parsed as GitLogViewCache;
  } catch {
    return null;
  }
}

function writeGitLogViewCache(
  projectPath: string,
  entries: GitLogEntry[],
  selectedHash: string | null,
  selectedFile: string | null,
  diff: GitDiff | null,
) {
  if (typeof window === "undefined" || entries.length === 0) return;
  try {
    window.localStorage.setItem(
      gitLogViewCacheKey(projectPath),
      JSON.stringify({
        version: 1,
        projectPath,
        cachedAt: Date.now(),
        entries,
        selectedHash,
        selectedFile,
        diffCommitHash: diff ? selectedHash : null,
        diffFilePath: diff ? selectedFile : null,
        diff,
      } satisfies GitLogViewCache),
    );
  } catch {
    // Best-effort UI cache.
  }
}

export function GitLogView({ projectPath, onRefresh }: Props) {
  const { t } = useTranslation();
  const cached = readGitLogViewCache(projectPath);
  const [entries, setEntries] = useState<GitLogEntry[]>(() => cached?.entries ?? []);
  const [selectedHash, setSelectedHash] = useState<string | null>(() => cached?.selectedHash ?? cached?.entries[0]?.full_hash ?? null);
  const [selectedFile, setSelectedFile] = useState<string | null>(() => cached?.selectedFile ?? null);
  const [diff, setDiff] = useState<GitDiff | null>(() => cached?.diff ?? null);
  const [diffMeta, setDiffMeta] = useState<{ commitHash: string; filePath: string } | null>(() =>
    cached?.diffCommitHash && cached.diffFilePath
      ? { commitHash: cached.diffCommitHash, filePath: cached.diffFilePath }
      : null,
  );
  const [loading, setLoading] = useState(false);
  const [diffLoading, setDiffLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [operation, setOperation] = useState<string | null>(null);
  const [operationMessage, setOperationMessage] = useState<string | null>(null);
  const [createBranchOpen, setCreateBranchOpen] = useState(false);
  const [createBranchError, setCreateBranchError] = useState<string | null>(null);
  const [suggestedBranchName, setSuggestedBranchName] = useState("");

  const loadLog = useCallback(
    (preserveSelection: boolean) => {
      setLoading(true);
      setError(null);
      return gitLog(projectPath, 200)
        .then((next) => {
          setEntries(next);
          setSelectedHash((current) =>
            preserveSelection && current && next.some((entry) => entry.full_hash === current)
              ? current
              : next[0]?.full_hash ?? null,
          );
          setSelectedFile(null);
        })
        .catch((err) => {
          setError(err instanceof Error ? err.message : String(err));
        })
        .finally(() => {
          setLoading(false);
        });
    },
    [projectPath],
  );

  useEffect(() => {
    let cancelled = false;
    const nextCache = readGitLogViewCache(projectPath);
    if (nextCache) {
      setEntries(nextCache.entries);
      setSelectedHash(nextCache.selectedHash ?? nextCache.entries[0]?.full_hash ?? null);
      setSelectedFile(nextCache.selectedFile);
      setDiff(nextCache.diff);
      setDiffMeta(
        nextCache.diffCommitHash && nextCache.diffFilePath
          ? { commitHash: nextCache.diffCommitHash, filePath: nextCache.diffFilePath }
          : null,
      );
      setLoading(false);
      return () => {
        cancelled = true;
      };
    }
    loadLog(false).finally(() => {
      if (cancelled) return;
    });
    return () => {
      cancelled = true;
    };
  }, [loadLog]);

  useEffect(() => {
    const matchingDiff =
      diffMeta?.commitHash === selectedHash && diffMeta.filePath === selectedFile ? diff : null;
    writeGitLogViewCache(projectPath, entries, selectedHash, selectedFile, matchingDiff);
  }, [diff, diffMeta, entries, projectPath, selectedFile, selectedHash]);

  const selectedCommit = useMemo(
    () => entries.find((entry) => entry.full_hash === selectedHash) ?? entries[0] ?? null,
    [entries, selectedHash],
  );

  useEffect(() => {
    if (!selectedCommit) {
      setSelectedFile(null);
      return;
    }
    setSelectedFile((current) =>
      current && selectedCommit.files.some((file) => file.path === current)
        ? current
        : selectedCommit.files[0]?.path ?? null,
    );
  }, [selectedCommit]);

  useEffect(() => {
    if (!selectedCommit || !selectedFile) {
      setDiff(null);
      return;
    }
    const nextCache = readGitLogViewCache(projectPath);
    const cachedDiff = nextCache?.diff;
    if (
      cachedDiff &&
      selectedHash === nextCache?.diffCommitHash &&
      selectedFile === nextCache.diffFilePath &&
      cachedDiff.file_path === selectedFile &&
      selectedCommit.full_hash === selectedHash
    ) {
      setDiff(cachedDiff);
      setDiffMeta({ commitHash: selectedCommit.full_hash, filePath: selectedFile });
      setDiffLoading(false);
      return;
    }
    let cancelled = false;
    setDiff(null);
    setDiffMeta(null);
    setDiffLoading(true);
    gitCommitDiff(projectPath, selectedCommit.full_hash, selectedFile)
      .then((next) => {
        if (!cancelled) {
          setDiff(next);
          setDiffMeta({ commitHash: selectedCommit.full_hash, filePath: selectedFile });
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setDiff({
            project_path: projectPath,
            repo_root: null,
            file_path: selectedFile,
            staged: false,
            is_repo: true,
            text: err instanceof Error ? err.message : String(err),
            truncated: false,
          });
          setDiffMeta({ commitHash: selectedCommit.full_hash, filePath: selectedFile });
        }
      })
      .finally(() => {
        if (!cancelled) setDiffLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath, selectedCommit, selectedFile]);

  const openCreateBranchDialog = () => {
    if (!selectedCommit || operation) return;
    const settings = getGitUiSettings();
    setOperationMessage(null);
    setCreateBranchError(null);
    setSuggestedBranchName(`${settings.branchPrefix}${slugBranchName(selectedCommit.subject) || selectedCommit.hash}`);
    setCreateBranchOpen(true);
  };

  const createBranchFromCommit = async (name: string) => {
    if (!selectedCommit || operation) return;
    setOperationMessage(null);
    setCreateBranchError(null);
    setOperation("branch");
    try {
      const result = await gitCreateBranch(projectPath, name, selectedCommit.full_hash);
      if (!result.ok) throw new Error(result.stderr || result.stdout || result.command);
      setOperationMessage(t("git.logPanel.branchCreated", { name }));
      setCreateBranchOpen(false);
      await loadLog(true);
      await onRefresh?.();
    } catch (err) {
      setCreateBranchError(err instanceof Error ? err.message : String(err));
    } finally {
      setOperation(null);
    }
  };

  return (
    <div className="grid h-full min-h-0 grid-cols-[minmax(340px,38%)_minmax(0,1fr)] bg-white">
      <div className="flex min-h-0 flex-col border-r border-border-theme">
        <div className="flex h-10 flex-shrink-0 items-center justify-between border-b border-border-theme px-3">
          <div className="flex items-center text-[13px] font-medium text-text-base">
            <FontAwesomeIcon icon={["fas", "code-commit"]} className="mr-2 text-text-secondary" />
            {t("git.logPanel.title")}
          </div>
          <span className="text-[11px] text-text-secondary">{t("git.logPanel.commitsCount", { count: entries.length })}</span>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto bg-gray-50/60">
          {loading ? (
            <div className="px-4 py-3 text-[13px] text-text-secondary">{t("git.logPanel.loadingLog")}</div>
          ) : error ? (
            <div className="px-4 py-3 text-[13px] text-red-500">{error}</div>
          ) : entries.length === 0 ? (
            <div className="px-4 py-3 text-[13px] text-text-secondary">{t("git.logPanel.noCommitsFound")}</div>
          ) : (
            entries.map((entry, index) => (
              <CommitRow
                key={entry.full_hash}
                entry={entry}
                index={index}
                total={entries.length}
                active={selectedCommit?.full_hash === entry.full_hash}
                onClick={() => {
                  setSelectedHash(entry.full_hash);
                  setSelectedFile(entry.files[0]?.path ?? null);
                }}
              />
            ))
          )}
        </div>
      </div>

      <div className="flex min-h-0 flex-col">
        {selectedCommit ? (
          <>
            <div className="border-b border-border-theme px-4 py-3">
              <div className="flex min-w-0 items-start justify-between gap-3">
                <div className="min-w-0">
                  <div className="flex min-w-0 items-center text-[14px] font-medium text-text-base">
                    <span className="truncate">{selectedCommit.subject}</span>
                    <span className="ml-2 rounded bg-gray-100 px-1.5 py-0.5 font-mono text-[11px] text-text-secondary">
                      {selectedCommit.hash}
                    </span>
                    {selectedCommit.parents.length > 1 && (
                      <span className="ml-2 rounded bg-purple-50 px-1.5 py-0.5 text-[10px] text-purple-600">
                        {t("git.logPanel.mergeCount", { count: selectedCommit.parents.length })}
                      </span>
                    )}
                  </div>
                  <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-text-secondary">
                    <span>{selectedCommit.author_name}</span>
                    <span>{formatDate(selectedCommit.date)}</span>
                    {selectedCommit.refs.map((ref) => (
                      <span key={ref} className="rounded bg-blue-50 px-1.5 py-0.5 text-blue-600">
                        {ref}
                      </span>
                    ))}
                  </div>
                  {operationMessage && (
                    <div className="mt-1 max-w-full truncate text-[11px] text-text-secondary" title={operationMessage}>
                      {operationMessage}
                    </div>
                  )}
                </div>
                <div className="flex shrink-0 items-center gap-1">
                  <LogActionButton
                    label={t("git.logPanel.createBranch")}
                    icon={["fas", "code-branch"]}
                    disabled={!!operation}
                    busy={operation === "branch"}
                    onClick={openCreateBranchDialog}
                  />
                </div>
              </div>
            </div>
            <div className="grid min-h-0 flex-1 grid-rows-[160px_minmax(0,1fr)]">
              <div className="overflow-y-auto border-b border-border-theme bg-gray-50/60 py-2">
                {selectedCommit.files.length === 0 ? (
                  <div className="px-4 py-2 text-[13px] text-text-secondary">{t("git.logPanel.noFileList")}</div>
                ) : (
                  selectedCommit.files.map((file) => (
                    <CommitFileRow
                      key={`${selectedCommit.full_hash}:${file.path}`}
                      file={file}
                      active={file.path === selectedFile}
                      onClick={() => setSelectedFile(file.path)}
                    />
                  ))
                )}
              </div>
              <div className="min-h-0 overflow-auto bg-[#fbfbfb]">
                {diffLoading ? (
                  <div className="flex h-full items-center justify-center text-[13px] text-text-secondary">
                    {t("git.logPanel.loadingCommitDiff")}
                  </div>
                ) : diff?.text ? (
                  <DiffText text={diff.text} />
                ) : (
                  <div className="flex h-full items-center justify-center text-[13px] text-text-secondary">
                    {t("git.logPanel.selectFile")}
                  </div>
                )}
              </div>
            </div>
          </>
        ) : (
          <div className="flex h-full items-center justify-center text-[13px] text-text-secondary">
            {t("git.logPanel.selectCommit")}
          </div>
        )}
      </div>

      <GitCreateBranchDialog
        open={createBranchOpen}
        title={t("git.createBranchDialog.title")}
        label={t("git.logPanel.newBranchName")}
        initialValue={suggestedBranchName}
        confirmLabel={t("git.logPanel.createBranch")}
        loading={operation === "branch"}
        error={createBranchError}
        onClose={() => {
          if (operation === "branch") return;
          setCreateBranchOpen(false);
          setCreateBranchError(null);
        }}
        onConfirm={createBranchFromCommit}
      />
    </div>
  );
}

function CommitRow({
  entry,
  index,
  total,
  active,
  onClick,
}: {
  entry: GitLogEntry;
  index: number;
  total: number;
  active: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const parentCount = entry.parents.length;
  const merge = parentCount > 1;
  return (
    <button
      type="button"
      className={`grid w-full grid-cols-[48px_minmax(0,1fr)] px-3 py-2 text-left transition-colors ${
        active ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:bg-white/70 hover:text-text-base"
      }`}
      onClick={onClick}
      title={entry.subject}
    >
      <CommitGraphRail first={index === 0} last={index === total - 1} active={active} merge={merge} />
      <div className="min-w-0">
        <div className="flex min-w-0 items-center">
          <span className="truncate text-[13px] font-medium">{entry.subject}</span>
          {merge && (
            <span className="ml-1.5 shrink-0 rounded bg-purple-50 px-1.5 py-0.5 text-[10px] text-purple-600">
              {t("git.logPanel.mergeCount", { count: parentCount })}
            </span>
          )}
          {entry.refs.slice(0, 2).map((ref) => (
            <span key={ref} className="ml-1.5 shrink-0 rounded bg-blue-50 px-1.5 py-0.5 text-[10px] text-blue-600">
              {ref}
            </span>
          ))}
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-[11px] text-text-secondary">
          <span className="font-mono">{entry.hash}</span>
          <span>{entry.author_name}</span>
          <span>{formatDate(entry.date)}</span>
          <span>{t("git.logPanel.filesCount", { count: entry.files.length })}</span>
        </div>
      </div>
    </button>
  );
}

function CommitGraphRail({
  first,
  last,
  active,
  merge,
}: {
  first: boolean;
  last: boolean;
  active: boolean;
  merge: boolean;
}) {
  const stroke = active ? "#2563eb" : "#9ca3af";
  const muted = "#d1d5db";
  return (
    <div className="flex h-full items-center justify-center">
      <svg width="40" height="40" viewBox="0 0 40 40" aria-hidden="true" className="shrink-0">
        {!first && <line x1="16" y1="0" x2="16" y2="16" stroke={muted} strokeWidth="2" />}
        {!last && <line x1="16" y1="24" x2="16" y2="40" stroke={muted} strokeWidth="2" />}
        {merge && (
          <>
            <path d="M16 20 C23 20 25 8 34 8" fill="none" stroke={muted} strokeWidth="2" />
            <path d="M16 20 C23 20 25 32 34 32" fill="none" stroke={muted} strokeWidth="2" />
            <circle cx="34" cy="8" r="2.5" fill={muted} />
            <circle cx="34" cy="32" r="2.5" fill={muted} />
          </>
        )}
        <circle cx="16" cy="20" r={active ? "5" : "4"} fill={stroke} />
        <circle cx="16" cy="20" r="7" fill="none" stroke={active ? "#bfdbfe" : "transparent"} strokeWidth="2" />
      </svg>
    </div>
  );
}

function CommitFileRow({
  file,
  active,
  onClick,
}: {
  file: GitCommitFile;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={`flex w-full items-center justify-between gap-3 px-4 py-2 text-left text-[12px] transition-colors ${
        active ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:bg-white/70 hover:text-text-base"
      }`}
      onClick={onClick}
      title={file.old_path ? `${file.old_path} -> ${file.path}` : file.path}
    >
      <div className="flex min-w-0 items-center">
        <FontAwesomeIcon icon={["far", "file-lines"]} className="mr-2 w-4 text-text-secondary" />
        <span className="truncate">{file.path}</span>
      </div>
      <div className="flex shrink-0 items-center gap-1 text-[11px] font-medium tabular-nums">
        <span className="text-green-600">+{file.additions}</span>
        <span className="text-red-500">-{file.deletions}</span>
      </div>
    </button>
  );
}

function LogActionButton({
  label,
  icon,
  disabled,
  busy,
  onClick,
}: {
  label: string;
  icon: ["fas", string];
  disabled: boolean;
  busy: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="inline-flex h-8 items-center rounded-md border border-border-theme bg-white px-2 text-[12px] font-medium text-text-secondary transition-colors hover:bg-gray-50 hover:text-text-base disabled:cursor-not-allowed disabled:opacity-50"
      disabled={disabled}
      onClick={onClick}
      title={label}
    >
      <FontAwesomeIcon
        icon={(busy ? ["fas", "spinner"] : icon) as any}
        className={`mr-1.5 text-[11px] ${busy ? "animate-spin" : ""}`}
      />
      {label}
    </button>
  );
}

function slugBranchName(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48);
}

function formatDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
