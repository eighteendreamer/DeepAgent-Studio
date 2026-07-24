import { useMemo } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { GitBranch } from "../../types";

interface GitBranchMenuContentProps {
  branches: GitBranch[];
  loading: boolean;
  busy: string | null;
  query: string;
  onQueryChange: (query: string) => void;
  currentBranch: string | null;
  additions: number;
  deletions: number;
  filesChanged: number;
  rebaseState: string | null;
  mergeState: boolean;
  operationError: string | null;
  operationResult: string | null;
  onCheckout: (branch: GitBranch) => void;
  onFetch: () => void;
  onCreateBranch: () => void;
  onOpenWorkbench?: () => void;
  className?: string;
  compact?: boolean;
  small?: boolean;
}

export function GitBranchMenuContent({
  branches,
  loading,
  busy,
  query,
  onQueryChange,
  currentBranch,
  additions,
  deletions,
  filesChanged,
  rebaseState,
  mergeState,
  operationError,
  operationResult,
  onCheckout,
  onFetch,
  onCreateBranch,
  onOpenWorkbench,
  className = "",
  compact = false,
  small = false,
}: GitBranchMenuContentProps) {
  const { t } = useTranslation();
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return branches;
    return branches.filter(
      (branch) =>
        branch.name.toLowerCase().includes(q) ||
        branch.subject?.toLowerCase().includes(q) ||
        branch.upstream?.toLowerCase().includes(q),
    );
  }, [branches, query]);

  const local = filtered.filter((branch) => branch.kind === "local");
  const remote = filtered.filter((branch) => branch.kind === "remote");

  const widthClass = small ? "w-[230px]" : compact ? "w-[280px]" : "w-[380px]";
  const searchPaddingClass = small ? "px-2 py-1" : compact ? "px-2 py-1.5" : "px-3 py-2";
  const searchBoxClass = small ? "px-2 py-1 text-[11px]" : compact ? "px-2 py-1 text-[12px]" : "px-2 py-1.5 text-[12px]";
  const summaryPaddingClass = small ? "px-2 py-1" : compact ? "px-2 py-1.5" : "px-3 py-2";
  const summaryCardClass = small ? "px-2 py-1" : compact ? "px-2.5 py-1.5" : "px-3 py-2";
  const listHeightClass = small ? "max-h-[120px]" : compact ? "max-h-[160px]" : "max-h-[280px]";
  const actionPaddingClass = small ? "px-2.5 py-1" : compact ? "px-3 py-1.5" : "px-4 py-2";

  return (
    <div className={`${widthClass} max-w-[calc(100vw-48px)] overflow-hidden rounded-lg border border-border-theme bg-white ${small ? "" : "shadow-[0_12px_36px_rgb(0,0,0,0.14)]"} ${className}`}>
      <div className={`border-b border-border-theme ${searchPaddingClass}`}>
        <div className={`flex items-center rounded-md bg-gray-50 text-text-secondary ${searchBoxClass}`}>
          <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="mr-2 text-[10px]" />
          <input
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            onKeyDown={(e) => e.stopPropagation()}
            placeholder={t("git.searchBranches")}
            className="w-full bg-transparent outline-none"
            autoFocus
          />
        </div>
      </div>

      <div className={summaryPaddingClass}>
        <div className={`flex items-center justify-between rounded-md bg-gray-50 ${summaryCardClass}`}>
          <div className="min-w-0">
            <div className="flex items-center text-[12px] font-medium text-text-base">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-[12px] text-text-secondary" />
              <span className="truncate">{currentBranch ?? t("git.detachedHead")}</span>
              {(rebaseState || mergeState) && (
                <span className="ml-2 rounded bg-amber-100 px-1.5 py-0.5 text-[10px] text-amber-700">
                  {rebaseState ? t("git.rebasing") : t("git.merging")}
                </span>
              )}
            </div>
            <div className="mt-0.5 truncate text-[10px] text-text-secondary">
              {filesChanged > 0 ? t("git.changedFilesCount", { count: filesChanged }) : t("git.workingTreeClean")}
            </div>
          </div>
          <div className="ml-3 flex items-center gap-1.5 text-[11px] font-medium tabular-nums">
            <span className="text-green-600">+{additions}</span>
            <span className="text-red-500">-{deletions}</span>
          </div>
        </div>
        {operationError && <Message tone="error">{operationError}</Message>}
        {operationResult && <Message tone="success">{operationResult}</Message>}
      </div>

      <div className={`${listHeightClass} overflow-y-auto py-0.5`}>
        {loading && <div className="px-4 py-2 text-[12px] text-text-secondary">{t("git.loadingState")}</div>}
        {!loading && branches.length === 0 && (
          <div className="px-4 py-2 text-[12px] text-text-secondary">{t("git.noBranchesFound")}</div>
        )}
        <BranchSection title={t("git.localBranches")} branches={local} busy={busy} onCheckout={onCheckout} compact={compact} small={small} />
        <BranchSection title={t("git.remoteBranches")} branches={remote} busy={busy} onCheckout={onCheckout} compact={compact} small={small} />
      </div>

      <div className={`border-t border-border-theme ${small ? "py-0.5" : "py-1"}`}>
        <button
          type="button"
          className={`flex w-full items-center text-left text-[12px] text-text-base hover:bg-gray-50 disabled:text-text-secondary ${actionPaddingClass}`}
          onClick={onFetch}
          disabled={!!busy}
        >
          <FontAwesomeIcon icon={["fas", "rotate-right"]} className="mr-2.5 w-3.5 text-text-secondary" />
          {busy === "fetch" ? t("git.fetching") : t("git.fetch")}
        </button>
        <button
          type="button"
          className={`flex w-full items-center text-left text-[12px] text-text-base hover:bg-gray-50 disabled:text-text-secondary ${actionPaddingClass}`}
          disabled={!!busy}
          onClick={onCreateBranch}
        >
          <FontAwesomeIcon icon={["fas", "plus"]} className="mr-2.5 w-3.5 text-text-secondary" />
          {t("git.createAndCheckoutBranch")}
        </button>
        <button
          type="button"
          className={`flex w-full items-center text-left text-[12px] ${
            onOpenWorkbench ? "text-text-base hover:bg-gray-50" : "text-text-secondary"
          } ${actionPaddingClass}`}
          disabled={!onOpenWorkbench}
          title={onOpenWorkbench ? t("git.openGit") : t("git.openGitUnavailable")}
          onClick={onOpenWorkbench}
        >
          <FontAwesomeIcon icon={["fas", "table"]} className="mr-2.5 w-3.5" />
          {t("git.openGit")}
        </button>
      </div>
    </div>
  );
}

function BranchSection({
  title,
  branches,
  busy,
  onCheckout,
  compact = false,
  small = false,
}: {
  title: string;
  branches: GitBranch[];
  busy: string | null;
  onCheckout: (branch: GitBranch) => void;
  compact?: boolean;
  small?: boolean;
}) {
  const { t } = useTranslation();

  if (branches.length === 0) return null;
  return (
    <div className={small || compact ? "py-0.5" : "py-1"}>
      <div className={`${small ? "px-2.5 py-0.5 text-[10px]" : compact ? "px-3 py-0.5 text-[10px]" : "px-4 py-1 text-[11px]"} font-medium text-text-secondary`}>{title}</div>
      {branches.map((branch) => {
        const disabled = !!busy || branch.current || !!branch.worktree_path;
        return (
          <button
            type="button"
            key={branch.full_name}
            className={`flex w-full items-center justify-between gap-2 text-left text-text-base hover:bg-gray-50 disabled:cursor-default disabled:hover:bg-transparent ${
              small ? "px-2.5 py-1 text-[11px]" : compact ? "px-3 py-1.5 text-[12px]" : "px-4 py-2 text-[13px]"
            }`}
            title={branch.worktree_path ? t("git.worktreeAt", { path: branch.worktree_path }) : branch.subject ?? branch.name}
            disabled={disabled}
            onClick={() => onCheckout(branch)}
          >
            <div className="flex min-w-0 items-center">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className={`${small || compact ? "mr-2 w-3.5" : "mr-2.5 w-4"} text-text-secondary`} />
              <div className="min-w-0">
                <div className="truncate font-medium">{branch.name}</div>
                <div className={`${small || compact ? "text-[10px]" : "text-[11px]"} truncate text-text-secondary`}>
                  {branch.worktree_path ? t("git.worktreeAt", { path: branch.worktree_path }) : branch.subject ?? branch.upstream ?? ""}
                </div>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              {(branch.ahead > 0 || branch.behind > 0) && (
                <span className={`${small || compact ? "text-[10px]" : "text-[11px]"} font-medium text-blue-600`}>{formatAheadBehind(branch.ahead, branch.behind, t)}</span>
              )}
              {branch.current && <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-base" />}
            </div>
          </button>
        );
      })}
    </div>
  );
}

function Message({ children, tone }: { children: string; tone: "error" | "success" }) {
  const cls = tone === "error" ? "bg-red-50 text-red-600" : "bg-green-50 text-green-700";
  return <div className={`mt-2 rounded-md px-3 py-2 text-[12px] ${cls}`}>{children}</div>;
}

export function formatAheadBehind(ahead: number, behind: number, t: TFunction): string {
  return [
    ahead > 0 ? t("git.aheadCount", { count: ahead }) : "",
    behind > 0 ? t("git.behindCount", { count: behind }) : "",
  ]
    .filter(Boolean)
    .join(" ");
}
