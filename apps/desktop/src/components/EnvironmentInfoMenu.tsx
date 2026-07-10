import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { GitProjectStatus } from "../types";
import { sshListConnections, type SshConnection } from "../api";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "./shadcn/dropdown-menu";
import { GitBranchSubMenu } from "./git/GitBranchSubMenu";

type EnvironmentMode = "local" | "remote";

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

interface EnvironmentInfoMenuProps {
  children: ReactNode;
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
  onEnvironmentChange?: (
    mode: EnvironmentMode,
    connectionId: string | null,
    connection?: SshConnection | null,
  ) => void;
}

export function EnvironmentInfoMenu({
  children,
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
  onEnvironmentChange,
}: EnvironmentInfoMenuProps) {
  const { t } = useTranslation();
  const hasProject = !!activeProjectPath;
  const isRepo = !!gitStatus?.is_repo;
  const [envMode, setEnvMode] = useState<EnvironmentMode>(() =>
    localStorage.getItem("envMode") === "remote" ? "remote" : "local",
  );
  const [selectedConnectionId, setSelectedConnectionId] = useState<string | null>(() =>
    localStorage.getItem("ssh_connection_id"),
  );
  const [sshConnections, setSshConnections] = useState<SshConnection[]>([]);

  const loadSshConnections = useCallback(() => {
    sshListConnections().then(setSshConnections).catch(() => setSshConnections([]));
  }, []);

  useEffect(() => {
    loadSshConnections();
  }, [loadSshConnections]);

  useEffect(() => {
    const refreshConnections = () => {
      if (document.visibilityState === "visible") loadSshConnections();
    };
    document.addEventListener("visibilitychange", refreshConnections);
    return () => document.removeEventListener("visibilitychange", refreshConnections);
  }, [loadSshConnections]);

  const selectedConnection = useMemo(
    () => sshConnections.find((connection) => connection.id === selectedConnectionId) ?? null,
    [selectedConnectionId, sshConnections],
  );

  const handleSelectLocal = useCallback(() => {
    setEnvMode("local");
    setSelectedConnectionId(null);
    localStorage.setItem("envMode", "local");
    localStorage.removeItem("ssh_connection_id");
    onEnvironmentChange?.("local", null, null);
  }, [onEnvironmentChange]);

  const handleSelectRemoteConnection = useCallback(
    (connection: SshConnection) => {
      setEnvMode("remote");
      setSelectedConnectionId(connection.id);
      localStorage.setItem("envMode", "remote");
      localStorage.setItem("ssh_connection_id", connection.id);
      onEnvironmentChange?.("remote", connection.id, connection);
    },
    [onEnvironmentChange],
  );

  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>{children}</DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        alignOffset={-128}
        sideOffset={8}
        collisionPadding={8}
        className="w-[300px] p-0"
      >
        <div className="flex max-h-[min(440px,calc(100vh-88px))] flex-col overflow-visible px-5 py-5">
          <div className="mb-3 flex h-6 items-center justify-between">
            <div className="text-[13px] font-normal text-text-secondary">{t("chatView.environmentInfo")}</div>
            <button
              type="button"
              className="flex h-6 w-6 items-center justify-center rounded-md text-[16px] text-text-secondary transition-colors hover:bg-gray-100 hover:text-text-base"
              aria-label={t("chatView.environmentSettings")}
              onClick={(event) => event.preventDefault()}
            >
              <FontAwesomeIcon icon={["fas", "plus"]} />
            </button>
          </div>

          <div className="space-y-1 text-[13px]">
            <button
              type="button"
              className="flex min-h-[30px] w-full items-center justify-between gap-3 rounded-lg px-0 py-0.5 text-left transition-colors hover:text-text-base disabled:hover:text-text-base"
              onClick={() => {
                if (hasProject && isRepo) onOpenGitWorkbench();
              }}
              disabled={!hasProject || !isRepo}
            >
              <div className="flex min-w-0 items-center text-text-base">
                <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-3.5 w-4 text-[13px] text-text-base" />
                <span>{t("chatView.changes")}</span>
                {hasProject && isRepo && (
                  <FontAwesomeIcon icon={["fas", "chevron-right"]} className="ml-2 text-[11px] text-text-secondary" />
                )}
              </div>
              {hasProject ? (
                <div
                  className="flex items-center gap-1.5 text-[13px] font-normal tabular-nums"
                  title={`Git workspace changes; chat changes +${chatChanges.additions} -${chatChanges.deletions}`}
                >
                  {gitLoading ? (
                    <span className="text-[13px] text-text-secondary">{t("chatView.loading")}</span>
                  ) : isRepo ? (
                    <>
                      <span className="text-green-600">+{gitWorkspaceAdditions}</span>
                      <span className="text-red-500">-{gitWorkspaceDeletions}</span>
                      {gitWorkspaceFilesChanged > 0 && (
                        <span className="ml-1 text-[13px] text-text-secondary">{gitWorkspaceFilesChanged}</span>
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

            <EnvironmentModeMenu
              envMode={envMode}
              selectedConnection={selectedConnection}
              connections={sshConnections}
              onSelectLocal={handleSelectLocal}
              onSelectRemoteConnection={handleSelectRemoteConnection}
              onRefreshConnections={loadSshConnections}
            />
            {hasProject && isRepo && activeProjectPath && (
              <GitBranchSubMenu projectPath={activeProjectPath} onOpenWorkbench={onOpenGitWorkbench} />
            )}

            <EnvironmentInfoRow icon={["fas", "share-nodes"]} label={t("chatView.commitOrPush")} />
          </div>

          <div className="my-4 h-px w-full bg-border-theme/80" />

          <div>
            <div className="mb-2 flex h-6 items-center justify-between">
              <div className="text-[13px] text-text-secondary">{t("chatView.sources")}</div>
              <button
                type="button"
                className="flex h-6 w-6 items-center justify-center rounded-md text-[16px] text-text-secondary transition-colors hover:bg-gray-100 hover:text-text-base"
                aria-label={t("chatView.sources")}
                onClick={(event) => event.preventDefault()}
              >
                <FontAwesomeIcon icon={["fas", "plus"]} />
              </button>
            </div>
            {outputItems.length > 0 ? (
              <div className="custom-scrollbar max-h-32 space-y-1 overflow-y-auto pr-1">
                {outputItems.map((item) => (
                  <OutputRow key={outputItemKey(item)} item={item} onOpenUrl={onOpenUrl} />
                ))}
              </div>
            ) : (
              <div className="text-[13px] text-text-secondary">{t("chatView.noSources")}</div>
            )}
            {outputItems.length > 0 && (
              <button
                type="button"
                className="mt-1.5 flex min-h-[30px] w-full items-center rounded-lg text-left text-[13px] text-text-secondary transition-colors hover:text-text-base"
              >
                <FontAwesomeIcon icon={["fas", "link"]} className="mr-3.5 w-4 text-[13px] text-text-secondary" />
                <span>{t("common.viewAll", { defaultValue: "查看全部" })}</span>
              </button>
            )}
          </div>
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function EnvironmentModeMenu({
  envMode,
  selectedConnection,
  connections,
  onSelectLocal,
  onSelectRemoteConnection,
  onRefreshConnections,
}: {
  envMode: EnvironmentMode;
  selectedConnection: SshConnection | null;
  connections: SshConnection[];
  onSelectLocal: () => void;
  onSelectRemoteConnection: (connection: SshConnection) => void;
  onRefreshConnections: () => void;
}) {
  const { t } = useTranslation();
  const label =
    envMode === "local"
      ? t("chatView.localMode")
      : selectedConnection
        ? `${t("chatView.remoteMode")} · ${selectedConnection.name}`
        : t("chatView.remoteMode");

  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex min-h-[30px] w-full min-w-0 items-center justify-between rounded-lg py-0.5 text-left text-text-base outline-none transition-colors hover:text-text-base data-[state=open]:text-text-base"
          onClick={onRefreshConnections}
        >
          <div className="flex min-w-0 items-center">
            <FontAwesomeIcon
              icon={envMode === "local" ? ["fas", "laptop"] : ["fas", "cloud"]}
              className="mr-3.5 w-4 text-[13px] text-text-base"
            />
            <span className="truncate">{label}</span>
          </div>
          <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-3 text-[11px] text-text-secondary" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="right" align="start" sideOffset={10} className="w-[210px] p-1">
        <DropdownMenuItem
          className="min-h-[34px] cursor-pointer justify-between"
          onSelect={onSelectLocal}
        >
          <span className="flex min-w-0 items-center">
            <FontAwesomeIcon icon={["fas", "laptop"]} className="mr-2.5 w-4 text-text-secondary" />
            <span>{t("chatView.localMode")}</span>
          </span>
          {envMode === "local" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-secondary" />}
        </DropdownMenuItem>

        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="flex min-h-[34px] w-full items-center justify-between rounded-md px-3 py-2 text-left text-[13px] text-text-base outline-none transition-colors hover:bg-gray-100 data-[state=open]:bg-gray-100"
              onClick={onRefreshConnections}
            >
              <span className="flex min-w-0 items-center">
                <FontAwesomeIcon icon={["fas", "cloud"]} className="mr-2.5 w-4 text-text-secondary" />
                <span>{t("chatView.remoteMode")}</span>
              </span>
              <span className="ml-3 flex items-center gap-2">
                {envMode === "remote" && (
                  <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-secondary" />
                )}
                <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
              </span>
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="right" align="start" sideOffset={10} className="w-[260px] p-1">
            <DropdownMenuLabel>{t("chatView.existingSshConnections")}</DropdownMenuLabel>
            <DropdownMenuSeparator />
            {connections.length === 0 ? (
              <div className="px-3 py-2 text-[12px] text-text-secondary">{t("chatView.noSshConnections")}</div>
            ) : (
              connections.map((connection) => {
                const checked = envMode === "remote" && selectedConnection?.id === connection.id;
                return (
                  <DropdownMenuItem
                    key={connection.id}
                    className="cursor-pointer items-start gap-3 py-2"
                    onSelect={() => onSelectRemoteConnection(connection)}
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[13px] text-text-base">{connection.name}</div>
                      <div className="truncate text-[11px] text-text-secondary">
                        {connection.username}@{connection.host}:{connection.port}
                      </div>
                    </div>
                    <div className="flex shrink-0 items-center gap-2 pt-0.5">
                      <ConnectionStatusPill status={connection.status} />
                      {checked && (
                        <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-secondary" />
                      )}
                    </div>
                  </DropdownMenuItem>
                );
              })
            )}
          </DropdownMenuContent>
        </DropdownMenu>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ConnectionStatusPill({ status }: { status: SshConnection["status"] }) {
  const { t } = useTranslation();
  const isOnline = status === "connected";
  const isChecking = status === "connecting";
  const isOffline = status === "error";
  const label = isOnline
    ? t("settings.connections.online")
    : isChecking
      ? t("settings.connections.checking")
      : isOffline
        ? t("settings.connections.offline")
        : t("settings.connections.unknown");

  return (
    <span
      className={`inline-flex items-center rounded-full px-1.5 py-0.5 text-[10px] ${
        isOnline
          ? "bg-green-100 text-green-700"
          : isChecking
            ? "bg-yellow-100 text-yellow-700"
            : isOffline
              ? "bg-red-100 text-red-700"
              : "bg-gray-100 text-text-secondary"
      }`}
    >
      <span
        className={`mr-1 h-1.5 w-1.5 rounded-full ${
          isOnline
            ? "bg-green-500"
            : isChecking
              ? "bg-yellow-500"
              : isOffline
                ? "bg-red-500"
                : "bg-gray-400"
        }`}
      />
      {label}
    </span>
  );
}

function EnvironmentInfoRow({
  icon,
  label,
  value,
  muted = false,
  chevron = false,
  external = false,
}: {
  icon: IconProp;
  label: string;
  value?: string;
  muted?: boolean;
  chevron?: boolean;
  external?: boolean;
}) {
  return (
    <div className={`flex min-h-[30px] min-w-0 items-center justify-between rounded-lg py-0.5 ${muted ? "text-text-secondary" : "text-text-base"}`}>
      <div className="flex min-w-0 items-center">
        <FontAwesomeIcon icon={icon} className={`mr-3.5 w-4 text-[13px] ${muted ? "text-text-secondary" : "text-text-base"}`} />
        <span className="truncate">{label}</span>
      </div>
      <div className="ml-3 flex shrink-0 items-center">
        {value && <span className="truncate text-[13px] text-text-secondary">{value}</span>}
        {chevron && <FontAwesomeIcon icon={["fas", "chevron-down"]} className="text-[11px] text-text-secondary" />}
        {external && <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="text-[11px] text-text-secondary" />}
      </div>
    </div>
  );
}

function OutputRow({ item, onOpenUrl }: { item: OutputItem; onOpenUrl: (url: string) => void }) {
  const { t } = useTranslation();
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
  const clickable = item.kind === "url" || (item.kind === "image" && item.source === "url");
  const handleItemClick = () => {
    if (item.kind === "url") onOpenUrl(item.label);
    else if (item.kind === "image" && item.source === "url") onOpenUrl(item.label);
  };

  return (
    <div
      className={`flex min-h-[28px] min-w-0 items-center rounded-lg text-[13px] text-text-secondary transition-colors ${
        clickable ? "cursor-pointer hover:text-blue-500" : "cursor-default"
      }`}
      title={item.label}
      onClick={clickable ? handleItemClick : undefined}
    >
      <FontAwesomeIcon icon={icon} className={`mr-3.5 w-4 flex-shrink-0 text-[13px] ${iconColor}`} />
      <span className="flex-1 truncate">{item.label}</span>
      {item.kind === "file" && (
        <span className="ml-2 flex-shrink-0 rounded bg-gray-100 px-1.5 py-0.5 text-[11px] text-text-secondary">
          {item.action === "write" ? t("chatView.outputCreated") : t("chatView.outputEdited")}
        </span>
      )}
      {item.kind === "image" && item.source === "tool" && (
        <span className="ml-2 flex-shrink-0 rounded bg-gray-100 px-1.5 py-0.5 text-[11px] text-text-secondary">
          {t("chatView.outputCreated")}
        </span>
      )}
    </div>
  );
}

function outputItemKey(item: OutputItem) {
  if (item.kind === "todo") {
    return `todo:${item.total}:${item.pending}:${item.inProgress}:${item.completed}`;
  }
  return `${item.kind}:${item.label}`;
}
