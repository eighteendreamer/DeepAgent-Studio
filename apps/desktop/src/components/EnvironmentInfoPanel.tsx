import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { GitProjectStatus } from "../types";
import { GitBranchChip } from "./git/GitBranchChip";

export type OutputItem =
  | { kind: "url"; label: string }
  | { kind: "file"; label: string; action: "write" | "edit" }
  | { kind: "image"; label: string; source: "tool" | "url" }
  | {
      kind: "todo";
      label: string;
      total: number;
      pending: number;
      inProgress: number;
      completed: number;
    };

interface EnvironmentInfoPanelProps {
  activeProjectPath?: string | null;
  gitStatus: GitProjectStatus | null;
  gitLoading: boolean;
  gitWorkspaceAdditions: number;
  gitWorkspaceDeletions: number;
  gitWorkspaceFilesChanged: number;
  chatChanges: {
    additions: number;
    deletions: number;
  };
  outputItems: OutputItem[];
  onOpenGitWorkbench: () => void;
  onOpenUrl: (url: string) => void;
}

export function EnvironmentInfoPanel({
  activeProjectPath,
  gitStatus,
  gitLoading,
  gitWorkspaceAdditions,
  gitWorkspaceDeletions,
  gitWorkspaceFilesChanged,
  chatChanges,
  outputItems,
  onOpenGitWorkbench,
  onOpenUrl,
}: EnvironmentInfoPanelProps) {
  const { t } = useTranslation();

  const hasProject = !!activeProjectPath;
  const isRepo = !!gitStatus?.is_repo;

  return (
    <div
      className="absolute top-16 right-6 z-10 flex w-[300px] flex-col rounded-2xl border border-border-theme bg-white shadow-[0_12px_36px_rgb(0,0,0,0.10)]"
      style={{ maxHeight: "min(460px, calc(100% - 120px))" }}
    >
      <div className="flex flex-1 flex-col overflow-visible p-5">
        <div className="mb-3 flex items-center justify-between">
          <div className="text-[14px] text-text-secondary">{t("chatView.environmentInfo")}</div>
          <button
            type="button"
            className="h-7 w-7 rounded-md text-text-secondary transition-colors hover:bg-gray-100 hover:text-text-base"
            aria-label={t("chatView.environmentSettings")}
          >
            <FontAwesomeIcon icon={["fas", "gear"]} />
          </button>
        </div>

        <div className="space-y-2 text-[14px]">
          <button
            type="button"
            className="flex w-full items-center justify-between gap-3 rounded-xl px-1 py-1 text-left transition-colors hover:text-text-base disabled:hover:text-text-base"
            onClick={() => {
              if (hasProject && isRepo) onOpenGitWorkbench();
            }}
            disabled={!hasProject || !isRepo}
          >
            <div className="flex min-w-0 items-center text-text-base">
              <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-3 w-4 text-text-secondary" />
              <span>{t("chatView.changes")}</span>
              {hasProject && isRepo && (
                <FontAwesomeIcon icon={["fas", "chevron-right"]} className="ml-2 text-[10px] text-text-secondary" />
              )}
            </div>
            {hasProject ? (
              <div
                className="flex items-center gap-1.5 font-medium tabular-nums"
                title={`Git workspace changes; chat changes +${chatChanges.additions} -${chatChanges.deletions}`}
              >
                {gitLoading ? (
                  <span className="text-[13px] text-text-secondary">{t("chatView.loading")}</span>
                ) : isRepo ? (
                  <>
                    <span className="text-green-600">+{gitWorkspaceAdditions}</span>
                    <span className="text-red-500">-{gitWorkspaceDeletions}</span>
                    {gitWorkspaceFilesChanged > 0 && (
                      <span className="ml-1 text-[11px] text-text-secondary">{gitWorkspaceFilesChanged}</span>
                    )}
                  </>
                ) : (
                  <span className="text-[13px] text-text-secondary">{t("chatView.noGitRepository")}</span>
                )}
              </div>
            ) : (
              <span className="text-[13px] text-text-secondary">{t("chatView.noProject")}</span>
            )}
          </button>

          {hasProject ? (
            gitLoading ? (
              <div className="flex min-w-0 items-center justify-between rounded-xl px-3 py-2 text-text-base">
                <div className="flex min-w-0 items-center">
                  <FontAwesomeIcon icon={["fas", "desktop"]} className="mr-3 w-4 text-text-secondary" />
                  <span className="truncate">{t("chatView.local")}</span>
                </div>
                <span className="truncate text-[13px] text-text-secondary">{t("chatView.loading")}</span>
              </div>
            ) : isRepo && activeProjectPath ? (
              <GitBranchChip
                projectPath={activeProjectPath}
                variant="env"
                dropdownPlacement="bottom"
                dropdownAlign="right"
                onOpenWorkbench={onOpenGitWorkbench}
              />
            ) : (
              <div className="flex min-w-0 items-center justify-between rounded-xl px-3 py-2 text-text-base">
                <div className="flex min-w-0 items-center">
                  <FontAwesomeIcon icon={["fas", "desktop"]} className="mr-3 w-4 text-text-secondary" />
                  <span className="truncate">{t("chatView.local")}</span>
                </div>
                <span className="text-[13px] text-text-secondary">{t("chatView.noGitRepository")}</span>
              </div>
            )
          ) : (
            <div className="flex min-w-0 items-center justify-between rounded-xl px-3 py-2 text-text-base">
              <div className="flex min-w-0 items-center">
                <FontAwesomeIcon icon={["fas", "desktop"]} className="mr-3 w-4 text-text-secondary" />
                <span className="truncate">{t("chatView.local")}</span>
              </div>
              <span className="text-[13px] text-text-secondary">{t("chatView.noProject")}</span>
            </div>
          )}

          <div className="flex min-w-0 items-center rounded-xl px-1 py-1 text-text-base">
            <FontAwesomeIcon icon={["fas", "share-nodes"]} className="mr-3 w-4 text-text-secondary" />
            <span>{t("chatView.commitOrPush")}</span>
          </div>

          <div className="flex min-w-0 items-center rounded-xl px-1 py-1 text-text-secondary">
            <FontAwesomeIcon icon={["fab", "github"]} className="mr-3 w-4 text-text-secondary" />
            <span>
              {gitStatus?.gh_available ? t("chatView.githubCliAvailable") : t("chatView.githubCliUnavailable")}
            </span>
          </div>
        </div>

        <div className="my-5 h-px w-full bg-border-theme"></div>

        <div className="mb-5">
          <div className="mb-3 text-[14px] text-text-secondary">{t("chatView.output")}</div>
          {outputItems.length > 0 ? (
            <div className="custom-scrollbar max-h-48 space-y-2 overflow-y-auto pr-1">
              {outputItems.map((item) => {
                const key =
                  item.kind === "todo"
                    ? `todo:${item.total}:${item.pending}:${item.inProgress}:${item.completed}`
                    : `${item.kind}:${item.label}`;
                const icon: IconProp =
                  item.kind === "url"
                    ? ["fas", "globe"]
                    : item.kind === "image"
                      ? ["far", "image"]
                      : item.kind === "todo"
                        ? ["fas", "list-check"]
                        : ["far", "file-lines"];
                const iconColor =
                  item.kind === "todo"
                    ? "text-blue-500"
                    : item.kind === "image"
                      ? "text-purple-500"
                      : item.kind === "file"
                        ? item.action === "write"
                          ? "text-green-600"
                          : "text-amber-600"
                        : "text-text-secondary";
                const clickable =
                  item.kind === "url" || (item.kind === "image" && item.source === "url");
                const handleItemClick = () => {
                  if (item.kind === "url") onOpenUrl(item.label);
                  else if (item.kind === "image" && item.source === "url") onOpenUrl(item.label);
                };

                return (
                  <div
                    key={key}
                    className={`flex min-w-0 items-center text-[13px] text-text-base transition-colors ${
                      clickable ? "cursor-pointer hover:text-blue-500" : "cursor-default"
                    }`}
                    title={item.label}
                    onClick={clickable ? handleItemClick : undefined}
                  >
                    <FontAwesomeIcon icon={icon} className={`mr-2 w-4 flex-shrink-0 ${iconColor}`} />
                    <span className="flex-1 truncate">{item.label}</span>
                    {item.kind === "file" && (
                      <span className="ml-2 flex-shrink-0 rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-text-secondary">
                        {item.action === "write" ? t("chatView.outputCreated") : t("chatView.outputEdited")}
                      </span>
                    )}
                    {item.kind === "image" && item.source === "tool" && (
                      <span className="ml-2 flex-shrink-0 rounded bg-gray-100 px-1.5 py-0.5 text-[10px] text-text-secondary">
                        {t("chatView.outputCreated")}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="text-[13px] text-text-secondary">{t("chatView.noOutput")}</div>
          )}
        </div>

        <div>
          <div className="mb-3 text-[14px] text-text-secondary">{t("chatView.sources")}</div>
          <div className="text-[13px] text-text-secondary">{t("chatView.noSources")}</div>
        </div>
      </div>
    </div>
  );
}
