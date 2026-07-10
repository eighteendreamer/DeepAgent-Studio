import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";
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

const BASE_SIDEBAR_WIDTH = 320;
const WIDE_SIDEBAR_WIDTH = 360;

function defaultSidebarWidth() {
  if (typeof window !== "undefined" && window.innerWidth >= 1280) {
    return WIDE_SIDEBAR_WIDTH;
  }
  return BASE_SIDEBAR_WIDTH;
}

export function GitChangesPanel({ projectPath, changes, loading = false, onRefresh, onClose }: Props) {
  const { t } = useTranslation();
  const [localChanges, setLocalChanges] = useState<GitChanges | null>(changes);
  const files = localChanges?.files ?? [];
  const originalSidebarWidth = useMemo(() => defaultSidebarWidth(), []);
  const minSidebarWidth = originalSidebarWidth / 2;
  const maxSidebarWidth = originalSidebarWidth;
  const [sidebarWidth, setSidebarWidth] = useState(originalSidebarWidth);
  const [selectedPath, setSelectedPath] = useState<string | null>(files[0]?.path ?? null);
  const [commitMessage, setCommitMessage] = useState("");
  const [draftSource, setDraftSource] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [operationError, setOperationError] = useState<string | null>(null);
  const [selectedPathsByCategory, setSelectedPathsByCategory] = useState<Record<string, Set<string>>>(() => ({}));
  const knownPathsByCategoryRef = useRef<Record<string, Set<string>>>({});

  useEffect(() => {
    setLocalChanges(changes);
  }, [changes]);

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
    additions: localChanges?.additions ?? 0,
    deletions: localChanges?.deletions ?? 0,
  };
  const stagedFiles = useMemo(() => files.filter((file) => file.category === "staged"), [files]);
  const filesSelectionKey = useMemo(
    () => files.map((file) => `${file.category}:${file.path}`).join("\0"),
    [files],
  );
  const selectedStagedFiles = useMemo(
    () => stagedFiles.filter((file) => selectedPathsByCategory.staged?.has(file.path)),
    [selectedPathsByCategory, stagedFiles],
  );

  useEffect(() => {
    const nextKnown: Record<string, Set<string>> = {};
    for (const file of files) {
      nextKnown[file.category] ??= new Set();
      nextKnown[file.category].add(file.path);
    }
    setSelectedPathsByCategory((current) => {
      const next: Record<string, Set<string>> = {};
      for (const [category, paths] of Object.entries(nextKnown)) {
        const previousKnown = knownPathsByCategoryRef.current[category] ?? new Set<string>();
        const previousSelected = current[category] ?? new Set<string>();
        const selected = new Set([...previousSelected].filter((path) => paths.has(path)));
        for (const path of paths) {
          if (!previousKnown.has(path)) {
            selected.add(path);
          }
        }
        next[category] = selected;
      }
      return next;
    });
    knownPathsByCategoryRef.current = nextKnown;
  }, [filesSelectionKey]);

  const runOperation = async (
    action: "stage" | "unstage" | "commit",
    fn: () => Promise<{ ok: boolean; stderr: string; stdout: string }>,
    options?: {
      optimisticFiles?: GitChangedFile[];
      optimisticCategory?: "staged" | "unstaged" | "untracked";
      backgroundRefresh?: boolean;
    },
  ) => {
    setBusyAction(action);
    setOperationError(null);
    try {
      const result = await fn();
      if (!result.ok) {
        setOperationError(result.stderr || result.stdout || t("git.operationFailed", { action: t(`git.actions.${action}`) }));
        return false;
      }
      if (options?.optimisticFiles?.length && options.optimisticCategory) {
        applyOptimisticCategory(options.optimisticFiles, options.optimisticCategory);
      }
      const refresh = onRefresh?.();
      if (!options?.backgroundRefresh) {
        await refresh;
      }
      return true;
    } catch (error) {
      setOperationError(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setBusyAction(null);
    }
  };

  const applyOptimisticCategory = (targets: GitChangedFile[], category: "staged" | "unstaged" | "untracked") => {
    const targetPaths = new Set(targets.map((file) => file.path));
    setLocalChanges((current) => {
      if (!current) return current;
      const nextFiles = current.files.map((file) =>
        targetPaths.has(file.path) ? optimisticGitFile(file, category) : file,
      );
      const totals = nextFiles.reduce(
        (sum, file) => ({
          additions: sum.additions + file.additions,
          deletions: sum.deletions + file.deletions,
        }),
        { additions: 0, deletions: 0 },
      );
      return { ...current, files: nextFiles, additions: totals.additions, deletions: totals.deletions };
    });
  };

  const stageFile = (file: GitChangedFile) =>
    runOperation("stage", () => gitStage(projectPath, [file.path]), {
      optimisticFiles: [file],
      optimisticCategory: "staged",
      backgroundRefresh: true,
    });
  const unstageFile = (file: GitChangedFile) =>
    runOperation("unstage", () => gitUnstage(projectPath, [file.path]), {
      optimisticFiles: [file],
      optimisticCategory: file.status[0] === "A" ? "untracked" : "unstaged",
      backgroundRefresh: true,
    });
  const toggleFileSelection = (category: string, path: string, selected: boolean) => {
    setSelectedPathsByCategory((current) => {
      const next = { ...current };
      const categorySelected = new Set(next[category] ?? []);
      if (selected) {
        categorySelected.add(path);
      } else {
        categorySelected.delete(path);
      }
      next[category] = categorySelected;
      return next;
    });
  };
  const toggleAllGroupFiles = (category: string, groupFiles: GitChangedFile[]) => {
    const paths = groupFiles.map((file) => file.path);
    setSelectedPathsByCategory((current) => ({
      ...current,
      [category]: (current[category]?.size ?? 0) === paths.length ? new Set() : new Set(paths),
    }));
  };
  const stageSelectedFiles = async (targets: GitChangedFile[]) => {
    if (targets.length === 0) {
      setOperationError(t("git.selectFilesBeforeStage"));
      return;
    }
    await runOperation("stage", () => gitStage(projectPath, targets.map((file) => file.path)), {
      optimisticFiles: targets,
      optimisticCategory: "staged",
      backgroundRefresh: true,
    });
  };
  const unstageSelectedFiles = async (targets: GitChangedFile[]) => {
    if (targets.length === 0) {
      setOperationError(t("git.selectStagedFilesBeforeUnstage"));
      return;
    }
    await runOperation("unstage", () => gitUnstage(projectPath, targets.map((file) => file.path)), {
      optimisticFiles: targets,
      optimisticCategory: targets.some((file) => file.status[0] !== "A") ? "unstaged" : "untracked",
      backgroundRefresh: true,
    });
  };

  const startSidebarResize = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = sidebarWidth;
    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;

    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    const onMouseMove = (moveEvent: MouseEvent) => {
      const nextWidth = startWidth + moveEvent.clientX - startX;
      setSidebarWidth(Math.max(minSidebarWidth, Math.min(maxSidebarWidth, nextWidth)));
    };

    const onMouseUp = () => {
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    };

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  };

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
    if (selectedStagedFiles.length === 0) {
      setOperationError(t("git.selectStagedFilesBeforeCommit"));
      return;
    }
    const selectedPaths = selectedStagedFiles.map((file) => file.path);
    if (selectedPaths.length < stagedFiles.length && selectedStagedFiles.some(hasUnstagedPart)) {
      setOperationError(t("git.selectedFileHasUnstagedChanges"));
      return;
    }
    const ok = window.confirm(t("git.commitConfirm", { count: selectedPaths.length }));
    if (!ok) return;
    const committed = await runOperation("commit", () =>
      gitCommit(projectPath, message, selectedPaths.length === stagedFiles.length ? undefined : selectedPaths),
    );
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

      <div
        className="grid min-h-0 flex-1"
        style={{ gridTemplateColumns: `${sidebarWidth}px 8px minmax(0,1fr)` }}
      >
        <div className="flex min-h-0 min-w-0 flex-col border-r border-border-theme bg-gray-50/60">
          <div className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto py-2">
            {loading && files.length === 0 ? (
              <div className="px-4 py-3 text-[13px] text-text-secondary">{t("git.loadingChanges")}</div>
            ) : files.length === 0 ? (
              <div className="px-4 py-3 text-[13px] text-text-secondary">{t("git.workingTreeClean")}</div>
            ) : (
              GROUPS.map((group) => {
                const groupFiles = files.filter((file) => file.category === group.id);
                if (groupFiles.length === 0) return null;
                const selectedInGroup = groupFiles.filter((file) => selectedPathsByCategory[group.id]?.has(file.path));
                const canBatchStage = group.id === "unstaged" || group.id === "untracked";
                const canBatchUnstage = group.id === "staged";
                return (
                  <div key={group.id} className="mb-2">
                    <div className="flex items-center justify-between px-3 py-1 text-[11px] font-medium text-text-secondary">
                      <div className="flex min-w-0 items-center">
                        <FontAwesomeIcon icon={group.icon} className="mr-2 w-3" />
                        {t(group.labelKey)}
                        <span className="ml-1.5">{groupFiles.length}</span>
                      </div>
                      <div className="flex items-center gap-1">
                        <button
                          type="button"
                          className="rounded px-1.5 py-0.5 text-[10px] text-text-secondary hover:bg-white hover:text-text-base"
                          onClick={() => toggleAllGroupFiles(group.id, groupFiles)}
                        >
                          {selectedInGroup.length === groupFiles.length ? t("git.deselectAll") : t("git.selectAll")}
                        </button>
                        {canBatchStage && (
                          <button
                            type="button"
                            disabled={busyAction !== null || selectedInGroup.length === 0}
                            className="rounded bg-white px-1.5 py-0.5 text-[10px] text-text-secondary hover:text-blue-600 disabled:opacity-50"
                            onClick={() => stageSelectedFiles(selectedInGroup)}
                          >
                            {t("git.stageSelected", { count: selectedInGroup.length })}
                          </button>
                        )}
                        {canBatchUnstage && (
                          <button
                            type="button"
                            disabled={busyAction !== null || selectedInGroup.length === 0}
                            className="rounded bg-white px-1.5 py-0.5 text-[10px] text-text-secondary hover:text-amber-700 disabled:opacity-50"
                            onClick={() => unstageSelectedFiles(selectedInGroup)}
                          >
                            {t("git.unstageSelected", { count: selectedInGroup.length })}
                          </button>
                        )}
                      </div>
                    </div>
                    {groupFiles.map((file) => (
                      <ChangedFileRow
                        key={`${file.category}:${file.path}`}
                        file={file}
                        active={selected?.path === file.path}
                        busy={busyAction !== null}
                        checked={isSelectableChange(file) ? (selectedPathsByCategory[file.category]?.has(file.path) ?? false) : undefined}
                        onClick={() => setSelectedPath(file.path)}
                        onToggleChecked={
                          isSelectableChange(file) ? (checked) => toggleFileSelection(file.category, file.path, checked) : undefined
                        }
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
              disabled={busyAction !== null || selectedStagedFiles.length === 0 || !commitMessage.trim()}
              onClick={commitStaged}
              className="mt-2 inline-flex h-8 w-full items-center justify-center rounded-md bg-text-base px-3 text-[12px] font-medium text-white transition-colors hover:bg-primary disabled:cursor-not-allowed disabled:bg-gray-300"
            >
              {busyAction === "commit" ? t("git.committing") : t("git.commitStagedFiles", { count: selectedStagedFiles.length })}
            </button>
          </div>
        </div>

        <div
          className="group relative flex min-h-0 cursor-col-resize items-stretch justify-center bg-white"
          onMouseDown={startSidebarResize}
          role="separator"
          aria-orientation="vertical"
          aria-valuemin={minSidebarWidth}
          aria-valuemax={maxSidebarWidth}
          aria-valuenow={Math.round(sidebarWidth)}
          title="拖动调整宽度"
        >
          <div className="h-full w-px bg-border-theme transition-colors group-hover:bg-gray-400" />
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
  checked,
  onClick,
  onToggleChecked,
  onStage,
  onUnstage,
}: {
  file: GitChangedFile;
  active: boolean;
  busy: boolean;
  checked?: boolean;
  onClick: () => void;
  onToggleChecked?: (checked: boolean) => void;
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
      {onToggleChecked && (
        <input
          type="checkbox"
          checked={Boolean(checked)}
          disabled={busy}
          onChange={(event) => onToggleChecked(event.target.checked)}
          onClick={(event) => event.stopPropagation()}
          className="mr-2 h-3.5 w-3.5 flex-shrink-0 rounded border-border-theme accent-text-base"
          aria-label={file.path}
        />
      )}
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

function hasUnstagedPart(file: GitChangedFile): boolean {
  return file.status.length > 1 && file.status[1] !== " ";
}

function isSelectableChange(file: GitChangedFile): boolean {
  return file.category === "staged" || file.category === "unstaged" || file.category === "untracked";
}

function optimisticGitFile(
  file: GitChangedFile,
  targetCategory: "staged" | "unstaged" | "untracked",
): GitChangedFile {
  if (targetCategory === "staged") {
    return {
      ...file,
      category: "staged",
      status: file.category === "untracked" ? "A " : `${file.status.trim()[0] || "M"} `,
    };
  }
  if (targetCategory === "untracked" || file.status[0] === "A") {
    return {
      ...file,
      category: "untracked",
      status: "??",
    };
  }
  return {
    ...file,
    category: "unstaged",
    status: ` ${file.status.trim()[0] || "M"}`,
  };
}
