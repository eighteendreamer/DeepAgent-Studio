import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { gitCheckoutBranch, gitCreateBranch, gitFetch } from "../../api";
import type { GitBranch, GitOperationResult } from "../../types";
import { useGitStatus } from "../../hooks/useGitStatus";
import { getGitUiSettings } from "./gitSettings";
import { GitCreateBranchDialog } from "./GitCreateBranchDialog";
import { GitBranchMenuContent, formatAheadBehind } from "./GitBranchMenuContent";

interface Props {
  projectPath?: string | null;
  compact?: boolean;
  compactMenu?: boolean;
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
  compactMenu = false,
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
  const [createBranchOpen, setCreateBranchOpen] = useState(false);
  const [suggestedBranchName, setSuggestedBranchName] = useState("");
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
        return false;
      }
      if (result) {
        setOperationResult(result.stdout.trim() || t("git.operationCompleted", { action: t(`git.actions.${action}`) }));
      }
      await refresh();
      return true;
    } catch (error) {
      setOperationError(error instanceof Error ? error.message : String(error));
      return false;
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
    const ok = await runOperation("checkout", async () => gitCheckoutBranch(projectPath, branch.name));
    if (ok) setOpen(false);
  };

  const openCreateBranchDialog = () => {
    if (!projectPath || busy) return;
    const settings = getGitUiSettings();
    const suffix = status?.current_branch ? `${status.current_branch}-new` : "new-branch";
    setSuggestedBranchName(`${settings.branchPrefix}${suffix}`);
    setOperationError(null);
    setOperationResult(null);
    setCreateBranchOpen(true);
  };

  const createBranch = async (name: string) => {
    if (!projectPath || busy) return;
    const ok = await runOperation("createBranch", async () => gitCreateBranch(projectPath, name, null));
    if (ok) {
      setCreateBranchOpen(false);
      setOpen(false);
    }
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
          <GitBranchMenuContent
            small={compactMenu}
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
            onCreateBranch={openCreateBranchDialog}
            onOpenWorkbench={onOpenWorkbench}
          />
        </div>
      )}

      <GitCreateBranchDialog
        open={createBranchOpen}
        title={t("git.createBranchDialog.title")}
        label={t("git.newBranchName")}
        initialValue={suggestedBranchName}
        confirmLabel={t("git.actions.createBranch")}
        loading={busy === "createBranch"}
        error={operationError}
        onClose={() => {
          if (busy === "createBranch") return;
          setCreateBranchOpen(false);
        }}
        onConfirm={createBranch}
      />
    </div>
  );
}
