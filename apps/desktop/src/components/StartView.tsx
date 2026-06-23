import { useState, useRef, useEffect, useCallback } from "react";
import { AnimatePresence, motion } from "framer-motion";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { Composer } from "./Composer";
import { BalanceChip } from "./BalanceChip";
import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import type { Project } from "../types";
import { FilesPlugin } from "./plugins/FilesPlugin";
import { SideChatPlugin } from "./plugins/SideChatPlugin";
import { BrowserPlugin } from "./plugins/BrowserPlugin";
import { TerminalPlugin } from "./plugins/TerminalPlugin";
import { RecordingPlugin } from "./plugins/RecordingPlugin";
import { FilePreviewPlugin } from "./plugins/FilePreviewPlugin";
import { ProjectMapPanel } from "./project-map/ProjectMapPanel";
import { Tab, TOOL_CARDS } from "./ChatView";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";
const PROJECT_MAP_TAB_ID = "project-map";

interface Props {
  projectName: string;
  activeProjectPath?: string | null;
  projectMapOpenSignal?: number;
  projects: Project[];
  onSelectProject: (path: string) => void;
  onAddProject: () => void;
  onSubmit: (text: string) => void;
}

export function StartView({ projectName, activeProjectPath = null, projectMapOpenSignal = 0, projects, onSelectProject, onAddProject, onSubmit }: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [isDropdownOpen, setIsDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const [isModeDropdownOpen, setIsModeDropdownOpen] = useState(false);
  const modeDropdownRef = useRef<HTMLDivElement>(null);
  const [workMode, setWorkMode] = useState<"code" | "daily">(() => (localStorage.getItem("workMode") as any) || "code");

  const [isBottomPanelOpen, setIsBottomPanelOpen] = useState(false);
  const [bottomTabs, setBottomTabs] = useState<Tab[]>([]);
  const [activeBottomTabId, setActiveBottomTabId] = useState<string>("new");

  const [isRightSidebarOpen, setIsRightSidebarOpen] = useState(false);
  const [rightSidebarWidth, setRightSidebarWidth] = useState(600);
  const [isResizingSidebar, setIsResizingSidebar] = useState(false);
  const [bottomPanelHeight, setBottomPanelHeight] = useState(280);
  const [isResizingBottom, setIsResizingBottom] = useState(false);

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
      if (newWidth > 360 && newWidth < window.innerWidth - 100) {
        setRightSidebarWidth(newWidth);
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

  const getTranslatedToolName = (_title: string, type: string) => {
    if (type === "project_map") return "项目地图";
    return t(`chatView.tools.${type}`);
  };

  const getTranslatedToolDesc = (type: string) => {
    if (type === "project_map") return "查看模块关系";
    return t(`chatView.tools.${type}Desc`);
  };

  const handleOpenBottomPlugin = (c: typeof TOOL_CARDS[0]) => {
    const newTab: Tab = {
      id: Date.now().toString(),
      type: c.type,
      title: c.title === "terminal" ? "C:\\WINDOWS\\System32\\..." : 
             c.title === "files" ? "AUTH_SPEC.md" : getTranslatedToolName(c.title, c.type),
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
      title: c.title === "terminal" ? "C:\\WINDOWS\\System32\\..." : 
             c.title === "files" ? "AUTH_SPEC.md" : getTranslatedToolName(c.title, c.type),
      icon: c.title === "terminal" ? ["fas", "terminal"] :
            c.title === "files" ? ["far", "file-lines"] : c.icon
    };
    setSidebarTabs([...sidebarTabs, newTab]);
    setActiveSidebarTabId(newTab.id);
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
      if (modeDropdownRef.current && !modeDropdownRef.current.contains(e.target as Node)) {
        setIsModeDropdownOpen(false);
      }
    };
    if (isDropdownOpen || isModeDropdownOpen) {
      document.addEventListener("mousedown", handleClickOutside);
    }
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isDropdownOpen, isModeDropdownOpen]);

  const submit = () => {
    onSubmit(value.trim());
    setValue("");
  };

  return (
    <div className="w-full h-full flex flex-col relative">
      {/* Top-right widgets */}
      <div className="absolute top-4 right-4 flex items-center space-x-3 text-text-secondary z-50">
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
        <BottomPanelIcon 
          className="cursor-pointer transition-colors hover:text-text-base"
          onClick={() => {
            if (isBottomPanelOpen) {
              setIsBottomPanelOpen(false);
            } else {
              setIsBottomPanelOpen(true);
              if (!bottomTabs.some(t => t.type === "terminal")) {
                const terminalCard = TOOL_CARDS.find(c => c.type === "terminal");
                if (terminalCard) handleOpenBottomPlugin(terminalCard);
              } else {
                const termTab = bottomTabs.find(t => t.type === "terminal");
                if (termTab) setActiveBottomTabId(termTab.id);
              }
            }
          }}
        />
        <SidebarRightIcon
          className={`cursor-pointer transition-colors ${isRightSidebarOpen ? "text-text-base" : "hover:text-text-base"}`}
          onClick={() => setIsRightSidebarOpen(!isRightSidebarOpen)}
        />
      </div>

      
      <div className="flex-1 flex overflow-hidden relative w-full">
        <div className="flex-1 flex flex-col relative min-w-0 w-full">
          <div className="flex-1 flex flex-col items-center justify-center max-w-3xl mx-auto w-full px-8">
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
                onClick={() => setIsDropdownOpen(!isDropdownOpen)}
              >
                <FontAwesomeIcon icon={["far", "folder"]} className="mr-2 text-[13px]" />
                {projectName}
                <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
              </div>

              <div className="inline-flex items-center text-[12px] font-medium text-text-secondary hover:text-text-base cursor-pointer transition-colors">
                <FontAwesomeIcon icon={["fas", "desktop"]} className="mr-2 text-[13px]" />
                本地模式
                <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
              </div>

              <div className="inline-flex items-center text-[12px] font-medium text-text-secondary hover:text-text-base cursor-pointer transition-colors">
                <FontAwesomeIcon icon={["fas", "code-branch"]} className="mr-2 text-[13px]" />
                main
                <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
              </div>
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

      

      
          
      {/* Bottom Panel */}
      {isBottomPanelOpen && (
        <div 
          className={`absolute left-0 right-0 bottom-0 bg-white flex flex-col z-50 shadow-[0_0_40px_rgba(0,0,0,0.1)] border-t border-border-theme transform translate-y-0 ${isResizingBottom ? "" : "transition-transform duration-300"}`}
          style={{ height: `${bottomPanelHeight}px`, minHeight: '200px', maxHeight: '80vh' }}
        >
          {/* Resize Handle */}
          <div 
            className="absolute left-0 right-0 top-0 h-1.5 cursor-row-resize hover:bg-blue-500/50 z-50 -mt-[1px]"
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingBottom(true);
            }}
          />
          {/* Global Tab Bar */}
          {/* Resize Handle */}
          <div 
            className="absolute left-0 top-0 bottom-0 w-1.5 cursor-col-resize hover:bg-blue-500/50 z-50 -ml-[1px]"
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingSidebar(true);
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
              <div className="w-full h-full flex flex-col relative">
                <div className="flex-1 flex items-center justify-center pt-8 pb-4">
                  <div className="flex space-x-4">
                    {TOOL_CARDS.map((c) => (
                      <div
                        key={c.title}
                        onClick={() => handleOpenBottomPlugin(c)}
                        className="group flex-shrink-0 w-[200px] bg-bg-base rounded-2xl p-4 flex flex-col items-start cursor-pointer hover:shadow-lg hover:-translate-y-1 border border-border-theme hover:border-primary/50 transition-all duration-300"
                      >
                        <div className="w-10 h-10 rounded-xl bg-primary/10 flex items-center justify-center text-primary mb-3 group-hover:scale-110 transition-transform">
                          <FontAwesomeIcon icon={c.icon} className="text-[18px]" />
                        </div>
                        <div className="text-[14px] font-semibold text-text-base mb-1 group-hover:text-primary transition-colors line-clamp-1 w-full text-left">{getTranslatedToolName(c.title, c.type)}</div>
                        <div className="text-[12px] text-text-secondary leading-snug line-clamp-2 text-left">{getTranslatedToolDesc(c.type)}</div>
                      </div>
                    ))}
                  </div>
                </div>

                <div className="pl-12 pb-4 text-[13px] text-text-secondary font-medium">{t("chatView.recommended")}</div>
              </div>
            )}

            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "files" && <FilesPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "chat" && <SideChatPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "browser" && <BrowserPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "terminal" && <TerminalPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "recording" && <RecordingPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "file_preview" && <FilePreviewPlugin />}
            {activeBottomTabId !== "new" && bottomTabs.find(t => t.id === activeBottomTabId)?.type === "project_map" && <ProjectMapPanel projectPath={activeProjectPath} />}
          </div>
        </div>
      )}
        </div>
        
      {/* Right Sidebar */}
      {isRightSidebarOpen && (
        <>
        
        {/* Drawer Panel */}
        <div 
          className={`absolute right-0 top-0 h-full bg-white flex flex-col z-50 shadow-[0_0_40px_rgba(0,0,0,0.1)] border-l border-border-theme transform translate-x-0 ${isResizingSidebar ? "" : "transition-transform duration-300"}`}
          style={{ width: `${rightSidebarWidth}px`, minWidth: '360px' }}
        >
          {/* Resize Handle */}
          <div 
            className="absolute left-0 top-0 bottom-0 w-1.5 cursor-col-resize hover:bg-blue-500/50 z-50 -ml-[1px]"
            onMouseDown={(e) => {
              e.preventDefault();
              setIsResizingSidebar(true);
            }}
          />
          <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-white">
            <div className="flex items-center text-[13px] text-text-secondary h-full overflow-x-auto no-scrollbar">
              {sidebarTabs.map(tab => (
                <div 
                  key={tab.id}
                  onClick={() => setActiveSidebarTabId(tab.id)}
                  className={`flex items-center h-full px-3 border-b-2 cursor-pointer flex-shrink-0 ${
                    activeSidebarTabId === tab.id 
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
                       const newTabs = sidebarTabs.filter(t => t.id !== tab.id);
                       setSidebarTabs(newTabs);
                       if (activeSidebarTabId === tab.id) {
                         setActiveSidebarTabId(newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new");
                       }
                    }}
                  />
                </div>
              ))}
              <div 
                className={`flex items-center h-full px-3 cursor-pointer flex-shrink-0 ${activeSidebarTabId === "new" ? "text-text-base" : "hover:text-text-base"}`}
                onClick={() => setActiveSidebarTabId("new")}
              >
                <FontAwesomeIcon icon={["fas", "plus"]} />
              </div>
            </div>
            <div className="flex items-center space-x-3 text-text-secondary ml-2 flex-shrink-0 bg-white pl-2">
              {sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "files" && (
                <>
                  <FontAwesomeIcon icon={["fas", "ellipsis"]} className="cursor-pointer hover:text-text-base" />
                  <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="cursor-pointer hover:text-text-base text-[13px]" />
                  <FontAwesomeIcon icon={["far", "copy"]} className="cursor-pointer hover:text-text-base" />
                </>
              )}
              <FontAwesomeIcon icon={["fas", "xmark"]} className="cursor-pointer hover:text-text-base ml-1" onClick={() => setIsRightSidebarOpen(false)} />
            </div>
          </div>

          <div className="flex-1 overflow-hidden flex flex-col relative">
            {activeSidebarTabId === "new" && (
              <div className="w-full h-full flex flex-col relative overflow-y-auto bg-white">
                <div className="flex-1 flex flex-col p-6">
                  <div className="flex flex-col space-y-3">
                    {TOOL_CARDS.map((c) => (
                      <div
                        key={c.title}
                        onClick={() => handleOpenSidebarPlugin(c)}
                        className="group flex items-center p-4 rounded-2xl bg-bg-base border border-border-theme cursor-pointer hover:shadow-md hover:border-primary/50 transition-all duration-200"
                      >
                        <div className="w-11 h-11 rounded-xl bg-primary/10 flex items-center justify-center text-primary shrink-0 mr-4 group-hover:scale-105 transition-transform">
                          <FontAwesomeIcon icon={c.icon} className="text-[18px]" />
                        </div>
                        <div className="flex-1 text-left min-w-0">
                          <div className="text-[14px] font-semibold text-text-base mb-0.5 group-hover:text-primary transition-colors truncate">{getTranslatedToolName(c.title, c.type)}</div>
                          <div className="text-[12px] text-text-secondary leading-snug line-clamp-1">{getTranslatedToolDesc(c.type)}</div>
                        </div>
                        <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-text-secondary opacity-0 group-hover:opacity-100 group-hover:-translate-x-1 transition-all ml-2 text-sm" />
                      </div>
                    ))}
                  </div>
                </div>
                <div className="px-6 pb-6 text-[13px] text-text-secondary font-medium">{t("chatView.recommended")}</div>
              </div>
            )}

            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "files" && <FilesPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "chat" && <SideChatPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "browser" && <BrowserPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "terminal" && <TerminalPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "recording" && <RecordingPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "file_preview" && <FilePreviewPlugin />}
            {activeSidebarTabId !== "new" && sidebarTabs.find(t => t.id === activeSidebarTabId)?.type === "project_map" && <ProjectMapPanel projectPath={activeProjectPath} />}
          </div>
        </div>
        </>
      )}
      </div>
      
    </div>
  );
}
