import { useState, useRef, useEffect, useCallback, useId, useLayoutEffect } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { Composer } from "./Composer";
import { BalanceChip } from "./BalanceChip";
import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import type { ComposerAttachment, ComposerMention, ComposerSkillSelection, Project } from "../types";
import { sshListConnections, type SshConnection } from "../api";
import { ToolLauncherPanel } from "./ToolLauncherPanel";
import { GitBranchChip } from "./git/GitBranchChip";
import {
  createPluginTab,
  PLUGIN_TOOL_CARDS,
  renderPluginTab,
  type PluginTab,
  type PluginToolCard,
} from "./plugins/pluginRegistry";
import { RightSidebarWorkbench } from "./RightSidebarWorkbench";
import { usePanelPresence } from "../hooks/usePanelPresence";
import { MENU_LIST } from "./ui/motion";
import { MENU_ITEM_ATTR, SlidingMenuList } from "./ui/SlidingMenuList";
import { Panel } from "./ui/Panel";
import { cn } from "./shadcn/utils";
import { MorphingToolbarMenu } from "./ui/MorphingToolbarMenu";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";
const PROJECT_MAP_TAB_ID = "project-map";

/** 项目下拉 —— 与 Git 分支菜单同宽：px-2 外框 + 药丸贴齐内容区 */
const PROJECT_MENU = {
  padX: "px-2",
  divider: "mx-2 my-1.5 h-px shrink-0 bg-border-theme opacity-[0.55]",
  searchWrap: "px-2 pb-2.5 pt-3",
  searchBar: "flex items-center text-[13px] text-text-secondary",
  icon: "mr-2 w-3.5 shrink-0 text-[13px] text-text-secondary",
  row: "flex w-full cursor-pointer items-center justify-between rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base",
  pill: "left-0 right-0 rounded-lg",
} as const;

/** 环境模式下拉 —— 同行宽 + 远程子菜单 */
const ENV_MENU = {
  pad: "px-2 py-1.5",
  icon: "mr-2 w-3.5 shrink-0 text-[13px] text-text-secondary",
  row: "flex w-full cursor-pointer items-center justify-between rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base",
  rowMulti: "flex w-full cursor-pointer items-start justify-between gap-3 rounded-lg px-2.5 py-2 text-left text-[13px] text-text-base",
  pill: "left-0 right-0 rounded-lg",
} as const;

interface Props {
  projectName: string;
  activeProjectPath?: string | null;
  projectMapOpenSignal?: number;
  projects: Project[];
  onSelectProject: (path: string) => void;
  onAddProject: () => void;
  onSubmit: (
    text: string,
    attachments: ComposerAttachment[],
    envMode: "local" | "remote",
    connectionId: string | null,
    selectedSkills?: ComposerSkillSelection[],
    mentions?: ComposerMention[],
    displayText?: string,
  ) => void;
}

export function StartView({ projectName, activeProjectPath = null, projectMapOpenSignal = 0, projects, onSelectProject, onAddProject, onSubmit }: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [isDropdownOpen, setIsDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const [isGitMenuOpen, setIsGitMenuOpen] = useState(false);
  const [composerOverlayCloseSignal, setComposerOverlayCloseSignal] = useState(0);
  const [isComposerOverlayOpen, setIsComposerOverlayOpen] = useState(false);
  const [isEnvDropdownOpen, setIsEnvDropdownOpen] = useState(false);
  const [envMode, setEnvMode] = useState<"local" | "remote">(() => (localStorage.getItem("envMode") as any) || "local");
  const [selectedConnectionId, setSelectedConnectionId] = useState<string | null>(
    () => localStorage.getItem("ssh_connection_id")
  );
  const [sshConnections, setSshConnections] = useState<SshConnection[]>([]);
  const [isRemoteSubmenuOpen, setIsRemoteSubmenuOpen] = useState(false);
  const envPadRef = useRef<HTMLDivElement>(null);
  const remoteRowRef = useRef<HTMLDivElement>(null);
  const [remoteFlyoutAnchor, setRemoteFlyoutAnchor] = useState({ top: 0, height: 36 });
  const projectMorphLayoutId = useId().replace(/:/g, "");
  const envMorphLayoutId = useId().replace(/:/g, "");

  const syncRemoteFlyoutAnchor = useCallback(() => {
    const pad = envPadRef.current;
    const row = remoteRowRef.current;
    if (!pad || !row) return;
    const padRect = pad.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    setRemoteFlyoutAnchor({ top: rowRect.top - padRect.top, height: rowRect.height });
  }, []);

  useLayoutEffect(() => {
    if (!isEnvDropdownOpen) return;
    syncRemoteFlyoutAnchor();
  }, [isEnvDropdownOpen, isRemoteSubmenuOpen, envMode, syncRemoteFlyoutAnchor]);

  useEffect(() => {
    if (!isEnvDropdownOpen) return;
    const pad = envPadRef.current;
    if (!pad) return;
    const ro = new ResizeObserver(() => syncRemoteFlyoutAnchor());
    ro.observe(pad);
    return () => ro.disconnect();
  }, [isEnvDropdownOpen, syncRemoteFlyoutAnchor]);

  const closeFooterMenus = useCallback(() => {
    setIsDropdownOpen(false);
    setIsEnvDropdownOpen(false);
    setIsRemoteSubmenuOpen(false);
    setIsGitMenuOpen(false);
  }, []);

  const closeComposerOverlays = useCallback(() => {
    setComposerOverlayCloseSignal((signal) => signal + 1);
  }, []);

  const handleEnvModeChange = (mode: "local" | "remote") => {
    setEnvMode(mode);
    localStorage.setItem("envMode", mode);
    if (mode === "local") {
      setSelectedConnectionId(null);
      localStorage.removeItem("ssh_connection_id");
    }
  };

  const handleRemoteConnectionSelect = (connectionId: string) => {
    setEnvMode("remote");
    setSelectedConnectionId(connectionId);
    localStorage.setItem("envMode", "remote");
    localStorage.setItem("ssh_connection_id", connectionId);
    setIsEnvDropdownOpen(false);
    setIsRemoteSubmenuOpen(false);
  };

  const loadSshConnections = useCallback(() => {
    sshListConnections().then(setSshConnections).catch(() => setSshConnections([]));
  }, []);

  const [isBottomPanelOpen, setIsBottomPanelOpen] = useState(false);
  const [isRightSidebarOpen, setIsRightSidebarOpen] = useState(false);
  const [sidebarTabs, setSidebarTabs] = useState<PluginTab[]>([]);
  const [activeSidebarTabId, setActiveSidebarTabId] = useState<string>("new");
  const [bottomTabs, setBottomTabs] = useState<PluginTab[]>([]);
  const [activeBottomTabId, setActiveBottomTabId] = useState<string>("new");

  const [bottomPanelHeight, setBottomPanelHeight] = useState(280);
  const [isResizingBottom, setIsResizingBottom] = useState(false);
  const bottomPanelPresence = usePanelPresence(isBottomPanelOpen, 400);

  useEffect(() => {
    if (!isResizingBottom) return;
    const handleMouseMove = (e: MouseEvent) => {
      const newHeight = window.innerHeight - e.clientY;
      if (newHeight > 200 && newHeight < window.innerHeight - 100) {
        setBottomPanelHeight(newHeight);
      }
    };
    const handleMouseUp = () => setIsResizingBottom(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizingBottom]);

  const handleOpenBottomPlugin = (c: PluginToolCard) => {
    const tabOptions =
      c.pluginAppId || c.pluginId
        ? { id: c.id ? `${c.id}-${Date.now()}` : undefined, title: c.title }
        : undefined;
    const newTab = createPluginTab(c.type, {
      activeProjectPath,
      envMode,
      selectedConnection:
        sshConnections.find((connection) => connection.id === selectedConnectionId) ?? null,
      t,
    }, tabOptions);
    setBottomTabs([...bottomTabs, newTab]);
    setActiveBottomTabId(newTab.id);
  };

  const handleToggleBottomTerminalPanel = () => {
    if (isBottomPanelOpen) {
      setIsBottomPanelOpen(false);
    } else {
      setIsBottomPanelOpen(true);
      if (!bottomTabs.some((t) => t.type === "terminal")) {
        const terminalCard = PLUGIN_TOOL_CARDS.find((c) => c.type === "terminal");
        if (terminalCard) {
          void handleOpenBottomPlugin(terminalCard);
        }
      } else {
        const termTab = bottomTabs.find((t) => t.type === "terminal");
        if (termTab) setActiveBottomTabId(termTab.id);
      }
    }
  };

  const handleOpenSidebarPlugin = (c: PluginToolCard) => {
    const tabOptions =
      c.pluginAppId || c.pluginId
        ? { id: c.id ? `${c.id}-${Date.now()}` : undefined, title: c.title }
        : undefined;
    const newTab = createPluginTab(c.type, {
      activeProjectPath,
      envMode,
      selectedConnection:
        sshConnections.find((connection) => connection.id === selectedConnectionId) ?? null,
      t,
    }, tabOptions);
    setSidebarTabs((tabs) => [...tabs, newTab]);
    setActiveSidebarTabId(newTab.id);
  };

  const closeSidebarTab = (tabId: string) => {
    setSidebarTabs((tabs) => {
      const newTabs = tabs.filter((tab) => tab.id !== tabId);
      setActiveSidebarTabId((current) =>
        current === tabId ? (newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new") : current,
      );
      if (newTabs.length === 0) setIsRightSidebarOpen(false);
      return newTabs;
    });
  };

  const openProjectMapSidebar = useCallback(() => {
    setIsRightSidebarOpen(true);
    setSidebarTabs((tabs) => {
      const existingTab = tabs.find((tab) => tab.type === "project_map");
      setActiveSidebarTabId(existingTab?.id ?? PROJECT_MAP_TAB_ID);
      if (existingTab) return tabs;
      return [
        ...tabs,
        {
          id: PROJECT_MAP_TAB_ID,
          type: "project_map",
          title: t("chatView.tools.project_map", { defaultValue: "项目地图" }),
          icon: ["fas", "share-nodes"],
        },
      ];
    });
  }, [t]);

  useEffect(() => {
    if (projectMapOpenSignal > 0) openProjectMapSidebar();
  }, [openProjectMapSidebar, projectMapOpenSignal]);

  useEffect(() => {
    const onOpen = () => openProjectMapSidebar();
    window.addEventListener(PROJECT_MAP_OPEN_EVENT, onOpen);
    return () => window.removeEventListener(PROJECT_MAP_OPEN_EVENT, onOpen);
  }, [openProjectMapSidebar]);


  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        closeFooterMenus();
      }
    };
    if (isDropdownOpen || isEnvDropdownOpen || isGitMenuOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isDropdownOpen, isEnvDropdownOpen, isGitMenuOpen, closeFooterMenus]);

  useEffect(() => {
    if (!isEnvDropdownOpen) {
      setIsRemoteSubmenuOpen(false);
      return;
    }

    loadSshConnections();
  }, [isEnvDropdownOpen, loadSshConnections]);

  useEffect(() => {
    if (envMode === "remote" || selectedConnectionId) {
      loadSshConnections();
    }
  }, [envMode, selectedConnectionId, loadSshConnections]);

  useEffect(() => {
    if (envMode !== "remote" && !selectedConnectionId && !isEnvDropdownOpen) return;

    const refreshConnections = () => {
      if (document.visibilityState === "visible") {
        loadSshConnections();
      }
    };

    refreshConnections();
    const intervalId = window.setInterval(refreshConnections, 15000);
    document.addEventListener("visibilitychange", refreshConnections);

    return () => {
      window.clearInterval(intervalId);
      document.removeEventListener("visibilitychange", refreshConnections);
    };
  }, [envMode, isEnvDropdownOpen, loadSshConnections, selectedConnectionId]);

  const selectedConnection =
    sshConnections.find((conn) => conn.id === selectedConnectionId) ?? null;
  const envLabel =
    envMode === "local"
      ? t("chatView.localMode")
      : selectedConnection
      ? `${t("chatView.remoteMode")} · ${selectedConnection.name}`
      : t("chatView.remoteMode");

  const submit = (
    attachments: ComposerAttachment[] = [],
    selectedSkills: ComposerSkillSelection[] = [],
    mentions: ComposerMention[] = [],
    displayText?: string,
  ) => {
    onSubmit(value.trim(), attachments, envMode, selectedConnectionId, selectedSkills, mentions, displayText);
    setValue("");
  };
  const activeBottomTab =
    bottomTabs.find((tab) => tab.id === activeBottomTabId) ?? null;

  return (
    <div className="w-full h-full min-w-0 overflow-hidden flex flex-col relative">
      {/* Top-right action buttons: sidebar + terminal, fixed position in all states. */}
      <div className="absolute top-0.5 right-6 z-50 flex items-center gap-3 text-text-secondary pointer-events-auto">
        <button
          type="button"
          onClick={() => setIsRightSidebarOpen((v) => !v)}
          className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors ${
            isRightSidebarOpen ? "text-text-base" : "text-text-secondary hover:bg-hover-bg hover:text-text-base"
          }`}
          title={isRightSidebarOpen ? "收起侧栏" : "打开右侧栏"}
          aria-label={isRightSidebarOpen ? "收起侧栏" : "打开右侧栏"}
        >
          <SidebarRightIcon className="text-[15px]" />
        </button>
        <button
          type="button"
          onClick={handleToggleBottomTerminalPanel}
          className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors ${
            isBottomPanelOpen ? "text-text-base" : "text-text-secondary hover:bg-hover-bg hover:text-text-base"
          }`}
          title="打开底部终端"
          aria-label="打开底部终端"
        >
          <BottomPanelIcon className="text-[15px]" />
        </button>
      </div>

      {/* Top half: main content & right sidebar */}
      <div className="relative flex flex-1 min-h-0 min-w-0 w-full overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
            <div className="flex flex-1 flex-col items-center justify-center min-w-0 px-8">
              <div className="flex-1 flex flex-col items-center justify-center max-w-3xl mx-auto w-full">
        <h1 className="text-[28px] font-medium text-text-base mb-8 flex-shrink-0 line-clamp-2">
          {projectName ? t("startView.greeting", { projectName }) : t("startView.greetingNoProject")}
        </h1>

        <Composer 
          value={value} 
          onChange={setValue} 
          onSubmit={submit} 
          placeholder={t("startView.placeholder")}
          activeProjectPath={activeProjectPath}
          textareaMaxHeight={300}
          overlayCloseSignal={composerOverlayCloseSignal}
          onOverlayOpenChange={(open) => {
            setIsComposerOverlayOpen(open);
            if (open) closeFooterMenus();
          }}
          footer={
            <div className="flex w-full items-center overflow-visible" ref={dropdownRef}>
              <div className="flex min-w-0 flex-1 items-center gap-6 overflow-visible">
              <MorphingToolbarMenu
                open={isDropdownOpen}
                onOpenChange={(next) => {
                  if (next) {
                    setIsEnvDropdownOpen(false);
                    setIsRemoteSubmenuOpen(false);
                    setIsGitMenuOpen(false);
                    closeComposerOverlays();
                  }
                  setIsDropdownOpen(next);
                }}
                layoutId={projectMorphLayoutId}
                icon={["far", "folder"]}
                label={projectName}
                panelClassName="flex w-[300px] flex-col"
                zIndex={50}
                staggerContent={false}
              >
                    <div className={cn(PROJECT_MENU.searchWrap, PROJECT_MENU.searchBar)}>
                      <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className={PROJECT_MENU.icon} />
                      <input
                        type="text"
                        placeholder={t("startView.searchProject")}
                        className={MENU_LIST.searchInput}
                      />
                    </div>

                    <div className={PROJECT_MENU.divider} aria-hidden />

                    <div className={cn(PROJECT_MENU.padX, "max-h-[200px] overflow-y-auto pb-0.5")}>
                      <SlidingMenuList
                        activeId={activeProjectPath ?? ""}
                        pillClassName={PROJECT_MENU.pill}
                        className="w-full"
                      >
                        {projects.map(p => (
                          <div
                            key={p.path}
                            {...{ [MENU_ITEM_ATTR]: p.path }}
                            className={cn(PROJECT_MENU.row, "relative z-[1] hover:bg-transparent")}
                            onClick={() => {
                              onSelectProject(p.path);
                              setIsDropdownOpen(false);
                            }}
                          >
                            <div className="flex min-w-0 items-center">
                              <FontAwesomeIcon icon={["far", "folder"]} className={PROJECT_MENU.icon} />
                              <span className="truncate">{p.name ?? "Untitled project"}</span>
                            </div>
                            {p.path === activeProjectPath && (
                              <FontAwesomeIcon icon={["fas", "check"]} className="ml-2 shrink-0 text-[11px] text-text-secondary" />
                            )}
                          </div>
                        ))}
                      </SlidingMenuList>
                    </div>

                    <div className={PROJECT_MENU.divider} aria-hidden />

                    <div className={cn(PROJECT_MENU.padX, "pb-2 pt-0.5")}>
                      <SlidingMenuList
                        activeId={activeProjectPath ? "" : "__none__"}
                        pillClassName={PROJECT_MENU.pill}
                        className="w-full"
                      >
                        <div
                          {...{ [MENU_ITEM_ATTR]: "__add__" }}
                          className={cn(PROJECT_MENU.row, "relative z-[1] hover:bg-transparent")}
                          onClick={() => { onAddProject(); setIsDropdownOpen(false); }}
                        >
                          <div className="flex min-w-0 items-center">
                            <FontAwesomeIcon icon={["fas", "plus"]} className={PROJECT_MENU.icon} />
                            <span className="truncate">{t("startView.addNewProject")}</span>
                          </div>
                          <FontAwesomeIcon icon={["fas", "chevron-right"]} className="ml-2 shrink-0 text-[10px] text-text-secondary" />
                        </div>
                        <div
                          {...{ [MENU_ITEM_ATTR]: "__none__" }}
                          className={cn(PROJECT_MENU.row, "relative z-[1] hover:bg-transparent")}
                        >
                          <div className="flex min-w-0 items-center">
                            <FontAwesomeIcon icon={["far", "folder"]} className={PROJECT_MENU.icon} />
                            <span className="truncate">{t("startView.noProject")}</span>
                          </div>
                        </div>
                      </SlidingMenuList>
                    </div>
              </MorphingToolbarMenu>

              <MorphingToolbarMenu
                open={isEnvDropdownOpen}
                onOpenChange={(next) => {
                  if (next) {
                    setIsDropdownOpen(false);
                    setIsGitMenuOpen(false);
                    closeComposerOverlays();
                  }
                  setIsEnvDropdownOpen(next);
                  if (!next) setIsRemoteSubmenuOpen(false);
                }}
                layoutId={envMorphLayoutId}
                icon={envMode === "local" ? ["fas", "desktop"] : ["fas", "cloud"]}
                label={envLabel}
                panelClassName="w-[200px] overflow-visible"
                staggerContent={false}
              >
                    <div
                      ref={envPadRef}
                      className={cn(ENV_MENU.pad, "relative")}
                      onMouseLeave={() => setIsRemoteSubmenuOpen(false)}
                    >
                    <SlidingMenuList activeId={envMode} pillClassName={ENV_MENU.pill} className="flex w-full flex-col">
                      <div 
                        {...{ [MENU_ITEM_ATTR]: "local" }}
                        className={cn(ENV_MENU.row, "relative z-[1] hover:bg-transparent")}
                        onClick={() => {
                          handleEnvModeChange("local");
                          setIsEnvDropdownOpen(false);
                          setIsRemoteSubmenuOpen(false);
                        }}
                      >
                        <div className="flex min-w-0 items-center">
                          <FontAwesomeIcon icon={["fas", "desktop"]} className={ENV_MENU.icon} />
                          <span className="truncate">{t("chatView.localMode")}</span>
                        </div>
                        {envMode === "local" && (
                          <FontAwesomeIcon icon={["fas", "check"]} className="ml-2 shrink-0 text-[11px] text-text-secondary" />
                        )}
                      </div>

                      <div
                        ref={remoteRowRef}
                        {...{ [MENU_ITEM_ATTR]: "remote" }}
                        className={cn(ENV_MENU.row, "relative z-[1] hover:bg-transparent")}
                        onMouseEnter={() => setIsRemoteSubmenuOpen(true)}
                        onClick={() => {
                          handleEnvModeChange("remote");
                          setIsRemoteSubmenuOpen(true);
                        }}
                      >
                        <div className="flex min-w-0 items-center">
                          <FontAwesomeIcon icon={["fas", "cloud"]} className={ENV_MENU.icon} />
                          <span className="truncate">{t("chatView.remoteMode")}</span>
                        </div>
                        <div className="ml-2 flex shrink-0 items-center gap-2">
                          {envMode === "remote" && (
                            <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-secondary" />
                          )}
                          <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
                        </div>
                      </div>
                    </SlidingMenuList>

                        {isRemoteSubmenuOpen && (
                          <>
                            <div
                              className="absolute left-full z-[65] w-2"
                              style={{ top: remoteFlyoutAnchor.top, height: remoteFlyoutAnchor.height }}
                              onMouseEnter={() => setIsRemoteSubmenuOpen(true)}
                              aria-hidden
                            />
                          <Panel
                            menu
                            className="absolute left-full top-0 z-[70] ml-1.5 w-[220px] min-w-[200px] max-w-[260px] origin-top-left"
                            style={{ top: remoteFlyoutAnchor.top }}
                            onMouseEnter={() => setIsRemoteSubmenuOpen(true)}
                          >
                            <div className="px-2 py-2">
                              <div className="px-0.5 pb-1.5 text-[11px] font-medium text-text-secondary">
                                {t("chatView.existingSshConnections")}
                              </div>

                              {sshConnections.length === 0 ? (
                                <div className="rounded-lg bg-black/5 px-2.5 py-2.5 text-[12px] leading-relaxed text-text-secondary">
                                  {t("chatView.noSshConnections")}
                                </div>
                              ) : (
                                <SlidingMenuList
                                  activeId={
                                    envMode === "remote" && selectedConnectionId ? selectedConnectionId : ""
                                  }
                                  pillClassName={ENV_MENU.pill}
                                  className="w-full"
                                >
                                  {sshConnections.map((conn) => {
                                    const checked =
                                      envMode === "remote" &&
                                      selectedConnectionId === conn.id;

                                    return (
                                      <div
                                        key={conn.id}
                                        {...{ [MENU_ITEM_ATTR]: conn.id }}
                                        className={cn(ENV_MENU.rowMulti, "relative z-[1] hover:bg-transparent")}
                                        onClick={() => handleRemoteConnectionSelect(conn.id)}
                                      >
                                        <div className="min-w-0">
                                          <div className="truncate text-text-base">
                                            {conn.name}
                                          </div>
                                          <div className="break-all text-[11px] text-text-secondary">
                                            {conn.username}@{conn.host}:{conn.port}
                                          </div>
                                        </div>
                                        <div className="flex shrink-0 items-center gap-2 pt-0.5">
                                          <span
                                            className={`inline-flex items-center rounded-full px-2 py-0.5 text-[10px] ${
                                              conn.status === "connected"
                                                ? "bg-green-100 text-green-700"
                                                : conn.status === "connecting"
                                                ? "bg-yellow-100 text-yellow-700"
                                                : conn.status === "error"
                                                ? "bg-red-100 text-red-700"
                                                : "bg-sidebar-bg text-text-secondary"
                                            }`}
                                          >
                                            <span
                                              className={`mr-1 h-1.5 w-1.5 rounded-full ${
                                                conn.status === "connected"
                                                  ? "bg-green-500"
                                                  : conn.status === "connecting"
                                                  ? "bg-yellow-500"
                                                  : conn.status === "error"
                                                  ? "bg-red-500"
                                                  : "bg-gray-400"
                                              }`}
                                            />
                                            {conn.status === "connected"
                                              ? t("settings.connections.online")
                                              : conn.status === "connecting"
                                              ? t("settings.connections.checking")
                                              : conn.status === "error"
                                              ? t("settings.connections.offline")
                                              : t("settings.connections.unknown")}
                                          </span>
                                          {checked && (
                                            <FontAwesomeIcon
                                              icon={["fas", "check"]}
                                              className="text-[11px] text-text-secondary"
                                            />
                                          )}
                                        </div>
                                      </div>
                                    );
                                  })}
                                </SlidingMenuList>
                              )}
                            </div>
                          </Panel>
                          </>
                        )}
                    </div>
              </MorphingToolbarMenu>

              <GitBranchChip
                projectPath={activeProjectPath}
                compactMenu
                className="shrink-0"
                open={isGitMenuOpen}
                onOpenChange={(next) => {
                  if (next) {
                    setIsDropdownOpen(false);
                    setIsEnvDropdownOpen(false);
                    setIsRemoteSubmenuOpen(false);
                    closeComposerOverlays();
                  }
                  setIsGitMenuOpen(next);
                }}
              />
              </div>

              {/* Right-aligned: live DeepSeek balance chip. */}
              <div className="ml-auto shrink-0">
                <BalanceChip
                  popoverSuppressed={
                    isDropdownOpen || isEnvDropdownOpen || isGitMenuOpen || isComposerOverlayOpen
                  }
                />
              </div>

            </div>
          }
        />
      </div>
            </div>

            <RightSidebarWorkbench
              open={isRightSidebarOpen}
              tabs={sidebarTabs}
              activeTabId={activeSidebarTabId}
              onSelectTab={setActiveSidebarTabId}
              onCloseTab={closeSidebarTab}
              onShowLauncher={() => setActiveSidebarTabId("new")}
              onSelectPlugin={handleOpenSidebarPlugin}
              renderContext={{ activeProjectPath, envMode, selectedConnectionId }}
            />
        </div>
      </div>
      </div>

        {bottomPanelPresence.shouldRender && (
            <div 
              className={`bottom-panel-workbench relative z-0 flex w-full min-w-0 flex-shrink-0 flex-col overflow-hidden border-t border-border-theme bg-bg-base ${
                bottomPanelPresence.phase === "opening" ? "is-opening" : ""
              } ${bottomPanelPresence.isClosing ? "is-closing" : ""} ${isResizingBottom ? "is-resizing" : ""}`}
              style={{
                height: bottomPanelPresence.isVisible ? `${bottomPanelHeight}px` : "0px",
                minHeight: bottomPanelPresence.isVisible ? "200px" : "0px",
                maxHeight: '80vh',
                width: '100%',
              }}
            >
              <div 
                className={`panel-resize-handle-row ${isResizingBottom ? "is-active" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setIsResizingBottom(true);
                }}
              />
              <div className="bottom-panel-workbench-inner flex min-h-0 flex-1 flex-col overflow-hidden">
              <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-bg-base">
                <div className="flex h-full min-w-0 flex-1 items-center overflow-x-auto text-[13px] text-text-secondary no-scrollbar">
                  {bottomTabs.map(tab => (
                    <div 
                      key={tab.id}
                      onClick={() => setActiveBottomTabId(tab.id)}
                      className={`flex h-full max-w-[220px] flex-shrink-0 cursor-pointer items-center border-b-2 px-3 ${
                        activeBottomTabId === tab.id 
                          ? "border-text-base text-text-base" 
                          : "border-transparent hover:text-text-base"
                      }`}
                    >
                      <FontAwesomeIcon icon={tab.icon} className="mr-2 flex-shrink-0" />
                      <span className="min-w-0 truncate">{tab.title}</span>
                      <FontAwesomeIcon 
                        icon={["fas", "xmark"]} 
                        className="ml-3 flex-shrink-0 hover:text-red-500 text-[10px]"
                        onClick={(e) => {
                           e.stopPropagation();
                           const newTabs = bottomTabs.filter(t => t.id !== tab.id);
                           setBottomTabs(newTabs);
                           if (activeBottomTabId === tab.id) {
                             setActiveBottomTabId(newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new");
                           }
                        }}
                      />
                    </div>
                  ))}
                  <div 
                    className={`flex h-full flex-shrink-0 cursor-pointer items-center px-3 ${activeBottomTabId === "new" ? "text-text-base" : "hover:text-text-base"}`}
                    onClick={() => setActiveBottomTabId("new")}
                  >
                    <FontAwesomeIcon icon={["fas", "plus"]} />
                  </div>
                </div>
                <div className="ml-3 flex flex-shrink-0 items-center space-x-3 text-text-secondary">
                  {activeBottomTab?.type === "files" && (
                    <>
                      <FontAwesomeIcon icon={["fas", "ellipsis"]} className="cursor-pointer hover:text-text-base" />
                      <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="cursor-pointer hover:text-text-base text-[13px]" />
                      <FontAwesomeIcon icon={["far", "copy"]} className="cursor-pointer hover:text-text-base" />
                    </>
                  )}
                  <FontAwesomeIcon icon={["fas", "xmark"]} className="cursor-pointer hover:text-text-base ml-2" onClick={() => setIsBottomPanelOpen(false)} />
                </div>
              </div>

              <div className="flex-1 overflow-hidden flex flex-col relative">
                {activeBottomTabId === "new" && (
                  <ToolLauncherPanel cards={PLUGIN_TOOL_CARDS} onSelect={handleOpenBottomPlugin} variant="bottom" />
                )}

                {activeBottomTabId !== "new" && activeBottomTab
                  ? renderPluginTab(activeBottomTab, {
                      activeProjectPath,
                      envMode,
                      selectedConnectionId,
                    })
                  : null}
              </div>
              </div>
            </div>
        )}
      
    </div>
  );
}
