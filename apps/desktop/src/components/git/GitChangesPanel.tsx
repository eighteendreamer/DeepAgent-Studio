import { useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import { gitCommit, gitCommitMessageDraft, gitStage, gitUnstage } from "../../api";
import type { GitChangedFile, GitChanges } from "../../types";
import { GitDiffViewer } from "./GitDiffViewer";

interface Props {
  projectPath: string;
  changes: GitChanges | null;
  loading?: boolean;
  onRefresh?: () => Promise<void> | void;
  onClose?: () => void;
}

const GROUPS = [
  { id: "conflicted", labelKey: "git.groups.conflicted", icon: ["fas", "triangle-exclamation"] as IconProp },
  { id: "staged", labelKey: "git.groups.staged", icon: ["fas", "check"] as IconProp },
  { id: "unstaged", labelKey: "git.groups.unstaged", icon: ["fas", "pen"] as IconProp },
  { id: "untracked", labelKey: "git.groups.untracked", icon: ["far", "file"] as IconProp },
] as const;

export function GitChangesPanel({ projectPath, changes, loading = false, onRefresh, onClose }: Props) {
  const { t } = useTranslation();
  const files = changes?.files ?? [];
  const [selectedPath, setSelectedPath] = useState<string | null>(files[0]?.path ?? null);
  const [commitMessage, setCommitMessage] = useState("");
  const [draftSource, setDraftSource] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);

  useEffect(() => {
    if (files.length === 0) {
      setSelectedPath(null);
      return;
    }
    setSelectedPath((current) => (current && files.some((file) => file.path === current) ? current : files[0].path));
  }, [files]);

  const selected = useMemo(() => {
    if (!files.length) return null;
    return files.find((file) => file.path === selectedPath) ?? files[0];
  }, [files, selectedPath]);

  const totals = {
    additions: changes?.additions ?? 0,
    deletions: changes?.deletions ?? 0,
  };
  const stagedFiles = files.filter((file) => file.category === "staged");

  const runOperation = async (
    action: "stage" | "unstage" | "commit",
    fn: () => Promise<{ ok: boolean; stderr: string; stdout: string }>,
  ) => {
    setBusyAction(action);
    setOperationError(null);
    try {
      const result = await fn();
      if (!result.ok) {
        setOperationError(result.stderr || result.stdout || t("git.operationFailed", { action: t(`git.actions.${action}`) }));
        return false;
      }
      await onRefresh?.();
      return true;
    } catch (error) {
      setOperationError(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setBusyAction(null);
    }
  };

  const stageFile = (file: GitChangedFile) => runOperation("stage", () => gitStage(projectPath, [file.path]));
  const unstageFile = (file: GitChangedFile) => runOperation("unstage", () => gitUnstage(projectPath, [file.path]));

  const commitStaged = async () => {
    const message = commitMessage.trim();
    if (!message) {
      setOperationError(t("git.commitMessageRequired"));
      return;
    }
    if (stagedFiles.length === 0) {
      setOperationError(t("git.stageBeforeCommit"));
      return;
    }
    const ok = window.confirm(t("git.commitConfirm", { count: stagedFiles.length }));
    if (!ok) return;
    const committed = await runOperation("commit", () => gitCommit(projectPath, message));
    if (committed) setCommitMessage("");
  };

  const draftCommitMessage = async () => {
    setBusyAction("draft");
    setOperationError(null);
    setDraftSource(null);
    try {
      const draft = await gitCommitMessageDraft(projectPath);
      if (draft.blocked_reason) {
        setOperationError(draft.blocked_reason);
        return;
      }
      const message = [draft.title, draft.body].filter(Boolean).join("\n\n");
      setCommitMessage(message);
      setDraftSource(draft.source);
    } catch (error) {
      setOperationError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusyAction(null);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-white">
      <div className="flex h-11 flex-shrink-0 items-center justify-between border-b border-border-theme px-4">
        <div className="flex min-w-0 items-center">
          <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-2 text-text-secondary" />
          <span className="text-[14px] font-medium text-text-base">{t("git.changesTitle")}</span>
          <span className="ml-2 text-[12px] text-text-secondary">{t("git.filesCount", { count: files.length })}</span>
          <span className="ml-3 text-[12px] font-medium tabular-nums">
            <span className="text-green-600">+{totals.additions}</span>
            <span className="ml-1.5 text-red-500">-{totals.deletions}</span>
          </span>
        </div>
        {onClose && (
          <button
            type="button"
            className="h-7 w-7 rounded-md text-text-secondary hover:bg-gray-100 hover:text-text-base"
            onClick={onClose}
            aria-label={t("git.closeChanges")}
          >
            <FontAwesomeIcon icon={["fas", "xmark"]} />
          </button>
        )}
      </div>

      <div className="grid min-h-0 flex-1 grid-cols-[320px_minmax(0,1fr)] xl:grid-cols-[360px_minmax(0,1fr)]">
        <div className="flex min-h-0 min-w-0 flex-col border-r border-border-theme bg-gray-50/60">
          <div className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto py-2">
            {loading ? (
              <div className="px-4 py-3 text-[13px] text-text-secondary">{t("git.loadingChanges")}</div>
            ) : files.length === 0 ? (
              <div className="px-4 py-3 text-[13px] text-text-secondary">{t("git.workingTreeClean")}</div>
            ) : (
              GROUPS.map((group) => {
                const groupFiles = files.filter((file) => file.category === group.id);
                if (groupFiles.length === 0) return null;
                return (
                  <div key={group.id} className="mb-2">
                    <div className="flex items-center px-3 py-1 text-[11px] font-medium text-text-secondary">
                      <FontAwesomeIcon icon={group.icon} className="mr-2 w-3" />
                      {t(group.labelKey)}
                      <span className="ml-1.5">{groupFiles.length}</span>
                    </div>
                    {groupFiles.map((file) => (
                      <ChangedFileRow
                        key={`${file.category}:${file.path}`}
                        file={file}
                        active={selected?.path === file.path}
                        busy={busyAction !== null}
                        onClick={() => setSelectedPath(file.path)}
                        onStage={() => stageFile(file)}
                        onUnstage={() => unstageFile(file)}
                      />
                    ))}
                  </div>
                );
              })
            )}
          </div>
          <div className="border-t border-border-theme bg-white p-3">
            <div className="mb-2 flex items-center justify-between gap-2">
              <div className="min-w-0 text-[11px] text-text-secondary">
                {draftSource === "staged"
                  ? t("git.draftedFromStaged")
                  : draftSource === "working_tree"
                    ? t("git.draftedFromWorkingTree")
                    : t("git.commitMessage")}
              </div>
              <button
                type="button"
                disabled={busyAction !== null || files.length === 0}
                onClick={draftCommitMessage}
                className="inline-flex h-7 items-center rounded-md border border-border-theme bg-white px-2 text-[11px] font-medium text-text-base hover:bg-gray-100 disabled:opacity-50"
              >
                <FontAwesomeIcon icon={["fas", "lightbulb"]} className="mr-1.5 text-[10px]" />
                {busyAction === "draft" ? t("git.draftingMessage") : t("git.draftMessage")}
              </button>
            </div>
            <textarea
              value={commitMessage}
              onChange={(event) => setCommitMessage(event.target.value)}
              placeholder={t("git.commitMessage")}
              className="h-28 w-full resize-none rounded-lg border border-border-theme bg-white px-3 py-2 text-[12px] text-text-base outline-none focus:border-primary/60"
            />
            {operationError && (
              <div className="mt-2 rounded-md bg-red-50 px-2 py-1.5 text-[11px] text-red-600">
                {operationError}
              </div>
            )}
            <button
              type="button"
              disabled={busyAction !== null || stagedFiles.length === 0 || !commitMessage.trim()}
              onClick={commitStaged}
              className="mt-2 inline-flex h-8 w-full items-center justify-center rounded-md bg-text-base px-3 text-[12px] font-medium text-white transition-colors hover:bg-primary disabled:cursor-not-allowed disabled:bg-gray-300"
            >
              {busyAction === "commit" ? t("git.committing") : t("git.commitStagedFiles", { count: stagedFiles.length })}
            </button>
          </div>
        </div>

        <div className="min-h-0 min-w-0 overflow-hidden">
          {selected ? (
            <GitDiffViewer projectPath={projectPath} file={selected} onRefresh={onRefresh} />
          ) : (
            <div className="flex h-full items-center justify-center text-[13px] text-text-secondary">
              {t("git.selectFileToViewDiff")}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function ChangedFileRow({
  file,
  active,
  busy,
  onClick,
  onStage,
  onUnstage,
}: {
  file: GitChangedFile;
  active: boolean;
  busy: boolean;
  onClick: () => void;
  onStage: () => void;
  onUnstage: () => void;
}) {
  const { t } = useTranslation();
  const canStage = file.category === "unstaged" || file.category === "untracked";
  const canUnstage = file.category === "staged";

  return (
    <div
      className={`group flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-[12px] transition-colors ${
        active ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:bg-white/70 hover:text-text-base"
      }`}
      title={file.old_path ? `${file.old_path} -> ${file.path}` : file.path}
    >
      <button type="button" className="flex min-w-0 flex-1 items-center text-left" onClick={onClick}>
        <span className={`mr-2 w-7 text-[11px] font-semibold ${statusColor(file.status)}`}>{file.status.trim() || "M"}</span>
        <span className="truncate">{file.path}</span>
      </button>
      <div className="ml-2 flex flex-shrink-0 items-center gap-1.5">
        <div className="flex items-center gap-1 text-[11px] font-medium tabular-nums">
          <span className="text-green-600">+{file.additions}</span>
          <span className="text-red-500">-{file.deletions}</span>
        </div>
        {canStage && (
          <button
            type="button"
            disabled={busy}
            onClick={onStage}
            className="hidden rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-text-secondary hover:bg-blue-50 hover:text-blue-600 disabled:opacity-50 group-hover:inline-flex"
          >
            {t("git.stage")}
          </button>
        )}
        {canUnstage && (
          <button
            type="button"
            disabled={busy}
            onClick={onUnstage}
            className="hidden rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-text-secondary hover:bg-amber-50 hover:text-amber-700 disabled:opacity-50 group-hover:inline-flex"
          >
            {t("git.unstage")}
          </button>
        )}
      </div>
    </div>
  );
}

function statusColor(status: string): string {
  if (status.includes("U")) return "text-red-600";
  if (status.includes("?")) return "text-blue-600";
  if (status.includes("A")) return "text-green-600";
  if (status.includes("D")) return "text-red-500";
  if (status.includes("R")) return "text-purple-600";
  return "text-amber-600";
}
