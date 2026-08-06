import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { GitChanges, GitProjectStatus } from "../../types";
import { GitChangesPanel } from "./GitChangesPanel";
import { GitLogView } from "./GitLogView";
import { GitProjectsPanel } from "./GitProjectsPanel";
import { GitPushPanel } from "./GitPushPanel";

interface Props {
  projectPath: string;
  status: GitProjectStatus | null;
  changes: GitChanges | null;
  loading?: boolean;
  onRefresh?: () => Promise<void> | void;
  onClose?: () => void;
}

type Tab = "changes" | "branches" | "push" | "log";

export function GitWorkbench({ projectPath, status, changes, loading = false, onRefresh, onClose }: Props) {
  const { t } = useTranslation();
  const [tab, setTab] = useState<Tab>("changes");
  const [visitedTabs, setVisitedTabs] = useState<Set<Tab>>(() => new Set(["changes"]));

  const selectTab = (next: Tab) => {
    setTab(next);
    setVisitedTabs((current) => new Set(current).add(next));
  };

  return (
    <div className="flex h-full min-h-0 flex-col bg-white">
      <div className="flex h-12 flex-shrink-0 items-center justify-between border-b border-border-theme px-4">
        <div className="flex min-w-0 items-center">
          <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-text-secondary" />
          <div className="min-w-0">
            <div className="truncate text-[14px] font-medium text-text-base">{t("git.title")}</div>
            <div className="truncate text-[11px] text-text-secondary">
              {t("git.branchChangedSummary", {
                branch: status?.current_branch ?? t("git.title"),
                count: changes?.files.length ?? status?.files_changed ?? 0,
              })}
            </div>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <TabButton active={tab === "changes"} onClick={() => selectTab("changes")} icon={["fas", "list-check"]} label={t("git.tabs.changes")} />
          <TabButton active={tab === "branches"} onClick={() => selectTab("branches")} icon={["fas", "code-branch"]} label={t("git.tabs.branches")} />
          <TabButton active={tab === "push"} onClick={() => selectTab("push")} icon={["fas", "upload"]} label={t("git.tabs.push")} />
          <TabButton active={tab === "log"} onClick={() => selectTab("log")} icon={["fas", "code-commit"]} label={t("git.tabs.log")} />
          {onClose && (
            <button
              type="button"
              className="ml-2 h-8 w-8 rounded-md text-text-secondary hover:bg-black/5 hover:text-text-base"
              onClick={onClose}
              aria-label={t("git.close")}
            >
              <FontAwesomeIcon icon={["fas", "xmark"]} />
            </button>
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1">
        <div className={tab === "changes" ? "h-full min-h-0" : "hidden h-full min-h-0"}>
          <GitChangesPanel projectPath={projectPath} changes={changes} loading={loading} onRefresh={onRefresh} />
        </div>
        <div className={tab === "branches" ? "h-full min-h-0" : "hidden h-full min-h-0"}>
          {visitedTabs.has("branches") ? <GitProjectsPanel activeProjectPath={projectPath} onRefresh={onRefresh} /> : null}
        </div>
        <div className={tab === "push" ? "h-full min-h-0" : "hidden h-full min-h-0"}>
          {visitedTabs.has("push") ? <GitPushPanel projectPath={projectPath} onRefresh={onRefresh} /> : null}
        </div>
        <div className={tab === "log" ? "h-full min-h-0" : "hidden h-full min-h-0"}>
          {visitedTabs.has("log") ? <GitLogView projectPath={projectPath} onRefresh={onRefresh} /> : null}
        </div>
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  icon,
  label,
}: {
  active: boolean;
  onClick: () => void;
  icon: ["fas", string];
  label: string;
}) {
  return (
    <button
      type="button"
      className={`inline-flex h-8 items-center rounded-md px-2.5 text-[12px] font-medium transition-colors ${
        active ? "bg-black/5 text-text-base" : "text-text-secondary hover:bg-black/5 hover:text-text-base"
      }`}
      onClick={onClick}
    >
      <FontAwesomeIcon icon={icon as any} className="mr-1.5 text-[11px]" />
      {label}
    </button>
  );
}
