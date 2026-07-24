import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { gitCheckoutBranch, gitCreateBranch, gitFetch } from "../../api";
import type { GitBranch, GitOperationResult } from "../../types";
import { useGitStatus } from "../../hooks/useGitStatus";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "../shadcn/dropdown-menu";
import { getGitUiSettings } from "./gitSettings";
import { GitCreateBranchDialog } from "./GitCreateBranchDialog";
import { GitBranchMenuContent } from "./GitBranchMenuContent";

interface GitBranchSubMenuProps {
  projectPath: string;
  onOpenWorkbench?: () => void;
}

export function GitBranchSubMenu({ projectPath, onOpenWorkbench }: GitBranchSubMenuProps) {
  const { t } = useTranslation();
  const { loading, status, branches, changes, refresh } = useGitStatus(projectPath);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [operationResult, setOperationResult] = useState<string | null>(null);
  const [createBranchOpen, setCreateBranchOpen] = useState(false);
  const [suggestedBranchName, setSuggestedBranchName] = useState("");

  const dirty = !!status?.has_changes;
  const currentLabel = status?.current_branch ?? (loading ? "..." : t("git.title"));

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
        setOperationResult(t("git.operationCompleted", { action: t(`git.actions.${action}`) }));
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
    await runOperation("checkout", async () => gitCheckoutBranch(projectPath, branch.full_name));
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
    if (ok) setCreateBranchOpen(false);
  };

  const fetchAndRefresh = async () => {
    if (!projectPath || busy) return;
    await runOperation("fetch", async () => gitFetch(projectPath, false));
  };

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            className="flex min-h-[30px] w-full min-w-0 items-center justify-between rounded-lg py-0.5 text-left text-[13px] text-text-base outline-none transition-colors hover:text-text-base data-[state=open]:text-text-base"
          >
            <div className="flex min-w-0 items-center">
              <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-3.5 w-4 text-[13px] text-text-base" />
              <span className="truncate">{currentLabel}</span>
              {dirty && <span className="ml-2 h-2 w-2 rounded-full bg-amber-500" />}
            </div>
            <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-3 text-[11px] text-text-secondary" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="left" align="start" sideOffset={8} className="p-0">
          <div onClick={(event) => event.stopPropagation()}>
            <GitBranchMenuContent
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
              className="border-0 shadow-none"
              compact
            />
          </div>
        </DropdownMenuContent>
      </DropdownMenu>

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
    </>
  );
}
