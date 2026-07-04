import { useState, useRef, useEffect, useCallback } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { Composer } from "./Composer";
import { BalanceChip } from "./BalanceChip";
import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import type { Project } from "../types";
import { sshListConnections, type SshConnection } from "../api";
import { FilesPlugin } from "./plugins/FilesPlugin";
import { SideChatPlugin } from "./plugins/SideChatPlugin";
import { BrowserPlugin } from "./plugins/BrowserPlugin";
import { TerminalPlugin } from "./plugins/TerminalPlugin";
import { RecordingPlugin } from "./plugins/RecordingPlugin";
import { FilePreviewPlugin } from "./plugins/FilePreviewPlugin";
import { ProjectMapPanel } from "./project-map/ProjectMapPanel";
import { Tab, TOOL_CARDS } from "./ChatView";
import { ToolLauncherPanel } from "./ToolLauncherPanel";
import { GitBranchChip } from "./git/GitBranchChip";
import { SidebarPluginHeader } from "./SidebarPluginHeader";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";
const PROJECT_MAP_TAB_ID = "project-map";

function getProjectDisplayName(path?: string | null): string | null {
  const value = path?.trim();
  if (!value) return null;
  const parts = value.replace(/[\\/]+$/, "").split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? value;
}

interface Props {
  projectName: string;
  activeProjectPath?: string | null;
  projectMapOpenSignal?: number;
  projects: Project[];
  onSelectProject: (path: string) => void;
  onAddProject: () => void;
  onSubmit: (text: string, envMode: "local" | "remote", connectionId: string | null) => void;
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

  const [isModeDropdownOpen, setIsModeDropdownOpen] = useState(false);
  const modeDropdownRef = useRef<HTMLDivElement>(null);
  const [workMode, setWorkMode] = useState<"code" | "daily">(() => (localStorage.getItem("workMode") as any) || "code");

  const [isBottomPanelOpen, setIsBottomPanelOpen] = useState(false);
  const [bottomTabs, setBottomTabs] = useState<Tab[]>([]);
  const [activeBottomTabId, setActiveBottomTabId] = useState<string>("new");

  const [isRightSidebarOpen, setIsRightSidebarOpen] = useState(false);
  const [rightSidebarWidth, setRightSidebarWidth] = useState(600);
  const [isResizingSidebar, setIsResizingSidebar] = useState(false);
  const [isRightSidebarMaximized, setIsRightSidebarMaximized] = useState(false);
  const [bottomPanelHeight, setBottomPanelHeight] = useState(280);
  const [isResizingBottom, setIsResizingBottom] = useState(false);
  const sidebarRestoreWidthRef = useRef(600);

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

  useEffect(() => {
    if (!isResizingSidebar) return;
    const handleMouseMove = (e: MouseEvent) => {
      const newWidth = window.innerWidth - e.clientX;
      const maxSidebarWidth = Math.max(360, window.innerWidth - 500);
      if (newWidth >= 360 && newWidth <= maxSidebarWidth) {
        setRightSidebarWidth(newWidth);
      } else if (newWidth < 360) {
        setRightSidebarWidth(360);
      } else if (newWidth > maxSidebarWidth) {
        setRightSidebarWidth(maxSidebarWidth);
      }
    };
    const handleMouseUp = () => setIsResizingSidebar(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizingSidebar]);
  const [sidebarTabs, setSidebarTabs] = useState<Tab[]>([]);
  const [activeSidebarTabId, setActiveSidebarTabId] = useState<string>("new");
  const activeSidebarTab = sidebarTabs.find((tab) => tab.id === activeSidebarTabId) ?? null;

  const resolveToolName = (type: string) =>
    t(`chatView.tools.${type}`, {
      defaultValue: type === "project_map" ? "项目地图" : type,
    });

  const getTerminalTabTitle = () => {
    if (envMode === "remote") {
      const current = sshConnections.find((connection) => connection.id === selectedConnectionId);
      if (current?.name) return current.name;
      if (current) return `${current.username}@${current.host}`;
      return "SSH Terminal";
    }
    const path = activeProjectPath?.trim();
    return path && path.length > 0 ? path : "Terminal";
  };

  const getFilesTabTitle = () => {
    return getProjectDisplayName(activeProjectPath) ?? resolveToolName("files");
  };

  const handleOpenBottomPlugin = (c: typeof TOOL_CARDS[0]) => {
    const newTab: Tab = {
      id: Date.now().toString(),
      type: c.type,
      title: c.title === "terminal" ? getTerminalTabTitle() : 
             c.title === "files" ? getFilesTabTitle() : resolveToolName(c.type),
      icon: c.title === "terminal" ? ["fas", "terminal"] :
            c.title === "files" ? ["far", "file-lines"] : c.icon
    };
    setBottomTabs([...bottomTabs, newTab]);
    setActiveBottomTabId(newTab.id);
  };

  const handleOpenSidebarPlugin = (c: typeof TOOL_CARDS[0]) => {
    const newTab: Tab = {
      id: Date.now().toString(),
      type: c.type,
      title: c.title === "terminal" ? getTerminalTabTitle() : 
             c.title === "files" ? getFilesTabTitle() : resolveToolName(c.type),
      icon: c.title === "terminal" ? ["fas", "terminal"] :
            c.title === "files" ? ["far", "file-lines"] : c.icon
    };
    setSidebarTabs([...sidebarTabs, newTab]);
    setActiveSidebarTabId(newTab.id);
  };

  const closeSidebarTab = (tabId: string) => {
    const newTabs = sidebarTabs.filter((tab) => tab.id !== tabId);
    setSidebarTabs(newTabs);
    if (activeSidebarTabId === tabId) {
      setActiveSidebarTabId(newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new");
    }
    if (newTabs.length === 0) {
      setIsRightSidebarOpen(false);
      if (isRightSidebarMaximized) {
        setIsRightSidebarMaximized(false);
        setRightSidebarWidth(sidebarRestoreWidthRef.current);
      }
    }
  };

  const handleToggleBottomTerminalPanel = () => {
    if (envMode === "local") {
      setIsBottomPanelOpen(true);
      if (!bottomTabs.some((t) => t.type === "terminal")) {
        const terminalCard = TOOL_CARDS.find((c) => c.type === "terminal");
        if (terminalCard) {
          void handleOpenBottomPlugin(terminalCard);
        }
      } else {
        const termTab = bottomTabs.find((t) => t.type === "terminal");
        if (termTab) setActiveBottomTabId(termTab.id);
      }
      return;
    }

    if (isBottomPanelOpen) {
      setIsBottomPanelOpen(false);
    } else {
      setIsBottomPanelOpen(true);
      if (!bottomTabs.some((t) => t.type === "terminal")) {
        const terminalCard = TOOL_CARDS.find((c) => c.type === "terminal");
        if (terminalCard) {
          void handleOpenBottomPlugin(terminalCard);
        }
      } else {
        const termTab = bottomTabs.find((t) => t.type === "terminal");
        if (termTab) setActiveBottomTabId(termTab.id);
      }
    }
  };

  const toggleSidebarMaximize = () => {
    if (isRightSidebarMaximized) {
      setRightSidebarWidth(sidebarRestoreWidthRef.current);
      setIsRightSidebarMaximized(false);
      return;
    }
    sidebarRestoreWidthRef.current = rightSidebarWidth;
    setIsRightSidebarMaximized(true);
  };

  const openProjectMapSidebar = useCallback(() => {
    const existingTab = sidebarTabs.find((tab) => tab.type === "project_map");
    setIsRightSidebarOpen(true);
    setActiveSidebarTabId(existingTab?.id ?? PROJECT_MAP_TAB_ID);
    setSidebarTabs((tabs) => {
      if (tabs.some((tab) => tab.type === "project_map")) return tabs;
      return [
        ...tabs,
        {
          id: PROJECT_MAP_TAB_ID,
          type: "project_map",
          title: "项目地图",
          icon: ["fas", "share-nodes"],
        },
      ];
    });
  }, [sidebarTabs]);

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
      if (modeDropdownRef.current && !modeDropdownRef.current.contains(e.target as Node)) {
        setIsModeDropdownOpen(false);
      }
    };
    if (isDropdownOpen || isModeDropdownOpen || isEnvDropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isDropdownOpen, isModeDropdownOpen, isEnvDropdownOpen]);

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
  }, [loadSshConnections]);

  const selectedConnection =
    sshConnections.find((conn) => conn.id === selectedConnectionId) ?? null;
  const envLabel =
    envMode === "local"
      ? "\u672c\u5730\u6a21\u5f0f"
      : selectedConnection
      ? `\u8fdc\u7a0b\u6a21\u5f0f \u00b7 ${selectedConnection.name}`
      : "\u8fdc\u7a0b\u6a21\u5f0f";

  const submit = () => {
    onSubmit(value.trim(), envMode, selectedConnectionId);
    setValue("");
  };

  return (
    <div className="w-full h-full flex flex-col relative">
      {/* Top-right widgets */}
      <div className={`absolute top-4 z-50 flex items-center gap-3 text-text-secondary ${isRightSidebarOpen && sidebarTabs.length > 0 ? "right-0" : "right-4"}`}>
        <div className="relative" ref={modeDropdownRef}>
          <div
            className="flex items-center px-2 py-1 hover:bg-gray-100 rounded cursor-pointer text-sm transition-colors"
            onClick={() => setIsModeDropdownOpen(!isModeDropdownOpen)}
          >
            <FontAwesomeIcon icon={workMode === "code" ? ["fas", "code"] : ["far", "comments"]} className={workMode === "code" ? "text-blue-500" : "text-text-base"} />
          </div>

          <AnimatePresence>
            {isModeDropdownOpen && (
              <motion.div
                initial={{ opacity: 0, y: -10, scale: 0.95 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: -10, scale: 0.95 }}
                transition={{ duration: 0.15, ease: "easeOut" }}
                className="absolute top-full right-0 mt-1 w-[260px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-[100] py-2 origin-top-right"
              >
                <div
                  className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-start justify-between"
                  onClick={() => {
                    setWorkMode("code");
                    localStorage.setItem("workMode", "code");
                    setIsModeDropdownOpen(false);
                  }}
                >
                  <div className="flex items-start">
                    <div className="w-6 h-6 flex items-center justify-center mr-3 mt-0.5">
                      <FontAwesomeIcon icon={["fas", "code"]} className="text-blue-500 text-[13px]" />
                    </div>
                    <div className="flex-1">
                      <div className="text-[13px] font-medium text-text-base">{t("settings.general.workMode.code")}</div>
                      <div className="text-[11px] text-text-secondary">{t("settings.general.workMode.codeDesc")}</div>
                    </div>
                  </div>
                  <div className="w-4 flex justify-end mt-1">
                    {workMode === "code" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />}
                  </div>
                </div>

                <div
                  className="px-4 py-2 hover:bg-gray-50 cursor-pointer flex items-start justify-between"
                  onClick={() => {
                    setWorkMode("daily");
                    localStorage.setItem("workMode", "daily");
                    setIsModeDropdownOpen(false);
                  }}
                >
                  <div className="flex items-start">
                    <div className="w-6 h-6 flex items-center justify-center mr-3 mt-0.5">
                      <FontAwesomeIcon icon={["far", "comments"]} className="text-text-base text-[13px]" />
                    </div>
                    <div className="flex-1">
                      <div className="text-[13px] font-medium text-text-base">{t("settings.general.workMode.daily")}</div>
                      <div className="text-[11px] text-text-secondary">{t("settings.general.workMode.dailyDesc")}</div>
                    </div>
                  </div>
                  <div className="w-4 flex justify-end mt-1">
                    {workMode === "daily" && <FontAwesomeIcon icon={["fas", "check"]} className="text-[12px] text-text-base" />}
                  </div>
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        {isRightSidebarOpen && sidebarTabs.length > 0 ? (
          <div
            className={`overflow-hidden border border-border-theme border-b-0 bg-white ${isResizingSidebar ? "" : "transition-[width] duration-300"} ${isRightSidebarMaximized ? "flex-1 min-w-0" : ""}`}
            style={isRightSidebarMaximized ? { minWidth: "360px" } : { width: `${rightSidebarWidth}px`, minWidth: "360px" }}
          >
            <SidebarPluginHeader
              tabs={sidebarTabs}
              activeTabId={activeSidebarTabId}
              onSelectTab={setActiveSidebarTabId}
              onCloseTab={closeSidebarTab}
              onShowLauncher={() => setActiveSidebarTabId("new")}
              extraActions={
                <>
                  {activeSidebarTab?.type === "files" ? (
                    <button
                      type="button"
                      onClick={toggleSidebarMaximize}
                      className="flex h-9 w-9 items-center justify-center rounded-xl text-text-secondary transition-colors hover:bg-[#f3f4f6] hover:text-text-base"
                      title={isRightSidebarMaximized ? "Exit full screen file view" : "Full screen file view"}
                    >
                      <FontAwesomeIcon
                        icon={["fas", isRightSidebarMaximized ? "compress" : "expand"]}
                        className="text-[12px]"
                      />
                    </button>
                  ) : null}
                  <button
                    type="button"
                    onClick={handleToggleBottomTerminalPanel}
                    className="flex h-9 w-9 items-center justify-center rounded-xl text-text-secondary transition-colors hover:bg-[#f3f4f6] hover:text-text-base"
                    title="Open bottom panel"
                  >
                    <BottomPanelIcon className="text-[15px]" />
                  </button>
                  <button
                    type="button"
                    onClick={() => setIsRightSidebarOpen(false)}
                    className="flex h-9 w-9 items-center justify-center rounded-xl text-text-secondary transition-colors hover:bg-[#f3f4f6] hover:text-text-base"
                    title="Collapse sidebar"
                  >
                    <SidebarRightIcon className="text-[15px]" />
                  </button>
                </>
              }
              className="border-0 bg-transparent"
            />
          </div>
        ) : (
          <div className="flex items-center space-x-3">
            <BottomPanelIcon
              className="cursor-pointer transition-colors hover:text-text-base"
              onClick={handleToggleBottomTerminalPanel}
            />
            <SidebarRightIcon
              className="cursor-pointer transition-colors hover:text-text-base"
              onClick={() => setIsRightSidebarOpen(true)}
            />
          </div>
        )}
      </div>

      
      <div className="flex flex-1 min-h-0 w-full overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
            <div className="flex flex-1 flex-col items-center justify-center min-w-0 px-8">
              <div className="flex-1 flex flex-col items-center justify-center max-w-3xl mx-auto w-full">
        <h1 className="text-[28px] font-medium text-text-base mb-8">
          {projectName ? t("startView.greeting", { projectName }) : t("startView.greetingNoProject")}
        </h1>

        <Composer 
          value={value} 
          onChange={setValue} 
          onSubmit={submit} 
          placeholder={t("startView.placeholder")}
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
                
                <AnimatePresence>
                  {isEnvDropdownOpen && (
                    <motion.div
                      initial={{ opacity: 0, y: 10, scale: 0.95 }}
                      animate={{ opacity: 1, y: 0, scale: 1 }}
                      exit={{ opacity: 0, y: 10, scale: 0.95 }}
                      transition={{ duration: 0.15, ease: "easeOut" }}
                      className="absolute bottom-full left-0 mb-2 w-[180px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-[60] py-1 origin-bottom-left"
                    >
                      <div 
                        className="px-3 py-2 hover:bg-gray-100 cursor-pointer flex items-center justify-between group"
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
                          className="px-3 py-2 hover:bg-gray-100 cursor-pointer flex items-center justify-between group"
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

                        <AnimatePresence>
                          {isRemoteSubmenuOpen && (
                            <motion.div
                              initial={{ opacity: 0, x: 8, scale: 0.98 }}
                              animate={{ opacity: 1, x: 0, scale: 1 }}
                              exit={{ opacity: 0, x: 8, scale: 0.98 }}
                              transition={{ duration: 0.15, ease: "easeOut" }}
                              className="absolute left-full top-0 ml-2 w-[280px] overflow-hidden rounded-xl border border-border-theme bg-white py-1 shadow-[0_8px_30px_rgb(0,0,0,0.12)]"
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
                                      className="px-3 py-2 hover:bg-gray-100 cursor-pointer flex items-start justify-between gap-3"
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
                                              : "bg-gray-100 text-text-secondary"
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
                            </motion.div>
                          )}
                        </AnimatePresence>
                      </div>
                    </motion.div>
                  )}
                </AnimatePresence>
              </div>

              <GitBranchChip projectPath={activeProjectPath} />
              </div>

              {/* Right-aligned: live DeepSeek balance chip. */}
              <div className="ml-auto">
                <BalanceChip />
              </div>

              {/* Dropdown Menu */}
              <AnimatePresence>
                {isDropdownOpen && (
                  <motion.div
                    initial={{ opacity: 0, y: 10, scale: 0.95 }}
                    animate={{ opacity: 1, y: 0, scale: 1 }}
                    exit={{ opacity: 0, y: 10, scale: 0.95 }}
                    transition={{ duration: 0.15, ease: "easeOut" }}
                    className="absolute bottom-full left-0 mb-2 w-[300px] bg-white border border-border-theme rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] flex flex-col z-50 overflow-hidden py-1 origin-bottom-left"
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
                          className="flex items-center justify-between px-4 py-2 hover:bg-gray-100 cursor-pointer text-[13px] text-text-base group"
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

                    <div className="flex items-center justify-between px-4 py-2 hover:bg-gray-100 cursor-pointer text-[13px] text-text-base" onClick={() => { onAddProject(); setIsDropdownOpen(false); }}>
                      <div className="flex items-center">
                        <FontAwesomeIcon icon={["fas", "plus"]} className="mr-2 text-text-secondary w-4" />
                        {t("startView.addNewProject")}
                      </div>
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-text-secondary" />
                    </div>
                    <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer text-[13px] text-text-base">
                      <FontAwesomeIcon icon={["far", "folder"]} className="mr-2 text-text-secondary w-4" />
                      {t("startView.noProject")}
                    </div>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
          }
        />

        {/* Suggestions removed as they are no longer in Props */}
        <div className="w-full mt-6 space-y-1">
          <div className="flex items-center px-3 py-2.5 text-[13px] text-text-secondary hover:bg-gray-50 rounded-lg cursor-pointer transition-colors group">
            <FontAwesomeIcon
              icon={["fas", "border-all"]}
              className="w-6 text-gray-400 group-hover:text-text-base transition-colors"
            />
            {t("startView.connectApps")}
          </div>
        </div>
      </div>
            </div>
            
            {isRightSidebarOpen && (
              <div 
                className={`relative flex h-full flex-shrink-0 flex-col border-l border-border-theme bg-white shadow-[-12px_0_30px_rgba(0,0,0,0.06)] ${isResizingSidebar ? "" : "transition-[width] duration-300"} ${isRightSidebarMaximized ? "w-full" : ""}`}
                style={isRightSidebarMaximized ? { minWidth: '360px' } : { width: `${rightSidebarWidth}px`, minWidth: '360px' }}
              >
          {/* Resize Handle */}
          <div 
            className={`panel-resize-handle-col ${isResizingSidebar ? "is-active" : ""}`}
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingSidebar(true);
            }}
          />
          <div className="flex-1 overflow-hidden flex flex-col relative">
            {activeSidebarTabId === "new" && (
              <ToolLauncherPanel cards={TOOL_CARDS} onSelect={handleOpenSidebarPlugin} variant="sidebar" />
            )}

            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "files" && <FilesPlugin projectPath={activeProjectPath} />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "chat" && <SideChatPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "browser" && <BrowserPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "terminal" && (
              <TerminalPlugin mode={envMode} connectionId={selectedConnectionId} />
            )}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "recording" && <RecordingPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "file_preview" && <FilePreviewPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "project_map" && <ProjectMapPanel projectPath={activeProjectPath} />}
          </div>
              </div>
            )}
          </div>

          {isBottomPanelOpen && (
            <div 
              className={`relative flex flex-shrink-0 flex-col border-t border-border-theme bg-white shadow-[0_-12px_30px_rgba(0,0,0,0.06)] ${isResizingBottom ? "" : "transition-[height] duration-300"}`}
              style={{ height: `${bottomPanelHeight}px`, minHeight: '200px', maxHeight: '80vh' }}
            >
              <div 
                className={`panel-resize-handle-row ${isResizingBottom ? "is-active" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setIsResizingBottom(true);
                }}
              />
              <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-white">
                <div className="flex items-center text-[13px] text-text-secondary h-full">
                  {bottomTabs.map(tab => (
                    <div 
                      key={tab.id}
                      onClick={() => setActiveBottomTabId(tab.id)}
                      className={`flex items-center h-full px-3 border-b-2 cursor-pointer ${
                        activeBottomTabId === tab.id 
                          ? "border-text-base text-text-base" 
                          : "border-transparent hover:text-text-base"
                      }`}
                    >
                      <FontAwesomeIcon icon={tab.icon} className="mr-2" />
                      {tab.title}
                      <FontAwesomeIcon 
                        icon={["fas", "xmark"]} 
                        className="ml-3 hover:text-red-500 text-[10px]" 
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
                    className={`flex items-center h-full px-3 cursor-pointer ${activeBottomTabId === "new" ? "text-text-base" : "hover:text-text-base"}`}
                    onClick={() => setActiveBottomTabId("new")}
                  >
                    <FontAwesomeIcon icon={["fas", "plus"]} />
                  </div>
                </div>
                <div className="flex items-center space-x-3 text-text-secondary">
                  {bottomTabs.find(t => t.id === activeBottomTabId)?.type === "files" && (
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
                  <ToolLauncherPanel cards={TOOL_CARDS} onSelect={handleOpenBottomPlugin} variant="bottom" />
                )}

                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "files" && <FilesPlugin projectPath={activeProjectPath} />}
                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "chat" && <SideChatPlugin />}
                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "browser" && <BrowserPlugin />}
                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "terminal" && (
                  <TerminalPlugin mode={envMode} connectionId={selectedConnectionId} />
                )}
                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "recording" && <RecordingPlugin />}
                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "file_preview" && <FilePreviewPlugin />}
                {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "project_map" && <ProjectMapPanel projectPath={activeProjectPath} />}
              </div>
            </div>
          )}
        </div>
      </div>
      
    </div>
  );
}
