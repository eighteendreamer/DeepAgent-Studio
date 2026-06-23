import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
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
  const [tab, setTab] = useState<Tab>("changes");

  return (
    <div className="flex h-full min-h-0 flex-col bg-white">
      <div className="flex h-12 flex-shrink-0 items-center justify-between border-b border-border-theme px-4">
        <div className="flex min-w-0 items-center">
          <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-text-secondary" />
          <div className="min-w-0">
            <div className="truncate text-[14px] font-medium text-text-base">Git</div>
            <div className="truncate text-[11px] text-text-secondary">
              {status?.current_branch ?? "Git"} · {changes?.files.length ?? status?.files_changed ?? 0} changed
            </div>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <TabButton active={tab === "changes"} onClick={() => setTab("changes")} icon={["fas", "list-check"]} label="变更" />
          <TabButton active={tab === "branches"} onClick={() => setTab("branches")} icon={["fas", "code-branch"]} label="分支" />
          <TabButton active={tab === "push"} onClick={() => setTab("push")} icon={["fas", "upload"]} label="上传" />
          <TabButton active={tab === "log"} onClick={() => setTab("log")} icon={["fas", "code-commit"]} label="修改记录" />
          {onClose && (
            <button
              type="button"
              className="ml-2 h-8 w-8 rounded-md text-text-secondary hover:bg-gray-100 hover:text-text-base"
              onClick={onClose}
              aria-label="Close Git"
            >
              <FontAwesomeIcon icon={["fas", "xmark"]} />
            </button>
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1">
        {tab === "changes" ? (
          <GitChangesPanel
            projectPath={projectPath}
            changes={changes}
            loading={loading}
            onRefresh={onRefresh}
          />
        ) : tab === "branches" ? (
          <GitProjectsPanel
            activeProjectPath={projectPath}
            onRefresh={onRefresh}
          />
        ) : tab === "push" ? (
          <GitPushPanel projectPath={projectPath} onRefresh={onRefresh} />
        ) : (
          <GitLogView projectPath={projectPath} onRefresh={onRefresh} />
        )}
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
        active ? "bg-gray-100 text-text-base" : "text-text-secondary hover:bg-gray-50 hover:text-text-base"
      }`}
      onClick={onClick}
    >
      <FontAwesomeIcon icon={icon as any} className="mr-1.5 text-[11px]" />
      {label}
    </button>
  );
}
