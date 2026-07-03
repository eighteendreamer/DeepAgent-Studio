import { useEffect, useMemo, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { gitCheckoutBranch, gitCreateBranch, gitFetch } from "../../api";
import type { GitBranch, GitOperationResult } from "../../types";
import { useGitStatus } from "../../hooks/useGitStatus";
import { getGitUiSettings } from "./gitSettings";

interface Props {
  projectPath?: string | null;
  compact?: boolean;
  className?: string;
  variant?: "chip" | "row" | "env";
  dropdownPlacement?: "top" | "bottom";
  dropdownAlign?: "left" | "right";
  onStatusChange?: (status: ReturnType<typeof useGitStatus>["status"]) => void;
  onOpenWorkbench?: () => void;
}

export function GitBranchChip({
  projectPath,
  compact = false,
  className = "",
  variant = "chip",
  dropdownPlacement = "top",
  dropdownAlign = "left",
  onStatusChange,
  onOpenWorkbench,
}: Props) {
  const { t } = useTranslation();
  const { loading, status, branches, changes, refresh } = useGitStatus(projectPath);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [operationResult, setOperationResult] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    onStatusChange?.(status);
  }, [onStatusChange, status]);

  useEffect(() => {
    const onClick = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) setOpen(false);
    };
    if (open) document.addEventListener("mousedown", onClick);
    return () => document.removeEventListener("mousedown", onClick);
  }, [open]);

  const currentLabel = status?.current_branch ?? (loading ? "..." : null);
  if (!projectPath || (!loading && !status?.is_repo)) return null;

  const dirty = !!status?.has_changes;
  const displayAheadBehind =
    status && (status.ahead > 0 || status.behind > 0) ? formatAheadBehind(status.ahead, status.behind, t) : "";

  const runOperation = async (
    action: "checkout" | "createBranch" | "fetch",
    operation: () => Promise<GitOperationResult | void>,
  ) => {
    setBusy(action);
    setOperationError(null);
    setOperationResult(null);
    try {
      const result = await operation();
      if (result && !result.ok) {
        setOperationError(result.stderr || result.stdout || t("git.operationFailed", { action: t(`git.actions.${action}`) }));
        return;
      }
      if (result) {
        setOperationResult(result.stdout.trim() || t("git.operationCompleted", { action: t(`git.actions.${action}`) }));
      }
      await refresh();
    } catch (error) {
      setOperationError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  };

  const checkoutBranch = async (branch: GitBranch) => {
    if (!projectPath || branch.current || busy) return;
    if (branch.worktree_path) {
      setOperationError(t("git.branchAlreadyCheckedOut", { path: branch.worktree_path }));
      return;
    }
    if (dirty) {
      const ok = window.confirm(t("git.switchBranchConfirm", { branch: branch.name }));
      if (!ok) return;
    }
    await runOperation("checkout", async () => gitCheckoutBranch(projectPath, branch.name));
    setOpen(false);
  };

  const createBranch = async () => {
    if (!projectPath || busy) return;
    const settings = getGitUiSettings();
    const suffix = status?.current_branch ? `${status.current_branch}-new` : "new-branch";
    const suggested = `${settings.branchPrefix}${suffix}`;
    const name = window.prompt(t("git.newBranchName"), suggested)?.trim();
    if (!name) return;
    await runOperation("createBranch", async () => gitCreateBranch(projectPath, name, null));
    setOpen(false);
  };

  const fetchAndRefresh = async () => {
    if (!projectPath || busy) return;
    await runOperation("fetch", async () => gitFetch(projectPath, false));
  };

  return (
    <div className={`relative ${className}`} ref={ref}>
      <button
        type="button"
        className={
          variant === "env"
            ? `flex w-full min-w-0 items-center justify-between rounded-xl px-3 py-2 text-[14px] text-text-base transition-colors hover:bg-gray-100/80 ${
                compact ? "min-h-[38px]" : "min-h-[42px]"
              }`
            : variant === "row"
              ? `flex w-full min-w-0 items-center justify-between rounded-xl px-3 py-2 text-[14px] font-medium text-text-base transition-colors hover:bg-gray-50 ${
                  compact ? "min-h-[38px]" : "min-h-[42px]"
                }`
              : `inline-flex min-w-0 items-center rounded-md text-[12px] font-medium text-text-secondary transition-colors hover:bg-gray-100 hover:text-text-base ${
                  compact ? "px-1.5 py-1" : "px-2 py-1.5"
                }`
        }
        onClick={() => setOpen((value) => !value)}
        title={status?.repo_root ?? projectPath ?? undefined}
      >
        {variant === "env" ? (
          <>
            <div className="flex min-w-0 items-center">
              <FontAwesomeIcon icon={["fas", "desktop"]} className="mr-3 w-4 text-text-secondary" />
              <span className="truncate">{t("chatView.local")}</span>
            </div>
            <div className="ml-3 flex min-w-0 items-center">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-[12px] text-text-secondary" />
              <span className="max-w-[96px] truncate font-medium">{currentLabel ?? t("git.title")}</span>
              {dirty && <span className="ml-2 h-2 w-2 rounded-full bg-amber-500" />}
              <FontAwesomeIcon icon={["fas", open ? "chevron-up" : "chevron-down"]} className="ml-2 text-[11px] text-text-secondary" />
            </div>
          </>
        ) : variant === "row" ? (
          <>
            <div className="flex min-w-0 items-center">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-3 w-4 text-text-secondary" />
              <span className="truncate">{currentLabel ?? t("git.title")}</span>
              {dirty && <span className="ml-2 h-2 w-2 rounded-full bg-amber-500" />}
              {displayAheadBehind && <span className="ml-2 text-[11px] text-blue-500">{displayAheadBehind}</span>}
            </div>
            <FontAwesomeIcon icon={["fas", open ? "chevron-up" : "chevron-down"]} className="ml-3 text-[11px] text-text-secondary" />
          </>
        ) : (
          <>
            <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-[13px]" />
            <span className="max-w-[150px] truncate">{currentLabel ?? t("git.title")}</span>
            {dirty && <span className="ml-1.5 h-1.5 w-1.5 rounded-full bg-amber-500" />}
            {displayAheadBehind && <span className="ml-1.5 text-[11px] text-blue-500">{displayAheadBehind}</span>}
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
          </>
        )}
      </button>

      {open && (
        <div
          className={`absolute z-[70] ${
            dropdownPlacement === "top" ? "bottom-full mb-2" : "top-full mt-2"
          } ${dropdownAlign === "right" ? "right-0" : "left-0"}`}
        >
          <GitBranchDropdown
            branches={branches}
            loading={loading}
            busy={busy}
            query={query}
            onQueryChange={setQuery}
            currentBranch={status?.current_branch ?? null}
            additions={status?.additions ?? changes?.additions ?? 0}
            deletions={status?.deletions ?? changes?.deletions ?? 0}
            filesChanged={status?.files_changed ?? changes?.files.length ?? 0}
            rebaseState={status?.rebase_state ?? null}
            mergeState={!!status?.merge_state}
            operationError={operationError}
            operationResult={operationResult}
            onCheckout={checkoutBranch}
            onFetch={fetchAndRefresh}
            onCreateBranch={createBranch}
            onOpenWorkbench={onOpenWorkbench}
          />
        </div>
      )}
    </div>
  );
}

function GitBranchDropdown({
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
}: {
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
}) {
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

  return (
    <div className="w-[380px] max-w-[calc(100vw-48px)] overflow-hidden rounded-lg border border-border-theme bg-white shadow-[0_12px_36px_rgb(0,0,0,0.14)]">
      <div className="border-b border-border-theme px-3 py-2">
        <div className="flex items-center rounded-md bg-gray-50 px-2 py-1.5 text-[12px] text-text-secondary">
          <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="mr-2 text-[11px]" />
          <input
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            placeholder={t("git.searchBranches")}
            className="w-full bg-transparent outline-none"
            autoFocus
          />
        </div>
      </div>

      <div className="px-3 py-2">
        <div className="flex items-center justify-between rounded-md bg-gray-50 px-3 py-2">
          <div className="min-w-0">
            <div className="flex items-center text-[13px] font-medium text-text-base">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-text-secondary" />
              <span className="truncate">{currentBranch ?? t("git.detachedHead")}</span>
              {(rebaseState || mergeState) && (
                <span className="ml-2 rounded bg-amber-100 px-1.5 py-0.5 text-[10px] text-amber-700">
                  {rebaseState ? t("git.rebasing") : t("git.merging")}
                </span>
              )}
            </div>
            <div className="mt-0.5 text-[11px] text-text-secondary">
              {filesChanged > 0 ? t("git.changedFilesCount", { count: filesChanged }) : t("git.workingTreeClean")}
            </div>
          </div>
          <div className="ml-3 flex items-center gap-1.5 text-[12px] font-medium tabular-nums">
            <span className="text-green-600">+{additions}</span>
            <span className="text-red-500">-{deletions}</span>
          </div>
        </div>
        {operationError && <Message tone="error">{operationError}</Message>}
        {operationResult && <Message tone="success">{operationResult}</Message>}
      </div>

      <div className="max-h-[280px] overflow-y-auto py-1">
        {loading && <div className="px-4 py-2 text-[12px] text-text-secondary">{t("git.loadingState")}</div>}
        {!loading && branches.length === 0 && (
          <div className="px-4 py-2 text-[12px] text-text-secondary">{t("git.noBranchesFound")}</div>
        )}
        <BranchSection title={t("git.localBranches")} branches={local} busy={busy} onCheckout={onCheckout} />
        <BranchSection title={t("git.remoteBranches")} branches={remote} busy={busy} onCheckout={onCheckout} />
      </div>

      <div className="border-t border-border-theme py-1">
        <button
          type="button"
          className="flex w-full items-center px-4 py-2 text-left text-[13px] text-text-base hover:bg-gray-50 disabled:text-text-secondary"
          onClick={onFetch}
          disabled={!!busy}
        >
          <FontAwesomeIcon icon={["fas", "rotate-right"]} className="mr-2.5 w-4 text-text-secondary" />
          {busy === "fetch" ? t("git.fetching") : t("git.fetch")}
        </button>
        <button
          type="button"
          className="flex w-full items-center px-4 py-2 text-left text-[13px] text-text-base hover:bg-gray-50 disabled:text-text-secondary"
          disabled={!!busy}
          onClick={onCreateBranch}
        >
          <FontAwesomeIcon icon={["fas", "plus"]} className="mr-2.5 w-4 text-text-secondary" />
          {t("git.createAndCheckoutBranch")}
        </button>
        <button
          type="button"
          className={`flex w-full items-center px-4 py-2 text-left text-[13px] ${
            onOpenWorkbench ? "text-text-base hover:bg-gray-50" : "text-text-secondary"
          }`}
          disabled={!onOpenWorkbench}
          title={onOpenWorkbench ? t("git.openGit") : t("git.openGitUnavailable")}
          onClick={onOpenWorkbench}
        >
          <FontAwesomeIcon icon={["fas", "table"]} className="mr-2.5 w-4" />
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
}: {
  title: string;
  branches: GitBranch[];
  busy: string | null;
  onCheckout: (branch: GitBranch) => void;
}) {
  const { t } = useTranslation();

  if (branches.length === 0) return null;
  return (
    <div className="py-1">
      <div className="px-4 py-1 text-[11px] font-medium text-text-secondary">{title}</div>
      {branches.map((branch) => {
        const disabled = !!busy || branch.current || !!branch.worktree_path;
        return (
          <button
            type="button"
            key={branch.full_name}
            className="flex w-full items-center justify-between gap-3 px-4 py-2 text-left text-[13px] text-text-base hover:bg-gray-50 disabled:cursor-default disabled:hover:bg-transparent"
            title={branch.worktree_path ? t("git.worktreeAt", { path: branch.worktree_path }) : branch.subject ?? branch.name}
            disabled={disabled}
            onClick={() => onCheckout(branch)}
          >
            <div className="flex min-w-0 items-center">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2.5 w-4 text-text-secondary" />
              <div className="min-w-0">
                <div className="truncate font-medium">{branch.name}</div>
                <div className="truncate text-[11px] text-text-secondary">
                  {branch.worktree_path ? t("git.worktreeAt", { path: branch.worktree_path }) : branch.subject ?? branch.upstream ?? ""}
                </div>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              {(branch.ahead > 0 || branch.behind > 0) && (
                <span className="text-[11px] font-medium text-blue-600">{formatAheadBehind(branch.ahead, branch.behind, t)}</span>
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

function formatAheadBehind(ahead: number, behind: number, t: TFunction): string {
  return [
    ahead > 0 ? t("git.aheadCount", { count: ahead }) : "",
    behind > 0 ? t("git.behindCount", { count: behind }) : "",
  ]
    .filter(Boolean)
    .join(" ");
}
