import { useState, useRef, useEffect, useCallback } from "react";
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

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";
const PROJECT_MAP_TAB_ID = "project-map";

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
  const [isEnvDropdownOpen, setIsEnvDropdownOpen] = useState(false);
  const envDropdownRef = useRef<HTMLDivElement>(null);
  const [envMode, setEnvMode] = useState<"local" | "remote">(() => (localStorage.getItem("envMode") as any) || "local");
  const [selectedConnectionId, setSelectedConnectionId] = useState<string | null>(
    () => localStorage.getItem("ssh_connection_id")
  );
  const [sshConnections, setSshConnections] = useState<SshConnection[]>([]);
  const [isRemoteSubmenuOpen, setIsRemoteSubmenuOpen] = useState(false);

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
  const bottomPanelPresence = usePanelPresence(isBottomPanelOpen, 260);

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
        setIsDropdownOpen(false);
      }
      if (envDropdownRef.current && !envDropdownRef.current.contains(e.target as Node)) {
        setIsEnvDropdownOpen(false);
      }
    };
    if (isDropdownOpen || isEnvDropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isDropdownOpen, isEnvDropdownOpen]);

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
      ? "\u672c\u5730\u6a21\u5f0f"
      : selectedConnection
      ? `\u8fdc\u7a0b\u6a21\u5f0f \u00b7 ${selectedConnection.name}`
      : "\u8fdc\u7a0b\u6a21\u5f0f";

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
          footer={
            <div className="flex items-center w-full relative" ref={dropdownRef}>
              <div className="flex items-center space-x-4">
              <div 
                className="inline-flex items-center text-[12px] font-medium text-text-secondary hover:text-text-base cursor-pointer transition-colors"
                onClick={() => {
                  setIsDropdownOpen(!isDropdownOpen);
                  setIsEnvDropdownOpen(false);
                }}
              >
                <FontAwesomeIcon icon={["far", "folder"]} className="mr-2 text-[13px]" />
                {projectName}
                <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
              </div>

              <div className="relative" ref={envDropdownRef}>
                <div 
                  className="inline-flex items-center text-[12px] font-medium text-text-secondary hover:text-text-base cursor-pointer transition-colors"
                  onClick={() => {
                    setIsEnvDropdownOpen(!isEnvDropdownOpen);
                    setIsDropdownOpen(false);
                    setIsRemoteSubmenuOpen(false);
                  }}
                >
                  <FontAwesomeIcon icon={envMode === "local" ? ["fas", "desktop"] : ["fas", "cloud"]} className="mr-2 w-4 text-[13px]" />
                  {envLabel}
                  <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
                </div>
                
                {isEnvDropdownOpen && (
                    <div
                      className="popover-menu absolute bottom-full left-0 mb-2 w-[180px] bg-elevated-bg border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-[60] py-1 origin-bottom-left"
                    >
                      <div 
                        className="px-3 py-2 hover:bg-hover-bg cursor-pointer flex items-center justify-between group"
                        onClick={() => {
                          handleEnvModeChange("local");
                          setIsEnvDropdownOpen(false);
                          setIsRemoteSubmenuOpen(false);
                        }}
                      >
                        <div className="flex items-center text-[13px] text-text-base">
                          <FontAwesomeIcon icon={["fas", "desktop"]} className="w-4 mr-2 text-text-secondary group-hover:text-text-base" />
                          {"\u672c\u5730\u6a21\u5f0f"}
                        </div>
                        {envMode === "local" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-secondary" />}
                      </div>

                      <div
                        className="relative"
                        onMouseEnter={() => setIsRemoteSubmenuOpen(true)}
                        onMouseLeave={() => setIsRemoteSubmenuOpen(false)}
                      >
                        <div 
                          className="px-3 py-2 hover:bg-hover-bg cursor-pointer flex items-center justify-between group"
                          onClick={() => {
                            handleEnvModeChange("remote");
                            setIsRemoteSubmenuOpen((prev) => !prev);
                          }}
                        >
                          <div className="flex items-center text-[13px] text-text-base">
                            <FontAwesomeIcon icon={["fas", "cloud"]} className="w-4 mr-2 text-text-secondary group-hover:text-text-base" />
                            {"\u8fdc\u7a0b\u6a21\u5f0f"}
                          </div>
                          <div className="flex items-center gap-2">
                            {envMode === "remote" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[11px] text-text-secondary" />}
                            <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
                          </div>
                        </div>

                        {isRemoteSubmenuOpen && (
                            <div
                              className="popover-menu absolute left-full top-0 ml-2 w-[280px] overflow-hidden rounded-xl border border-border-theme bg-elevated-bg py-1 shadow-[0_8px_30px_rgb(0,0,0,0.12)]"
                            >
                              <div className="px-3 py-2 text-[11px] font-medium text-text-secondary">
                                {"\u5df2\u6709 SSH \u8fde\u63a5"}
                              </div>

                              {sshConnections.length === 0 ? (
                                <div className="px-3 py-2 text-[12px] text-text-secondary">
                                  {"\u6682\u65e0 SSH \u8fde\u63a5"}
                                </div>
                              ) : (
                                sshConnections.map((conn) => {
                                  const checked =
                                    envMode === "remote" &&
                                    selectedConnectionId === conn.id;

                                  return (
                                    <div
                                      key={conn.id}
                                      className="px-3 py-2 hover:bg-hover-bg cursor-pointer flex items-start justify-between gap-3"
                                      onClick={() => handleRemoteConnectionSelect(conn.id)}
                                    >
                                      <div className="min-w-0">
                                        <div className="text-[13px] text-text-base">
                                          {conn.name}
                                        </div>
                                        <div className="text-[11px] text-text-secondary break-all">
                                          {conn.username}@{conn.host}:{conn.port}
                                        </div>
                                      </div>
                                      <div className="flex items-center gap-2 pt-0.5">
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
                                })
                              )}
                            </div>
                          )}
                      </div>
                    </div>
                  )}
              </div>

              <GitBranchChip projectPath={activeProjectPath} compactMenu />
              </div>

              {/* Right-aligned: live DeepSeek balance chip. */}
              <div className="ml-auto">
                <BalanceChip />
              </div>

              {/* Dropdown Menu */}
              {isDropdownOpen && (
                  <div
                    className="popover-menu absolute bottom-full left-0 mb-2 w-[300px] bg-elevated-bg border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-50 overflow-hidden py-1 origin-bottom-left"
                  >
                    <div className="px-3 py-2 border-b border-transparent text-[13px] flex items-center text-text-secondary">
                      <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="mr-2" />
                      <input 
                        type="text"
                        placeholder={t("startView.searchProject")}
                        className="bg-transparent outline-none w-full"
                      />
                    </div>
                    
                    <div className="flex-1 max-h-[200px] overflow-y-auto py-1">
                      {projects.map(p => (
                        <div
                          key={p.path}
                          className="flex items-center justify-between px-4 py-2 hover:bg-hover-bg cursor-pointer text-[13px] text-text-base group"
                          onClick={() => {
                            onSelectProject(p.path);
                            setIsDropdownOpen(false);
                          }}
                        >
                          <div className="flex items-center">
                            <FontAwesomeIcon icon={["far", "folder"]} className="mr-2 text-text-secondary w-4" />
                            <span className="truncate">{p.name ?? "Untitled project"}</span>
                          </div>
                          {p.path === activeProjectPath && (
                            <FontAwesomeIcon icon={["fas", "check"]} className="text-text-secondary text-[11px]" />
                          )}
                        </div>
                      ))}
                    </div>

                    <div className="w-full h-px bg-border-theme my-1"></div>

                    <div className="flex items-center justify-between px-4 py-2 hover:bg-hover-bg cursor-pointer text-[13px] text-text-base" onClick={() => { onAddProject(); setIsDropdownOpen(false); }}>
                      <div className="flex items-center">
                        <FontAwesomeIcon icon={["fas", "plus"]} className="mr-2 text-text-secondary w-4" />
                        {t("startView.addNewProject")}
                      </div>
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
                    </div>
                    <div className="flex items-center px-4 py-2 hover:bg-hover-bg cursor-pointer text-[13px] text-text-base">
                      <FontAwesomeIcon icon={["far", "folder"]} className="mr-2 text-text-secondary w-4" />
                      {t("startView.noProject")}
                    </div>
                  </div>
                )}
            </div>
          }
        />

        {/* Suggestions removed as they are no longer in Props */}
        <div className="w-full mt-6 space-y-1">
          <div className="flex items-center px-3 py-2.5 text-[13px] text-text-secondary hover:bg-hover-bg rounded-lg cursor-pointer transition-colors group">
            <FontAwesomeIcon
              icon={["fas", "border-all"]}
              className="w-6 text-gray-400 group-hover:text-text-base transition-colors"
            />
            {t("startView.connectApps")}
          </div>
        </div>
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
                bottomPanelPresence.isClosing ? "is-closing" : ""
              } ${isResizingBottom ? "is-resizing" : ""}`}
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
        )}
      
    </div>
  );
}
