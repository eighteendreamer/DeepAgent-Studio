import { useState, useRef, useEffect, useMemo } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import { useSlidingIndicator, SlidingPill } from "./ui/SlidingPill";
import type { Project, SessionSummary } from "../types";

interface Props {
  sessions: SessionSummary[];
  projects: Project[];
  activeProjectPath: string | null;
  activeId: string | null;
  onSelect: (id: string) => void;
  onSelectProject: (path: string) => void;
  onNewChat: () => void;
  onAddProject: () => void;
  onPinSession: (id: string, pinned: boolean) => void;
  onArchiveSession: (id: string) => void;
  onArchiveAllSessions: () => void;
  onRemoveProject: (path: string) => void;
  onPinProject: (path: string, pinned: boolean) => void;
  onOpenProject: (path: string) => void;
  onOpenProjectMap: (path: string) => void;
  onRenameProject: (path: string, name: string) => void;
  onArchiveProject: (path: string, name: string) => void;
  onOpenSearch: () => void;
  activeSurface?: "skills" | "knowledge" | "plugins" | "automation" | null;
  onOpenSkills: () => void;
  onOpenKnowledge: () => void;
  onOpenPlugins: () => void;
  onOpenAutomation: () => void;
  onOpenSettings: () => void;
  onLogout: () => void;
  /** Session ids with currently-running agent runs (show spinners). */
  runningSessionIds?: Set<string>;
}

function NavButton({ icon, label, active = false, onClick, navId }: { icon: IconProp; label: string; active?: boolean; onClick?: () => void; navId: string }) {
  return (
    <button
      data-nav={navId}
      className={`relative z-[1] w-full flex items-center px-2.5 py-1.5 rounded-md text-sm text-text-base ${
        active ? "font-medium" : ""
      }`}
      onClick={onClick}
    >
      <FontAwesomeIcon icon={icon} className="w-5 text-left text-text-secondary" />
      <span className="ml-0.5">{label}</span>
    </button>
  );
}

function formatTimeAgo(timestamp: number) {
  const diff = Date.now() - timestamp;
  const minutes = Math.floor(diff / 60000);
  if (minutes < 60) return `${minutes || 1} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days} 天`;
  const weeks = Math.floor(days / 7);
  if (weeks < 4) return `${weeks} 周`;
  const months = Math.floor(days / 30);
  return `${months} 个月`;
}

type SidebarOrganizeMode = "project" | "recent" | "time" | "down";
type SidebarSortCriterion = "updated" | "created";
const SIDEBAR_ORGANIZE_MODES = ["project", "recent", "time", "down"] as const;
const SIDEBAR_SORT_CRITERIA = ["updated", "created"] as const;
const SIDEBAR_EXPANDED_PROJECTS_KEY = "deepagent:sidebar-expanded-projects";

function readSidebarPreference<T extends string>(key: string, fallback: T, allowed: readonly T[]): T {
  if (typeof window === "undefined") return fallback;
  const value = window.localStorage.getItem(key);
  return allowed.includes(value as T) ? (value as T) : fallback;
}

function readExpandedProjects(): Record<string, boolean> {
  if (typeof window === "undefined") return {};
  try {
    const value = window.localStorage.getItem(SIDEBAR_EXPANDED_PROJECTS_KEY);
    if (!value) return {};
    const parsed = JSON.parse(value);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean")
    );
  } catch {
    return {};
  }
}

function writeExpandedProjects(value: Record<string, boolean>) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(SIDEBAR_EXPANDED_PROJECTS_KEY, JSON.stringify(value));
}

export function Sidebar({ sessions, projects, activeProjectPath, activeId, onSelect, onSelectProject, onNewChat, onAddProject, onPinSession, onArchiveSession, onArchiveAllSessions, onRemoveProject, onPinProject, onOpenProject, onOpenProjectMap, onRenameProject, onArchiveProject, onOpenSearch, activeSurface, onOpenSkills, onOpenKnowledge, onOpenPlugins, onOpenAutomation, onOpenSettings, onLogout, runningSessionIds }: Props) {
  const { t } = useTranslation();

  /* 顶部导航滑动药丸（静默着色）：悬停跟随，离开滑回激活项；无 surface 激活时停靠「新对话」 */
  const activeNavId = activeSurface ?? "new-chat";
  const {
    containerRef: topNavRef,
    containerProps: topNavProps,
    indicatorStyle: pillStyle,
  } = useSlidingIndicator({
    hoverSelector: "[data-nav]",
    activeSelector: `[data-nav="${activeNavId}"]`,
  });

  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false);
  const [activeMoreSubmenu, setActiveMoreSubmenu] = useState<"organize" | "sort" | null>(null);
  const [moreSubmenuPosition, setMoreSubmenuPosition] = useState({ left: 252, top: 0 });
  const [isNewProjectMenuOpen, setIsNewProjectMenuOpen] = useState(false);
  const [activeProjectMenu, setActiveProjectMenu] = useState<string | null>(null);
  const [archiveProject, setArchiveProject] = useState<{ path: string; name: string } | null>(null);
  const [renameProject, setRenameProject] = useState<{ path: string; name: string } | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [removeProject, setRemoveProject] = useState<{ path: string; name: string } | null>(null);
  const [organizeMode, setOrganizeMode] = useState<SidebarOrganizeMode>(() =>
    readSidebarPreference("deepagent:sidebar-organize-mode", "project", SIDEBAR_ORGANIZE_MODES)
  );
  const [sortCriterion, setSortCriterion] = useState<SidebarSortCriterion>(() =>
    readSidebarPreference("deepagent:sidebar-sort-criterion", "updated", SIDEBAR_SORT_CRITERIA)
  );
  
  const settingsRef = useRef<HTMLDivElement>(null);
  const moreMenuRef = useRef<HTMLDivElement>(null);
  const newProjectMenuRef = useRef<HTMLDivElement>(null);
  const projMenuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (settingsRef.current && !settingsRef.current.contains(e.target as Node)) {
        setIsSettingsOpen(false);
      }
      if (moreMenuRef.current && !moreMenuRef.current.contains(e.target as Node)) {
        setIsMoreMenuOpen(false);
        setActiveMoreSubmenu(null);
      }
      if (newProjectMenuRef.current && !newProjectMenuRef.current.contains(e.target as Node)) {
        setIsNewProjectMenuOpen(false);
      }
      if (projMenuRef.current && !projMenuRef.current.contains(e.target as Node)) {
        setActiveProjectMenu(null);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  useEffect(() => {
    window.localStorage.setItem("deepagent:sidebar-organize-mode", organizeMode);
  }, [organizeMode]);

  useEffect(() => {
    window.localStorage.setItem("deepagent:sidebar-sort-criterion", sortCriterion);
  }, [sortCriterion]);

  const openMoreSubmenu = (submenu: "organize" | "sort") => {
    const rect = moreMenuRef.current?.getBoundingClientRect();
    const popupTop = (rect?.bottom ?? 0) + 4;
    setMoreSubmenuPosition({
      left: (rect?.right ?? 244) + 4,
      top: popupTop + (submenu === "organize" ? 40 : 76),
    });
    setActiveMoreSubmenu(submenu);
  };

  const sessionSortValue = (session: SessionSummary) =>
    sortCriterion === "created" ? session.created_at : session.updated_at;

  const sortSessions = (items: SessionSummary[]) =>
    [...items].sort((a, b) => sessionSortValue(b) - sessionSortValue(a));

  // Group sessions by their project (display name). Seed the map from the real
  // projects list so projects with no sessions yet still appear.
  const groupedSessions = useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    for (const p of projects) {
      groups[p.name] = [];
    }
    for (const s of sortSessions(sessions)) {
      if (s.pinned) continue;
      const proj = s.project || t("sidebar.noProjects");
      if (!groups[proj]) groups[proj] = [];
      groups[proj].push(s);
    }
    return groups;
  }, [sessions, projects, sortCriterion, t]);

  // Map a project display name back to its path (for selecting the active one).
  const nameToPath = useMemo(() => {
    const m: Record<string, string> = {};
    for (const p of projects) m[p.name] = p.path;
    return m;
  }, [projects]);

  const projectByName = useMemo(() => {
    const m: Record<string, Project> = {};
    for (const p of projects) m[p.name] = p;
    return m;
  }, [projects]);

  const pinnedSessions = useMemo(() => {
    return sessions.filter((s) => s.pinned);
  }, [sessions]);

  const pinnedProjectNames = useMemo(() => {
    return new Set(projects.filter((p) => p.pinned).map((p) => p.name));
  }, [projects]);

  const [expandedProjects, setExpandedProjects] = useState<Record<string, boolean>>(() => readExpandedProjects());

  // Default only newly discovered projects to expanded; preserve user-collapsed state across view changes.
  // During view switches the sidebar can mount before projects are loaded. Do not treat
  // that transient empty list as a signal to clear the persisted expansion map.
  useEffect(() => {
    if (projects.length === 0) return;
    setExpandedProjects((prev) => {
      let changed = false;
      const next: Record<string, boolean> = {};
      for (const p of projects) {
        if (p.name in prev) {
          next[p.name] = prev[p.name];
        } else {
          next[p.name] = true;
          changed = true;
        }
      }
      if (Object.keys(prev).some((name) => !projects.some((p) => p.name === name))) changed = true;
      if (changed) writeExpandedProjects(next);
      return next;
    });
  }, [projects]);

  const toggleProject = (proj: string) => {
    setExpandedProjects((prev) => {
      const next = { ...prev, [proj]: !prev[proj] };
      writeExpandedProjects(next);
      return next;
    });
  };

  const toggleExpandAll = () => {
    const allExpanded = Object.keys(groupedSessions).every((proj) => expandedProjects[proj]);
    let next: Record<string, boolean>;
    if (allExpanded) {
      next = {};
      Object.keys(groupedSessions).forEach((proj) => {
        next[proj] = false;
      });
    } else {
      next = {};
      Object.keys(groupedSessions).forEach((proj) => {
        next[proj] = true;
      });
    }
    writeExpandedProjects(next);
    setExpandedProjects(next);
  };

  const chronologicalSessions = useMemo(
    () => sortSessions(sessions.filter((s) => !s.pinned)),
    [sessions, sortCriterion]
  );

  const orderProjectEntries = (entries: [string, SessionSummary[]][]) => {
    const ordered = [...entries];
    if (organizeMode === "recent") {
      ordered.sort((a, b) => {
        const aTime = Math.max(
          projectByName[a[0]]?.updated_at ?? 0,
          ...a[1].map((s) => sessionSortValue(s))
        );
        const bTime = Math.max(
          projectByName[b[0]]?.updated_at ?? 0,
          ...b[1].map((s) => sessionSortValue(s))
        );
        return bTime - aTime || a[0].localeCompare(b[0], "zh-CN");
      });
    } else if (organizeMode === "down") {
      ordered.sort((a, b) => {
        const aEmpty = a[1].filter((s) => s.title).length === 0 ? 1 : 0;
        const bEmpty = b[1].filter((s) => s.title).length === 0 ? 1 : 0;
        return aEmpty - bEmpty || a[0].localeCompare(b[0], "zh-CN");
      });
    }
    return ordered;
  };

  const pinnedProjectEntries = orderProjectEntries(
    Object.entries(groupedSessions).filter(([proj]) => pinnedProjectNames.has(proj))
  );
  const projectEntries =
    organizeMode === "time"
      ? []
      : orderProjectEntries(
          Object.entries(groupedSessions).filter(([proj]) => !pinnedProjectNames.has(proj))
        );

  const renderSessionItem = (s: SessionSummary, isPinnedSection: boolean = false) => {
    const active = s.id === activeId;
    const isPinned = s.pinned;
    const isRunning = runningSessionIds?.has(s.id) ?? false;
    return (
      <div
        key={s.id + (isPinnedSection ? '_pinned' : '')}
        onClick={() => onSelect(s.id)}
        className={`flex items-center justify-between ${isPinnedSection ? 'px-2.5 py-1.5 mb-0.5' : 'pl-8 pr-2 py-1'} text-[12px] cursor-pointer rounded-md transition-colors group/session ${
          active
            ? "bg-black/5 text-text-base font-medium"
            : "text-text-secondary hover:bg-black/5 hover:text-text-base"
        }`}
      >
        {isRunning && (
          <FontAwesomeIcon
            icon={["fas", "circle-notch"]}
            spin
            className="text-[11px] text-blue-500 mr-1.5 flex-shrink-0"
            title={t("sidebar.running")}
          />
        )}
        <span className="truncate flex-1 pr-2">{s.title?.trim() || t("sidebar.newChat")}</span>

        {/* Right side container for timestamp and buttons using grid stacking */}
        <div className="grid items-center flex-shrink-0">
          {/* Running indicator takes precedence and never fades on hover. */}
          {isRunning ? (
            <span className="col-start-1 row-start-1 text-[10px] text-blue-500 whitespace-nowrap justify-self-end">
              {t("sidebar.running")}
            </span>
          ) : (
            <>
              {/* Timestamp (fades out on hover) */}
              {!isPinnedSection && (
                <span className="col-start-1 row-start-1 text-[10px] text-gray-400 group-hover/session:opacity-0 transition-opacity whitespace-nowrap justify-self-end">
                  {formatTimeAgo(s.created_at)}
                </span>
              )}

              {/* Action Buttons (fades in on hover) */}
              <div className="col-start-1 row-start-1 flex items-center space-x-0.5 opacity-0 pointer-events-none group-hover/session:opacity-100 group-hover/session:pointer-events-auto transition-opacity justify-self-end">
                <button
                  onClick={(e) => { e.stopPropagation(); onPinSession(s.id, !isPinned); }}
                  className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded text-text-secondary"
                  title={isPinned ? t("sidebar.unpin") : t("sidebar.pin")}
                >
                  <FontAwesomeIcon icon={["fas", "thumbtack"]} className="text-[10px]" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); onArchiveSession(s.id); }}
                  className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded text-text-secondary"
                  title={t("sidebar.archive")}
                >
                  <FontAwesomeIcon icon={["fas", "box-archive"]} className="text-[10px]" />
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    );
  };

  const renderProjectGroup = (proj: string, projSessions: SessionSummary[]) => {
    const isExpanded = expandedProjects[proj];
    const project = projectByName[proj];
    const isProjectPinned = project?.pinned ?? false;
    return (
      <div key={proj} className="flex flex-col">
        <div
          className={`flex items-center px-2.5 py-1.5 text-[13px] cursor-pointer hover:bg-black/5 rounded-md transition-colors group/proj ${activeProjectMenu === proj || nameToPath[proj] === activeProjectPath ? 'bg-black/5 text-text-base font-medium' : 'text-text-secondary'}`}
          onClick={() => {
            const path = nameToPath[proj];
            if (path) onSelectProject(path);
            toggleProject(proj);
          }}
        >
          <FontAwesomeIcon icon={["far", "folder"]} className="w-4 text-left mr-2" />
          <span className="truncate flex-1">{proj}</span>

          <div className={`flex items-center space-x-0.5 transition-opacity ${activeProjectMenu === proj ? 'opacity-100' : 'opacity-0 group-hover/proj:opacity-100'}`}>
            <div className="relative" ref={activeProjectMenu === proj ? projMenuRef : null}>
              <button
                className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded"
                title="项目选项"
                onClick={(e) => {
                  e.stopPropagation();
                  setActiveProjectMenu(activeProjectMenu === proj ? null : proj);
                }}
              >
                <FontAwesomeIcon icon={["fas", "ellipsis"]} className="text-[10px]" />
              </button>
              {activeProjectMenu === proj && (
                <div
                  className="absolute top-full right-0 mt-1 w-44 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-50 flex flex-col font-normal"
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                    onClick={() => {
                      const path = nameToPath[proj];
                      if (!path) return;
                      setActiveProjectMenu(null);
                      onPinProject(path, !isProjectPinned);
                    }}
                  >
                    <FontAwesomeIcon icon={["fas", "thumbtack"]} className="text-text-secondary mr-2.5 w-4" />
                    {isProjectPinned ? t("sidebar.unpinProject") : t("sidebar.pinProject")}
                  </button>
                  <button
                    className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                    onClick={() => {
                      const path = nameToPath[proj];
                      if (!path) return;
                      setActiveProjectMenu(null);
                      onOpenProject(path);
                    }}
                  >
                    <FontAwesomeIcon icon={["far", "folder"]} className="text-text-secondary mr-2.5 w-4" />
                    {t("sidebar.openInExplorer")}
                  </button>
                  <button
                    className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                    onClick={() => {
                      const path = nameToPath[proj];
                      if (!path) return;
                      setActiveProjectMenu(null);
                      onOpenProjectMap(path);
                    }}
                  >
                    <FontAwesomeIcon icon={["fas", "share-nodes"]} className="text-text-secondary mr-2.5 w-4" />
                    项目地图
                  </button>
                  <button
                    className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                    onClick={() => {
                      const path = nameToPath[proj];
                      if (!path) return;
                      setActiveProjectMenu(null);
                      setRenameProject({ path, name: proj });
                      setRenameValue(proj);
                    }}
                  >
                    <FontAwesomeIcon icon={["fas", "pen"]} className="text-text-secondary mr-2.5 w-4" />
                    {t("sidebar.renameProject")}
                  </button>
                  <div className="my-1 border-t border-border-theme"></div>
                  <button
                    className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                    onClick={() => {
                      const path = nameToPath[proj];
                      if (!path) return;
                      setActiveProjectMenu(null);
                      setArchiveProject({ path, name: proj });
                    }}
                  >
                    <FontAwesomeIcon icon={["fas", "box-archive"]} className="text-text-secondary mr-2.5 w-4" />
                    {t("sidebar.archive")}
                  </button>
                  <button
                    className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                    onClick={() => {
                      setActiveProjectMenu(null);
                      const path = nameToPath[proj];
                      if (path) setRemoveProject({ path, name: proj });
                    }}
                  >
                    <FontAwesomeIcon icon={["fas", "xmark"]} className="text-text-secondary mr-2.5 w-4" />
                    {t("sidebar.remove")}
                  </button>
                </div>
              )}
            </div>
            <button
              className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded"
              title={t("sidebar.newChat")}
              onClick={(e) => {
                e.stopPropagation();
                const path = nameToPath[proj];
                if (path) onSelectProject(path);
                onNewChat();
              }}
            >
              <FontAwesomeIcon icon={["far", "pen-to-square"]} className="text-[10px]" />
            </button>
          </div>
        </div>
        {isExpanded && (
          <div className="flex flex-col mt-0.5 space-y-0.5">
            {projSessions.length === 0 ? (
              <div className="pl-8 py-1 text-[12px] text-gray-400">{t("sidebar.noChats")}</div>
            ) : (
              projSessions.map((s) => renderSessionItem(s))
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <aside className="w-[240px] flex flex-col bg-sidebar-bg h-full no-select flex-shrink-0 pb-2">
      {/* Top actions：滑动药丸指示器（同设置侧栏），无 surface 激活时停靠「新对话」 */}
      <div
        ref={topNavRef}
        {...topNavProps}
        className="relative px-3 py-2"
      >
        <div className="space-y-0.5">
          <button
            data-nav="new-chat"
            className="relative z-[1] w-full flex items-center px-2.5 py-1.5 rounded-md text-sm text-text-base"
            onClick={onNewChat}
          >
            <FontAwesomeIcon icon={["far", "pen-to-square"]} className="w-5 text-left text-text-secondary" />
            <span className="ml-0.5">{t("sidebar.newChat")}</span>
          </button>
          <NavButton icon={["fas", "magnifying-glass"]} label={t("sidebar.search")} navId="search" onClick={onOpenSearch} />
          <NavButton icon={["fas", "layer-group"]} label={t("sidebar.skills")} navId="skills" active={activeSurface === "skills"} onClick={onOpenSkills} />
          <NavButton icon={["fas", "book"]} label={t("sidebar.knowledge")} navId="knowledge" active={activeSurface === "knowledge"} onClick={onOpenKnowledge} />
          <NavButton icon={["fas", "puzzle-piece"]} label={t("sidebar.plugins")} navId="plugins" active={activeSurface === "plugins"} onClick={onOpenPlugins} />
          <NavButton icon={["far", "clock"]} label={t("sidebar.automation")} navId="automation" active={activeSurface === "automation"} onClick={onOpenAutomation} />
        </div>

        {/* 滑动药丸指示器 */}
        <SlidingPill style={pillStyle} />
      </div>

      {/* Project / session list */}
      <div className="flex-1 overflow-y-auto px-2 mt-4 space-y-3 pb-2 custom-scrollbar">
        {/* Pinned projects and sessions */}
        {(pinnedProjectEntries.length > 0 || pinnedSessions.length > 0) && (
          <div className="flex flex-col">
            <div className="px-2 mb-1 text-[12px] text-text-secondary">{t("sidebar.pinned")}</div>
            <div className="space-y-0.5">
              {pinnedProjectEntries.map(([proj, projSessions]) =>
                renderProjectGroup(proj, projSessions)
              )}
              {pinnedSessions.map((s) => renderSessionItem(s, true))}
            </div>
          </div>
        )}

        <div className="flex flex-col">
          <div className="flex items-center justify-between px-2 mb-1 text-text-secondary group">
            <div className="text-[12px]">{t("sidebar.projects")}</div>
            <div className="flex items-center space-x-1 opacity-0 group-hover:opacity-100 transition-opacity">
              <button 
                className="w-5 h-5 flex items-center justify-center hover:bg-black/5 rounded" 
                title={t("sidebar.collapseAll")}
                onClick={toggleExpandAll}
              >
                <FontAwesomeIcon icon={["fas", "compress"]} className="text-[10px]" />
              </button>
              
              <div className="relative" ref={moreMenuRef}>
                <button 
                  className="w-5 h-5 flex items-center justify-center hover:bg-black/5 rounded" 
                  title={t("sidebar.more")}
                  onClick={() => {
                    setIsMoreMenuOpen(!isMoreMenuOpen);
                    setActiveMoreSubmenu(null);
                    setIsNewProjectMenuOpen(false);
                  }}
                >
                  <FontAwesomeIcon icon={["fas", "ellipsis"]} className="text-[10px]" />
                </button>
                {isMoreMenuOpen && (
                  <div className="absolute top-full right-0 mt-1 w-48 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-50 flex flex-col">
                    <button
                      className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                      onClick={() => {
                        setIsMoreMenuOpen(false);
                        setActiveMoreSubmenu(null);
                        onArchiveAllSessions();
                      }}
                    >
                      <FontAwesomeIcon icon={["fas", "box-archive"]} className="text-text-secondary mr-2.5 w-4" />
                      {t("sidebar.archiveAll")}
                    </button>
                    <div className="my-1 border-t border-border-theme"></div>
                    <button
                      className={`flex items-center justify-between px-3 py-2 text-[13px] text-text-base transition-colors w-full text-left ${activeMoreSubmenu === "organize" ? "bg-gray-50" : "hover:bg-black/5"}`}
                      onClick={() => {
                        if (activeMoreSubmenu === "organize") setActiveMoreSubmenu(null);
                        else openMoreSubmenu("organize");
                      }}
                    >
                      <div className="flex items-center">
                        <FontAwesomeIcon icon={["far", "folder"]} className="text-text-secondary mr-2.5 w-4" />
                        {t("sidebar.organizeSidebar")}
                      </div>
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-text-secondary text-[10px]" />
                    </button>
                    <button
                      className={`flex items-center justify-between px-3 py-2 text-[13px] text-text-base transition-colors w-full text-left ${activeMoreSubmenu === "sort" ? "bg-gray-50" : "hover:bg-black/5"}`}
                      onClick={() => {
                        if (activeMoreSubmenu === "sort") setActiveMoreSubmenu(null);
                        else openMoreSubmenu("sort");
                      }}
                    >
                      <div className="flex items-center">
                        <FontAwesomeIcon icon={["far", "clock"]} className="text-text-secondary mr-2.5 w-4" />
                        {t("sidebar.sortCriteria")}
                      </div>
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-text-secondary text-[10px]" />
                    </button>
                    {activeMoreSubmenu === "organize" && (
                      <div
                        className="fixed w-48 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-[200] flex flex-col"
                        style={{ left: moreSubmenuPosition.left, top: moreSubmenuPosition.top }}
                      >
                        {[
                          { id: "project" as const, icon: ["far", "folder"] as IconProp, label: t("sidebar.organizeByProject") },
                          { id: "recent" as const, icon: ["far", "folder"] as IconProp, label: t("sidebar.organizeRecentProjects") },
                          { id: "time" as const, icon: ["far", "clock"] as IconProp, label: t("sidebar.organizeByTime") },
                          { id: "down" as const, icon: ["fas", "arrow-down"] as IconProp, label: t("sidebar.organizeMoveDown") },
                        ].map((item) => (
                          <button
                            key={item.id}
                            className="flex items-center justify-between px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              setOrganizeMode(item.id);
                              setIsMoreMenuOpen(false);
                              setActiveMoreSubmenu(null);
                            }}
                          >
                            <div className="flex items-center">
                              <FontAwesomeIcon icon={item.icon} className="text-text-secondary mr-2.5 w-4" />
                              {item.label}
                            </div>
                            {organizeMode === item.id && (
                              <FontAwesomeIcon icon={["fas", "check"]} className="text-text-secondary text-[11px]" />
                            )}
                          </button>
                        ))}
                      </div>
                    )}
                    {activeMoreSubmenu === "sort" && (
                      <div
                        className="fixed w-44 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-[200] flex flex-col"
                        style={{ left: moreSubmenuPosition.left, top: moreSubmenuPosition.top }}
                      >
                        {[
                          { id: "created" as const, icon: ["far", "clock"] as IconProp, label: t("sidebar.sortByCreated") },
                          { id: "updated" as const, icon: ["far", "clock"] as IconProp, label: t("sidebar.sortByUpdated") },
                        ].map((item) => (
                          <button
                            key={item.id}
                            className="flex items-center justify-between px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              setSortCriterion(item.id);
                              setIsMoreMenuOpen(false);
                              setActiveMoreSubmenu(null);
                            }}
                          >
                            <div className="flex items-center">
                              <FontAwesomeIcon icon={item.icon} className="text-text-secondary mr-2.5 w-4" />
                              {item.label}
                            </div>
                            {sortCriterion === item.id && (
                              <FontAwesomeIcon icon={["fas", "check"]} className="text-text-secondary text-[11px]" />
                            )}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>

              <div className="relative" ref={newProjectMenuRef}>
                <button 
                  className="w-5 h-5 flex items-center justify-center hover:bg-black/5 rounded" 
                  title={t("sidebar.newProject")}
                  onClick={() => { setIsNewProjectMenuOpen(!isNewProjectMenuOpen); setIsMoreMenuOpen(false); }}
                >
                  <FontAwesomeIcon icon={["fas", "folder-plus"]} className="text-[10px]" />
                </button>
                {isNewProjectMenuOpen && (
                  <div className="absolute top-full right-0 mt-1 w-40 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-50 flex flex-col">
                    <button
                      className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                      onClick={() => { setIsNewProjectMenuOpen(false); onAddProject(); }}
                    >
                      <FontAwesomeIcon icon={["fas", "folder-plus"]} className="text-text-secondary mr-2.5 w-4" />
                      {t("sidebar.newBlankProject")}
                    </button>
                    <button
                      className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                      onClick={() => { setIsNewProjectMenuOpen(false); onAddProject(); }}
                    >
                      <FontAwesomeIcon icon={["fas", "folder-plus"]} className="text-text-secondary mr-2.5 w-4" />
                      {t("sidebar.useExistingFolder")}
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
          <div className="space-y-0.5">
            {projects.length === 0 && projectEntries.length === 0 && (
              <div className="px-2.5 py-1 text-[13px] text-text-secondary">{t("sidebar.noProjects")}</div>
            )}
            {organizeMode === "time" && chronologicalSessions.length === 0 && (
              <div className="px-2.5 py-1 text-[13px] text-text-secondary">{t("sidebar.noChats")}</div>
            )}
            {organizeMode === "time" &&
              chronologicalSessions.map((session) => renderSessionItem(session, true))}
          {projectEntries.map(([proj, projSessions]) => {
            const isExpanded = expandedProjects[proj];
            const project = projectByName[proj];
            const isProjectPinned = project?.pinned ?? false;
            return (
              <div key={proj} className="flex flex-col">
                <div
                  className={`flex items-center px-2.5 py-1.5 text-[13px] cursor-pointer hover:bg-black/5 rounded-md transition-colors group/proj ${activeProjectMenu === proj || nameToPath[proj] === activeProjectPath ? 'bg-black/5 text-text-base font-medium' : 'text-text-secondary'}`}
                  onClick={() => {
                    const path = nameToPath[proj];
                    if (path) onSelectProject(path);
                    toggleProject(proj);
                  }}
                >
                  <FontAwesomeIcon icon={["far", "folder"]} className="w-4 text-left mr-2" />
                  <span className="truncate flex-1">{proj}</span>
                  
                  <div className={`flex items-center space-x-0.5 transition-opacity ${activeProjectMenu === proj ? 'opacity-100' : 'opacity-0 group-hover/proj:opacity-100'}`}>
                    <div className="relative" ref={activeProjectMenu === proj ? projMenuRef : null}>
                      <button 
                        className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded" 
                        title="项目选项"
                        onClick={(e) => { 
                          e.stopPropagation(); 
                          setActiveProjectMenu(activeProjectMenu === proj ? null : proj);
                        }}
                      >
                        <FontAwesomeIcon icon={["fas", "ellipsis"]} className="text-[10px]" />
                      </button>
                      {activeProjectMenu === proj && (
                        <div 
                          className="absolute top-full right-0 mt-1 w-44 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-50 flex flex-col font-normal"
                          onClick={(e) => e.stopPropagation()}
                        >
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              const path = nameToPath[proj];
                              if (!path) return;
                              setActiveProjectMenu(null);
                              onPinProject(path, !isProjectPinned);
                            }}
                          >
                            <FontAwesomeIcon icon={["fas", "thumbtack"]} className="text-text-secondary mr-2.5 w-4" />
                            {isProjectPinned ? t("sidebar.unpinProject") : t("sidebar.pinProject")}
                          </button>
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              const path = nameToPath[proj];
                              if (!path) return;
                              setActiveProjectMenu(null);
                              onOpenProject(path);
                            }}
                          >
                            <FontAwesomeIcon icon={["far", "folder"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.openInExplorer")}
                          </button>
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              const path = nameToPath[proj];
                              if (!path) return;
                              setActiveProjectMenu(null);
                              onOpenProjectMap(path);
                            }}
                          >
                            <FontAwesomeIcon icon={["fas", "share-nodes"]} className="text-text-secondary mr-2.5 w-4" />
                            项目地图
                          </button>
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              const path = nameToPath[proj];
                              if (!path) return;
                              setActiveProjectMenu(null);
                              setRenameProject({ path, name: proj });
                              setRenameValue(proj);
                            }}
                          >
                            <FontAwesomeIcon icon={["fas", "pen"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.renameProject")}
                          </button>
                          <div className="my-1 border-t border-border-theme"></div>
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              const path = nameToPath[proj];
                              if (!path) return;
                              setActiveProjectMenu(null);
                              setArchiveProject({ path, name: proj });
                            }}
                          >
                            <FontAwesomeIcon icon={["fas", "box-archive"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.archive")}
                          </button>
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
                            onClick={() => {
                              setActiveProjectMenu(null);
                              const path = nameToPath[proj];
                              if (path) setRemoveProject({ path, name: proj });
                            }}
                          >
                            <FontAwesomeIcon icon={["fas", "xmark"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.remove")}
                          </button>
                        </div>
                      )}
                    </div>
                    <button 
                      className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded"
                      title={t("sidebar.newChat")}
                      onClick={(e) => { 
                        e.stopPropagation(); 
                        const path = nameToPath[proj];
                        if (path) onSelectProject(path);
                        onNewChat();
                      }}
                    >
                      <FontAwesomeIcon icon={["far", "pen-to-square"]} className="text-[10px]" />
                    </button>
                  </div>
                </div>
                {isExpanded && (
                  <div className="flex flex-col mt-0.5 space-y-0.5">
                    {projSessions.length === 0 ? (
                      <div className="pl-8 py-1 text-[12px] text-gray-400">{t("sidebar.noChats")}</div>
                    ) : (
                      projSessions.map((s) => renderSessionItem(s))
                    )}
                  </div>
                )}
              </div>
            );
          })}
          </div>
        </div>
      </div>

      {/* Bottom settings */}
      <div className="px-3 pt-2 relative" ref={settingsRef}>
        <button 
          className="w-full flex items-center px-2.5 py-2 rounded-md text-sm text-text-base hover:bg-black/5 transition-colors"
          onClick={() => setIsSettingsOpen(!isSettingsOpen)}
        >
          <FontAwesomeIcon icon={["fas", "gear"]} className="w-5 text-left text-text-secondary" />
          <span className="ml-0.5">{t("sidebar.settings")}</span>
        </button>

        {isSettingsOpen && (
          <div className="absolute bottom-full left-3 mb-1 w-56 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-50 flex flex-col">
            <div className="px-3 py-2.5 border-b border-border-theme flex items-center mb-1">
              <FontAwesomeIcon icon={["fas", "circle-user"]} className="text-text-secondary mr-2.5 text-base" />
              <div className="text-[13px] text-text-base font-medium truncate">
                {t("sidebar.loginApi")}
              </div>
            </div>
            
            <button 
              className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
              onClick={() => {
                setIsSettingsOpen(false);
                onOpenSettings();
              }}
            >
              <FontAwesomeIcon icon={["fas", "gear"]} className="text-text-secondary mr-2.5 w-4" />
              {t("sidebar.settings")}
            </button>
            <button 
              className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-black/5 transition-colors w-full text-left"
              onClick={() => { setIsSettingsOpen(false); onLogout(); }}
            >
              <FontAwesomeIcon icon={["fas", "arrow-right-from-bracket"]} className="text-text-secondary mr-2.5 w-4" />
              {t("sidebar.logout")}
            </button>
          </div>
        )}
      </div>

      {renameProject && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/20 px-4">
          <form
            className="w-full max-w-[420px] rounded-2xl border border-border-theme bg-white shadow-[0_20px_60px_rgba(15,23,42,0.18)]"
            onSubmit={(e) => {
              e.preventDefault();
              const nextName = renameValue.trim();
              if (!nextName) return;
              onRenameProject(renameProject.path, nextName);
              setRenameProject(null);
              setRenameValue("");
            }}
          >
            <div className="px-5 pt-5 pb-3">
              <div className="text-[17px] font-semibold text-text-base">
                {t("sidebar.renameProjectDialog.title")}
              </div>
              <input
                autoFocus
                className="mt-4 w-full rounded-xl border border-border-theme px-3 py-2 text-[14px] text-text-base outline-none focus:border-primary"
                placeholder={t("sidebar.renameProjectDialog.placeholder")}
                value={renameValue}
                onChange={(e) => setRenameValue(e.target.value)}
              />
            </div>
            <div className="flex justify-end gap-2 border-t border-border-theme px-5 py-3">
              <button
                type="button"
                className="rounded-full border border-border-theme px-4 py-1.5 text-[13px] text-text-base hover:bg-black/5 transition-colors"
                onClick={() => {
                  setRenameProject(null);
                  setRenameValue("");
                }}
              >
                {t("sidebar.renameProjectDialog.cancel")}
              </button>
              <button
                type="submit"
                disabled={!renameValue.trim()}
                className="rounded-full bg-primary px-4 py-1.5 text-[13px] font-medium text-white hover:bg-opacity-90 transition-colors disabled:cursor-not-allowed disabled:opacity-50"
              >
                {t("sidebar.renameProjectDialog.confirm")}
              </button>
            </div>
          </form>
        </div>
      )}

      {removeProject && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/20 px-4">
          <div className="w-full max-w-[420px] rounded-2xl border border-border-theme bg-white shadow-[0_20px_60px_rgba(15,23,42,0.18)]">
            <div className="px-5 pt-5 pb-3">
              <div className="text-[17px] font-semibold text-text-base">
                {t("sidebar.removeProjectDialog.title")}
              </div>
              <div className="mt-2 text-[13px] leading-6 text-text-secondary">
                {t("sidebar.removeProjectDialog.description", {
                  project: removeProject.name,
                })}
              </div>
            </div>
            <div className="flex justify-end gap-2 border-t border-border-theme px-5 py-3">
              <button
                className="rounded-full border border-border-theme px-4 py-1.5 text-[13px] text-text-base hover:bg-black/5 transition-colors"
                onClick={() => setRemoveProject(null)}
              >
                {t("sidebar.removeProjectDialog.cancel")}
              </button>
              <button
                className="rounded-full bg-primary px-4 py-1.5 text-[13px] font-medium text-white hover:bg-opacity-90 transition-colors"
                onClick={() => {
                  onRemoveProject(removeProject.path);
                  setRemoveProject(null);
                }}
              >
                {t("sidebar.removeProjectDialog.confirm")}
              </button>
            </div>
          </div>
        </div>
      )}

      {archiveProject && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/20 px-4">
          <div className="w-full max-w-[420px] rounded-2xl border border-border-theme bg-white shadow-[0_20px_60px_rgba(15,23,42,0.18)]">
            <div className="px-5 pt-5 pb-3">
              <div className="text-[17px] font-semibold text-text-base">
                {t("sidebar.archiveProjectDialog.title")}
              </div>
              <div className="mt-2 text-[13px] leading-6 text-text-secondary">
                {t("sidebar.archiveProjectDialog.description", {
                  project: archiveProject.name,
                })}
              </div>
            </div>
            <div className="flex justify-end gap-2 border-t border-border-theme px-5 py-3">
              <button
                className="rounded-full border border-border-theme px-4 py-1.5 text-[13px] text-text-base hover:bg-black/5 transition-colors"
                onClick={() => setArchiveProject(null)}
              >
                {t("sidebar.archiveProjectDialog.cancel")}
              </button>
              <button
                className="rounded-full bg-primary px-4 py-1.5 text-[13px] font-medium text-white hover:bg-opacity-90 transition-colors"
                onClick={() => {
                  onArchiveProject(archiveProject.path, archiveProject.name);
                  setArchiveProject(null);
                }}
              >
                {t("sidebar.archiveProjectDialog.confirm")}
              </button>
            </div>
          </div>
        </div>
      )}
    </aside>
  );
}
