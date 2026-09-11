import { useState, useRef, useEffect, useMemo } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Archive, ArrowDown, Book, Check, ChevronRight, ChevronsDownUp, Clock, Ellipsis, Folder, FolderPlus, Layers, Puzzle, Search, Server, Shapes, SquarePen, type LucideIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useSlidingIndicator, SlidingPill } from "./ui/SlidingPill";
import { SidebarProjectMenu } from "./SidebarProjectMenu";
import { SidebarSettingsMenu } from "./SidebarSettingsMenu";
import { Panel } from "./ui/Panel";
import { InputSurface } from "./ui/InputSurface";
import { TintButton } from "./ui/TintButton";
import { PinThumbtackIcon } from "./ui/PinThumbtackIcon";
import { cn } from "./shadcn/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "./shadcn/dropdown-menu";
import type { Project, SessionSummary } from "../types";

type SidebarOverflowSurface = "plugins" | "automation" | "canvas" | "remote";

/** 侧栏常驻 4 项之后的入口；新增功能页只需往这里追加。 */
const SIDEBAR_OVERFLOW_NAV: Array<{
  id: SidebarOverflowSurface;
  icon: LucideIcon;
  labelKey: "plugins" | "automation" | "canvas" | "remote";
}> = [
  { id: "plugins", icon: Puzzle, labelKey: "plugins" },
  { id: "automation", icon: Clock, labelKey: "automation" },
  { id: "canvas", icon: Shapes, labelKey: "canvas" },
  { id: "remote", icon: Server, labelKey: "remote" },
];

const SIDEBAR_OVERFLOW_IDS = new Set<string>(SIDEBAR_OVERFLOW_NAV.map((item) => item.id));

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
  activeSurface?: "skills" | "knowledge" | "plugins" | "automation" | "remote" | null;
  onOpenSkills: () => void;
  onOpenKnowledge: () => void;
  onOpenPlugins: () => void;
  onOpenAutomation: () => void;
  onOpenRemote: () => void;
  onOpenCanvas: () => void;
  onOpenSettings: () => void;
  onLogout: () => void;
  /** Session ids with currently-running agent runs (show spinners). */
  runningSessionIds?: Set<string>;
}

function NavButton({ icon: Icon, label, active = false, onClick, navId }: { icon: LucideIcon; label: string; active?: boolean; onClick?: () => void; navId: string }) {
  return (
    <button
      data-nav={navId}
      className={`relative z-[1] w-full flex items-center px-2.5 py-1.5 rounded-md text-sm text-text-base ${
        active ? "font-medium" : ""
      }`}
      onClick={onClick}
    >
      <Icon className="h-[18px] w-[18px] shrink-0 text-text-secondary" />
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

export function Sidebar({ sessions, projects, activeProjectPath, activeId, onSelect, onSelectProject, onNewChat, onAddProject, onPinSession, onArchiveSession, onArchiveAllSessions, onRemoveProject, onPinProject, onOpenProject, onOpenProjectMap, onRenameProject, onArchiveProject, onOpenSearch, activeSurface, onOpenSkills, onOpenKnowledge, onOpenPlugins, onOpenAutomation, onOpenRemote, onOpenCanvas, onOpenSettings, onLogout, runningSessionIds }: Props) {
  const { t } = useTranslation();

  /* 顶部导航滑动药丸（静默着色）：悬停跟随，离开滑回激活项；无 surface 激活时停靠「新对话」 */
  const overflowSurfaceActive = Boolean(activeSurface && SIDEBAR_OVERFLOW_IDS.has(activeSurface));
  const activeNavId = overflowSurfaceActive ? "other" : (activeSurface ?? "new-chat");
  const overflowNavActions: Record<SidebarOverflowSurface, () => void> = {
    plugins: onOpenPlugins,
    automation: onOpenAutomation,
    canvas: onOpenCanvas,
    remote: onOpenRemote,
  };
  const [overflowNavOpen, setOverflowNavOpen] = useState(false);
  const overflowCloseTimer = useRef<number | null>(null);

  const openOverflowNav = () => {
    if (overflowCloseTimer.current != null) {
      window.clearTimeout(overflowCloseTimer.current);
      overflowCloseTimer.current = null;
    }
    setOverflowNavOpen(true);
  };

  const scheduleCloseOverflowNav = () => {
    if (overflowCloseTimer.current != null) window.clearTimeout(overflowCloseTimer.current);
    overflowCloseTimer.current = window.setTimeout(() => {
      setOverflowNavOpen(false);
      overflowCloseTimer.current = null;
    }, 160);
  };
  const {
    containerRef: topNavRef,
    containerProps: topNavProps,
    indicatorStyle: pillStyle,
  } = useSlidingIndicator({
    hoverSelector: "[data-nav]",
    activeSelector: `[data-nav="${activeNavId}"]`,
  });

  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false);
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
  
  useEffect(() => {
    return () => {
      if (overflowCloseTimer.current != null) window.clearTimeout(overflowCloseTimer.current);
    };
  }, []);

  useEffect(() => {
    window.localStorage.setItem("deepagent:sidebar-organize-mode", organizeMode);
  }, [organizeMode]);

  useEffect(() => {
    window.localStorage.setItem("deepagent:sidebar-sort-criterion", sortCriterion);
  }, [sortCriterion]);

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
    const showActions = isPinned || isPinnedSection;
    return (
      <div
        key={s.id + (isPinnedSection ? '_pinned' : '')}
        onClick={() => onSelect(s.id)}
        className={cn(
          "group/session flex cursor-pointer items-center rounded-md text-[13px] transition-colors duration-150",
          isPinnedSection ? "mb-0.5 px-2.5 py-1.5" : "py-1.5 pl-[34px] pr-2.5",
          active
            ? "bg-sidebar-highlight font-medium text-text-base"
            : "text-text-secondary hover:bg-sidebar-highlight hover:text-text-base",
        )}
      >
        <div className="flex min-w-0 flex-1 items-center gap-1.5 pr-2">
          {isRunning && (
            <FontAwesomeIcon
              icon={["fas", "circle-notch"]}
              spin
              className="flex-shrink-0 text-[11px] text-blue-500"
              title={t("sidebar.running")}
            />
          )}
          <span className="truncate">{s.title?.trim() || t("sidebar.newChat")}</span>
        </div>

        {/* Fixed-width slot: timestamp ↔ actions swap without shifting title */}
        <div className="relative h-5 w-11 flex-shrink-0">
          {isRunning ? (
            <span className="absolute inset-0 flex items-center justify-end text-[10px] text-blue-500 whitespace-nowrap">
              {t("sidebar.running")}
            </span>
          ) : (
            <>
              {!isPinnedSection && (
                <span className="absolute inset-0 flex items-center justify-end text-[10px] text-text-secondary whitespace-nowrap transition-opacity duration-150 group-hover/session:opacity-0">
                  {formatTimeAgo(s.created_at)}
                </span>
              )}
              <div
                className={cn(
                  "absolute inset-0 flex items-center justify-end gap-0.5 transition-opacity duration-150",
                  showActions
                    ? "opacity-100"
                    : "pointer-events-none opacity-0 group-hover/session:pointer-events-auto group-hover/session:opacity-100",
                )}
              >
                <button
                  type="button"
                  onClick={(e) => { e.stopPropagation(); onPinSession(s.id, !isPinned); }}
                  className={cn(
                    "flex h-5 w-5 items-center justify-center rounded hover:bg-sidebar-highlight",
                    isPinned ? "text-text-base" : "text-text-secondary",
                  )}
                  title={isPinned ? t("sidebar.unpin") : t("sidebar.pin")}
                >
                  <PinThumbtackIcon pinned={isPinned} />
                </button>
                <button
                  type="button"
                  onClick={(e) => { e.stopPropagation(); onArchiveSession(s.id); }}
                  className="flex h-5 w-5 items-center justify-center rounded text-text-secondary hover:bg-sidebar-highlight"
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
          className={`flex items-center px-2.5 py-1.5 text-[13px] cursor-pointer hover:bg-sidebar-highlight rounded-md transition-colors group/proj ${activeProjectMenu === proj || nameToPath[proj] === activeProjectPath ? 'bg-sidebar-highlight text-text-base font-medium' : 'text-text-secondary'}`}
          onClick={() => {
            const path = nameToPath[proj];
            if (path) onSelectProject(path);
            toggleProject(proj);
          }}
        >
          <FontAwesomeIcon icon={["far", "folder"]} className="w-4 shrink-0 text-left mr-2 text-text-secondary" />
          <span className="truncate flex-1">{proj}</span>

          <div className={`flex items-center space-x-0.5 transition-opacity ${activeProjectMenu === proj || isProjectPinned ? 'opacity-100' : 'opacity-0 group-hover/proj:opacity-100'}`}>
            <button
              type="button"
              className={cn(
                "w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded",
                isProjectPinned && "text-text-base",
              )}
              title={isProjectPinned ? t("sidebar.unpinProject") : t("sidebar.pinProject")}
              onClick={(e) => {
                e.stopPropagation();
                const path = nameToPath[proj];
                if (path) onPinProject(path, !isProjectPinned);
              }}
            >
              <PinThumbtackIcon pinned={isProjectPinned} />
            </button>
            <SidebarProjectMenu
              isPinned={isProjectPinned}
              open={activeProjectMenu === proj}
              onOpenChange={(next) => setActiveProjectMenu(next ? proj : null)}
              onPin={() => {
                const path = nameToPath[proj];
                if (path) onPinProject(path, !isProjectPinned);
              }}
              onOpenExplorer={() => {
                const path = nameToPath[proj];
                if (path) onOpenProject(path);
              }}
              onOpenMap={() => {
                const path = nameToPath[proj];
                if (path) onOpenProjectMap(path);
              }}
              onRename={() => {
                const path = nameToPath[proj];
                if (!path) return;
                setRenameProject({ path, name: proj });
                setRenameValue(proj);
              }}
              onArchive={() => {
                const path = nameToPath[proj];
                if (path) setArchiveProject({ path, name: proj });
              }}
              onRemove={() => {
                const path = nameToPath[proj];
                if (path) setRemoveProject({ path, name: proj });
              }}
            />
            <button
              className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded"
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
    <aside className="w-[220px] flex flex-col bg-sidebar-bg h-full no-select flex-shrink-0 pb-2">
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
            <SquarePen className="h-[18px] w-[18px] shrink-0 text-text-secondary" />
            <span className="ml-0.5">{t("sidebar.newChat")}</span>
          </button>
          <NavButton icon={Search} label={t("sidebar.search")} navId="search" onClick={onOpenSearch} />
          <NavButton icon={Layers} label={t("sidebar.skills")} navId="skills" active={activeSurface === "skills"} onClick={onOpenSkills} />
          <NavButton icon={Book} label={t("sidebar.knowledge")} navId="knowledge" active={activeSurface === "knowledge"} onClick={onOpenKnowledge} />
          <DropdownMenu open={overflowNavOpen} onOpenChange={setOverflowNavOpen} modal={false}>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                data-nav="other"
                className={`relative z-[1] w-full flex items-center px-2.5 py-1.5 rounded-md text-sm text-text-base ${
                  overflowSurfaceActive ? "font-medium" : ""
                }`}
                onPointerEnter={openOverflowNav}
                onPointerLeave={scheduleCloseOverflowNav}
              >
                <Ellipsis className="h-[18px] w-[18px] shrink-0 text-text-secondary" />
                <span className="ml-0.5">{t("sidebar.other")}</span>
                <ChevronRight className="ml-auto h-3 w-3 text-text-secondary" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent
              side="right"
              align="start"
              sideOffset={6}
              className="min-w-[10.5rem]"
              onPointerEnter={openOverflowNav}
              onPointerLeave={scheduleCloseOverflowNav}
            >
              {SIDEBAR_OVERFLOW_NAV.map((item) => (
                <DropdownMenuItem
                  key={item.id}
                  className={cn(
                    "gap-2 text-sm",
                    activeSurface === item.id && "font-medium bg-black/5"
                  )}
                  onSelect={() => overflowNavActions[item.id]()}
                >
                  <item.icon className="h-4 w-4 shrink-0 text-text-secondary" />
                  {t(`sidebar.${item.labelKey}`)}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* 滑动药丸指示器 */}
        <SlidingPill style={pillStyle} className="bg-sidebar-highlight" />
      </div>

      {/* Project / session list */}
      <div className="stable-scrollbar-gutter flex-1 overflow-y-auto px-2 mt-4 space-y-3 pb-2 custom-scrollbar">
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
            <div className={cn("flex items-center space-x-1 transition-opacity", isMoreMenuOpen || isNewProjectMenuOpen ? "opacity-100" : "opacity-0 group-hover:opacity-100")}>
              <button 
                className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded" 
                title={t("sidebar.collapseAll")}
                onClick={toggleExpandAll}
              >
                <ChevronsDownUp className="h-3 w-3" />
              </button>
              
              <DropdownMenu
                open={isMoreMenuOpen}
                onOpenChange={(next) => {
                  setIsMoreMenuOpen(next);
                  if (next) setIsNewProjectMenuOpen(false);
                }}
              >
                <DropdownMenuTrigger asChild title={t("sidebar.more")}>
                  <button className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded">
                    <Ellipsis className="h-3 w-3" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="min-w-[12rem]">
                  <DropdownMenuItem className="gap-2" onSelect={() => onArchiveAllSessions()}>
                    <Archive className="h-4 w-4 shrink-0 text-text-secondary" />
                    {t("sidebar.archiveAll")}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuSub>
                    <DropdownMenuSubTrigger>
                      <span className="flex items-center gap-2">
                        <Folder className="h-4 w-4 shrink-0 text-text-secondary" />
                        {t("sidebar.organizeSidebar")}
                      </span>
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="min-w-[12rem]">
                      {[
                        { id: "project" as const, icon: Folder, label: t("sidebar.organizeByProject") },
                        { id: "recent" as const, icon: Folder, label: t("sidebar.organizeRecentProjects") },
                        { id: "time" as const, icon: Clock, label: t("sidebar.organizeByTime") },
                        { id: "down" as const, icon: ArrowDown, label: t("sidebar.organizeMoveDown") },
                      ].map((item) => (
                        <DropdownMenuItem
                          key={item.id}
                          className="gap-2"
                          onSelect={() => setOrganizeMode(item.id)}
                        >
                          <item.icon className="h-4 w-4 shrink-0 text-text-secondary" />
                          {item.label}
                          {organizeMode === item.id && <Check className="ml-auto h-3.5 w-3.5 text-text-secondary" />}
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                  <DropdownMenuSub>
                    <DropdownMenuSubTrigger>
                      <span className="flex items-center gap-2">
                        <Clock className="h-4 w-4 shrink-0 text-text-secondary" />
                        {t("sidebar.sortCriteria")}
                      </span>
                    </DropdownMenuSubTrigger>
                    <DropdownMenuSubContent className="min-w-[11rem]">
                      {[
                        { id: "created" as const, icon: Clock, label: t("sidebar.sortByCreated") },
                        { id: "updated" as const, icon: Clock, label: t("sidebar.sortByUpdated") },
                      ].map((item) => (
                        <DropdownMenuItem
                          key={item.id}
                          className="gap-2"
                          onSelect={() => setSortCriterion(item.id)}
                        >
                          <item.icon className="h-4 w-4 shrink-0 text-text-secondary" />
                          {item.label}
                          {sortCriterion === item.id && <Check className="ml-auto h-3.5 w-3.5 text-text-secondary" />}
                        </DropdownMenuItem>
                      ))}
                    </DropdownMenuSubContent>
                  </DropdownMenuSub>
                </DropdownMenuContent>
              </DropdownMenu>

              <DropdownMenu
                open={isNewProjectMenuOpen}
                onOpenChange={(next) => {
                  setIsNewProjectMenuOpen(next);
                  if (next) setIsMoreMenuOpen(false);
                }}
              >
                <DropdownMenuTrigger asChild title={t("sidebar.newProject")}>
                  <button className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded">
                    <FolderPlus className="h-3 w-3" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="min-w-[10rem]">
                  <DropdownMenuItem className="gap-2" onSelect={() => onAddProject()}>
                    <FolderPlus className="h-4 w-4 shrink-0 text-text-secondary" />
                    {t("sidebar.newBlankProject")}
                  </DropdownMenuItem>
                  <DropdownMenuItem className="gap-2" onSelect={() => onAddProject()}>
                    <FolderPlus className="h-4 w-4 shrink-0 text-text-secondary" />
                    {t("sidebar.useExistingFolder")}
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
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
            {projectEntries.map(([proj, projSessions]) => renderProjectGroup(proj, projSessions))}
          </div>
        </div>
      </div>

      {/* Bottom settings */}
      <div className="px-3 pt-2">
        <SidebarSettingsMenu onOpenSettings={onOpenSettings} onLogout={onLogout} />
      </div>

      {renameProject && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/20 px-4">
          <form
            className="w-full max-w-[340px]"
            onSubmit={(e) => {
              e.preventDefault();
              const nextName = renameValue.trim();
              if (!nextName) return;
              onRenameProject(renameProject.path, nextName);
              setRenameProject(null);
              setRenameValue("");
            }}
          >
            <Panel menu={false} className="p-5 shadow-none">
              <div className="text-[15px] font-semibold text-text-base">
                {t("sidebar.renameProjectDialog.title")}
              </div>
              <InputSurface className="mt-4 rounded-2xl bg-ui-tint shadow-none focus-within:shadow-none">
                <input
                  autoFocus
                  className="w-full bg-transparent px-4 py-2.5 text-[14px] text-text-base outline-none placeholder:text-text-secondary"
                  placeholder={t("sidebar.renameProjectDialog.placeholder")}
                  value={renameValue}
                  onChange={(e) => setRenameValue(e.target.value)}
                />
              </InputSurface>
              <div className="mt-5 flex justify-end gap-2">
                <TintButton
                  type="button"
                  className="px-4 py-2 text-[13px]"
                  onClick={() => {
                    setRenameProject(null);
                    setRenameValue("");
                  }}
                >
                  {t("sidebar.renameProjectDialog.cancel")}
                </TintButton>
                <TintButton
                  type="submit"
                  disabled={!renameValue.trim()}
                  className="px-4 py-2 text-[13px] font-semibold"
                >
                  {t("sidebar.renameProjectDialog.confirm")}
                </TintButton>
              </div>
            </Panel>
          </form>
        </div>
      )}

      {removeProject && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/20 px-4">
          <Panel menu={false} className="w-full max-w-[340px] p-5 shadow-none">
            <div className="text-[15px] font-semibold text-text-base">
              {t("sidebar.removeProjectDialog.title")}
            </div>
            <p className="mt-2 text-[13px] leading-relaxed text-text-secondary">
              {t("sidebar.removeProjectDialog.description", {
                project: removeProject.name,
              })}
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <TintButton
                type="button"
                className="px-4 py-2 text-[13px]"
                onClick={() => setRemoveProject(null)}
              >
                {t("sidebar.removeProjectDialog.cancel")}
              </TintButton>
              <TintButton
                type="button"
                className="px-4 py-2 text-[13px] font-semibold"
                onClick={() => {
                  onRemoveProject(removeProject.path);
                  setRemoveProject(null);
                }}
              >
                {t("sidebar.removeProjectDialog.confirm")}
              </TintButton>
            </div>
          </Panel>
        </div>
      )}

      {archiveProject && (
        <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/20 px-4">
          <Panel menu={false} className="w-full max-w-[340px] p-5 shadow-none">
            <div className="text-[15px] font-semibold text-text-base">
              {t("sidebar.archiveProjectDialog.title")}
            </div>
            <p className="mt-2 text-[13px] leading-relaxed text-text-secondary">
              {t("sidebar.archiveProjectDialog.description", {
                project: archiveProject.name,
              })}
            </p>
            <div className="mt-5 flex justify-end gap-2">
              <TintButton
                type="button"
                className="px-4 py-2 text-[13px]"
                onClick={() => setArchiveProject(null)}
              >
                {t("sidebar.archiveProjectDialog.cancel")}
              </TintButton>
              <TintButton
                type="button"
                className="px-4 py-2 text-[13px] font-semibold"
                onClick={() => {
                  onArchiveProject(archiveProject.path, archiveProject.name);
                  setArchiveProject(null);
                }}
              >
                {t("sidebar.archiveProjectDialog.confirm")}
              </TintButton>
            </div>
          </Panel>
        </div>
      )}
    </aside>
  );
}
