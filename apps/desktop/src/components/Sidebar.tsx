import { HoverInfo } from "./ui/HoverInfo";
import { useState, useEffect, useMemo } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { Archive, ArrowDown, Check, ChevronDown, Clock, Ellipsis, Folder, FolderPlus, PanelLeft, SquarePen } from "lucide-react";
import { useTranslation } from "react-i18next";
import { SidebarProjectMenu } from "./SidebarProjectMenu";
import { SidebarSectionGroup } from "./SidebarSectionGroup";
import { EditProjectDialog, type EditingProject } from "./EditProjectDialog";
import { SidebarSessionMenu, type SidebarSessionActions } from "./SidebarSessionMenu";
import type { SessionMenuOptions } from "./ChatSessionMenu";
import { useSidebarPreferences } from "./sidebarPreferences";
import { partitionSidebarSessions, type SidebarSortCriterion } from "./sidebarSessions";
import { Button } from "./shadcn/button";
import { Panel } from "./ui/Panel";
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

interface Props {
  sessions: SessionSummary[];
  projects: Project[];
  activeProjectPath: string | null;
  activeId: string | null;
  onSelect: (id: string) => void;
  onSelectProject: (path: string | null) => void;
  onNewChat: () => void;
  onNewSectionChat?: (sectionId: string) => void;
  onArchiveSection?: (sectionId: string) => Promise<void>;
  onAddProject: () => void;
  onPinSession: (id: string, pinned: boolean) => void;
  onArchiveSession: (id: string) => void;
  sessionActions?: SidebarSessionActions;
  sessionMenuOptions?: (id: string) => SessionMenuOptions;
  onArchiveAllSessions: () => void;
  onRemoveProject: (path: string) => void;
  onPinProject: (path: string, pinned: boolean) => void;
  onOpenProject: (path: string) => void;
  onOpenProjectMap: (path: string) => void;
  onEditProject: (path: string, name: string, folder: string) => Promise<void>;
  onAddProjectSection: () => void;
  onArchiveProject: (path: string, name: string) => void;
  onCollapse: () => void;
  /** Session ids with currently-running agent runs (show spinners). */
  runningSessionIds?: Set<string>;
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
const SIDEBAR_ORGANIZE_MODES = ["project", "recent", "time", "down"] as const;
const SIDEBAR_SORT_CRITERIA = ["updated", "created"] as const;
const SIDEBAR_EXPANDED_PROJECTS_KEY = "deepagent:sidebar-expanded-projects";
const SIDEBAR_PROJECTS_COLLAPSED_KEY = "deepagent:sidebar-projects-collapsed";
const SIDEBAR_RECENT_COLLAPSED_KEY = "deepagent:sidebar-recent-collapsed";

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

function readProjectsCollapsed(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(SIDEBAR_PROJECTS_COLLAPSED_KEY) === "1";
}

function writeProjectsCollapsed(collapsed: boolean) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(SIDEBAR_PROJECTS_COLLAPSED_KEY, collapsed ? "1" : "0");
}

function readRecentCollapsed(): boolean {
  if (typeof window === "undefined") return false;
  return window.localStorage.getItem(SIDEBAR_RECENT_COLLAPSED_KEY) === "1";
}

function writeRecentCollapsed(collapsed: boolean) {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(SIDEBAR_RECENT_COLLAPSED_KEY, collapsed ? "1" : "0");
}

export function Sidebar({ sessions, projects, activeProjectPath, activeId, onSelect, onSelectProject, onNewChat, onNewSectionChat, onArchiveSection, onAddProject, onPinSession, onArchiveSession, sessionActions, sessionMenuOptions, onArchiveAllSessions, onRemoveProject, onPinProject, onOpenProject, onOpenProjectMap, onEditProject, onAddProjectSection, onArchiveProject, onCollapse, runningSessionIds }: Props) {
  const { t } = useTranslation();

  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false);
  const [isNewProjectMenuOpen, setIsNewProjectMenuOpen] = useState(false);
  const [isRecentMenuOpen, setIsRecentMenuOpen] = useState(false);
  const [sessionMenu, setSessionMenu] = useState<{
    id: string; position: { left: number; top: number }; open: boolean;
  } | null>(null);
  const menuSession = sessions.find((session) => session.id === sessionMenu?.id);
  const [activeProjectMenu, setActiveProjectMenu] = useState<{
    project: string;
    position?: { top: number; left: number };
  } | null>(null);
  const [archiveProject, setArchiveProject] = useState<{ path: string; name: string } | null>(null);
  const [editingProject, setEditingProject] = useState<EditingProject | null>(null);
  const [removeProject, setRemoveProject] = useState<{ path: string; name: string } | null>(null);
  const [organizeMode, setOrganizeMode] = useState<SidebarOrganizeMode>(() =>
    readSidebarPreference("deepagent:sidebar-organize-mode", "project", SIDEBAR_ORGANIZE_MODES)
  );
  const [sortCriterion, setSortCriterion] = useState<SidebarSortCriterion>(() =>
    readSidebarPreference("deepagent:sidebar-sort-criterion", "updated", SIDEBAR_SORT_CRITERIA)
  );

  useEffect(() => {
    window.localStorage.setItem("deepagent:sidebar-organize-mode", organizeMode);
  }, [organizeMode]);

  useEffect(() => {
    window.localStorage.setItem("deepagent:sidebar-sort-criterion", sortCriterion);
  }, [sortCriterion]);

  const preferences = useSidebarPreferences((state) => state.preferences);
  const [collapsedSections, setCollapsedSections] = useState<Record<string, boolean>>({});
  const sessionSortValue = (session: SessionSummary) =>
    sortCriterion === "created" ? session.created_at : session.updated_at;

  const { projectSessions, recentSessions } = useMemo(
    () => partitionSidebarSessions(sessions.filter((session) => !preferences.session_sections[session.id]), sortCriterion),
    [sessions, sortCriterion, preferences.session_sections]
  );

  // Group sessions by their project (display name). Seed the map from the real
  // projects list so projects with no sessions yet still appear.
  const groupedSessions = useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    for (const p of projects) {
      groups[p.name] = [];
    }
    for (const s of projectSessions) {
      const proj = s.project;
      if (!proj) continue;
      if (!groups[proj]) groups[proj] = [];
      groups[proj].push(s);
    }
    return groups;
  }, [projectSessions, projects]);

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

  const pinnedProjectNames = useMemo(() => {
    return new Set(projects.filter((p) => p.pinned).map((p) => p.name));
  }, [projects]);

  const [expandedProjects, setExpandedProjects] = useState<Record<string, boolean>>(() => readExpandedProjects());
  const [projectsCollapsed, setProjectsCollapsed] = useState<boolean>(readProjectsCollapsed);
  const [recentCollapsed, setRecentCollapsed] = useState<boolean>(readRecentCollapsed);

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

  const toggleProjectsSection = () => {
    setProjectsCollapsed((prev) => {
      const next = !prev;
      writeProjectsCollapsed(next);
      return next;
    });
  };

  const toggleRecentSection = () => {
    setRecentCollapsed((prev) => {
      const next = !prev;
      writeRecentCollapsed(next);
      return next;
    });
  };

  const projectSessionsByTime = useMemo(
    () => [...projectSessions].sort((a, b) =>
      Number(pinnedProjectNames.has(b.project ?? "")) - Number(pinnedProjectNames.has(a.project ?? ""))
    ),
    [projectSessions, pinnedProjectNames]
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
    ordered.sort((a, b) => Number(pinnedProjectNames.has(b[0])) - Number(pinnedProjectNames.has(a[0])));
    return ordered;
  };

  const projectEntries =
    organizeMode === "time"
      ? []
      : orderProjectEntries(Object.entries(groupedSessions));
  const pinnedProjectEntries = projectEntries.filter(([name]) => pinnedProjectNames.has(name));
  const regularProjectEntries = projectEntries.filter(([name]) => !pinnedProjectNames.has(name));
  const pinnedRecentSessions = recentSessions.filter(session => session.pinned);
  const regularRecentSessions = recentSessions.filter(session => !session.pinned);

  const renderSessionItem = (s: SessionSummary, isTopLevel: boolean = false) => {
    const active = s.id === activeId;
    const isPinned = s.pinned;
    const isRunning = runningSessionIds?.has(s.id) ?? false;
    return (
      <div
        key={s.id}
        data-session-id={s.id}
        data-running={isRunning}
        data-custom-contextmenu={sessionActions ? "1" : undefined}
        onContextMenu={(event) => {
          if (!sessionActions) return;
          event.preventDefault();
          event.stopPropagation();
          setActiveProjectMenu(null);
          setSessionMenu({ id: s.id, position: { left: event.clientX, top: event.clientY }, open: true });
        }}
        className={cn(
          "sidebar-session-row group/session relative flex items-center transition-colors duration-150",
          active
            ? "bg-sidebar-highlight text-text-base"
            : "text-text-base hover:bg-sidebar-highlight",
        )}
      >
        <HoverInfo content={`${s.title?.trim() || t("sidebar.newChat")} · ${formatTimeAgo(s.updated_at)}`} side="right">
        <Button variant="ghost" onClick={() => onSelect(s.id)} aria-current={active ? "page" : undefined}
          className={cn("sidebar-session-trigger h-8 min-w-0 w-full justify-start py-1.5 text-sm font-normal text-text-base hover:!bg-transparent",
            isTopLevel ? "px-2.5" : "pl-[34px] pr-2.5")}>
        <span className="sidebar-session-content flex min-w-0 flex-1 items-center gap-1.5">
          {isRunning && (
            <FontAwesomeIcon
              icon={["fas", "circle-notch"]}
              spin
              className="flex-shrink-0 text-[11px] text-blue-500"
            />
          )}
          {preferences.unread[s.id] && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-blue-500" aria-label="未读"/>}
          <span className={cn("truncate", preferences.unread[s.id] && "font-medium text-text-base")}>{s.title?.trim() || t("sidebar.newChat")}</span>
        </span>
        </Button>
        </HoverInfo>

        <div className="pointer-events-none absolute right-2 top-1/2 h-5 w-11 -translate-y-1/2">
          {isRunning ? (
            <span className="absolute inset-0 flex items-center justify-end text-[10px] text-blue-500 whitespace-nowrap">
              {t("sidebar.running")}
            </span>
          ) : (
            <>
              <div
                className={cn(
                  "absolute inset-0 flex items-center justify-end gap-0.5 transition-opacity duration-150",
                  "pointer-events-none opacity-0 group-hover/session:pointer-events-auto group-hover/session:opacity-100 group-focus-within/session:pointer-events-auto group-focus-within/session:opacity-100",
                )}
              >
                <HoverInfo content={isPinned ? t("sidebar.unpin") : t("sidebar.pin")}><button
                  type="button"
                  onClick={(e) => { e.stopPropagation(); onPinSession(s.id, !isPinned); }}
                  className={cn(
                    "flex h-5 w-5 items-center justify-center rounded hover:bg-sidebar-highlight",
                    isPinned ? "text-text-base" : "text-text-secondary",
                  )}

                >
                  <PinThumbtackIcon pinned={isPinned} />
                </button></HoverInfo>
                <HoverInfo content={t("sidebar.archive")}><button
                  type="button"
                  onClick={(e) => { e.stopPropagation(); onArchiveSession(s.id); }}
                  className="flex h-5 w-5 items-center justify-center rounded text-text-secondary hover:bg-sidebar-highlight"

                >
                  <Archive className="h-3.5 w-3.5 shrink-0" />
                </button></HoverInfo>
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
          data-custom-contextmenu="1"
          className={`sidebar-project-row relative flex items-center px-2.5 py-1.5 cursor-pointer hover:bg-sidebar-highlight transition-colors group/proj ${activeProjectMenu?.project === proj || nameToPath[proj] === activeProjectPath ? 'bg-sidebar-highlight text-text-base' : 'text-text-base'}`}
          data-actions-visible={activeProjectMenu?.project === proj || isProjectPinned}
          onContextMenu={(event) => {
            event.preventDefault();
            event.stopPropagation();
            setActiveProjectMenu({
              project: proj,
              position: { top: event.clientY, left: event.clientX },
            });
          }}
          onClick={() => {
            const path = nameToPath[proj];
            if (path) onSelectProject(path);
            toggleProject(proj);
          }}
        >
          <FontAwesomeIcon icon={["far", "folder"]} className="w-4 shrink-0 text-left mr-2 text-text-secondary" />
          <span className="sidebar-project-title truncate flex-1">{proj}</span>

          <div className={`absolute right-2 flex items-center space-x-0.5 transition-opacity ${activeProjectMenu?.project === proj || isProjectPinned ? 'opacity-100' : 'pointer-events-none opacity-0 group-hover/proj:pointer-events-auto group-hover/proj:opacity-100 group-focus-within/proj:pointer-events-auto group-focus-within/proj:opacity-100'}`}>
            <HoverInfo content={isProjectPinned ? t("sidebar.unpinProject") : t("sidebar.pinProject")}><button
              type="button"
              className={cn(
                "w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded",
                isProjectPinned && "text-text-base",
              )}

              onClick={(e) => {
                e.stopPropagation();
                const path = nameToPath[proj];
                if (path) onPinProject(path, !isProjectPinned);
              }}
            >
              <PinThumbtackIcon pinned={isProjectPinned} />
            </button></HoverInfo>
            <SidebarProjectMenu
              isPinned={isProjectPinned}
              open={activeProjectMenu?.project === proj}
              contextMenuPosition={activeProjectMenu?.project === proj ? activeProjectMenu.position : undefined}
              onOpenChange={(next) => setActiveProjectMenu(next ? { project: proj } : null)}
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
              onEdit={() => {
                const path = nameToPath[proj];
                if (path) setEditingProject({ path, name: proj });
              }}
              onAddSection={onAddProjectSection}
              onArchive={() => {
                const path = nameToPath[proj];
                if (path) setArchiveProject({ path, name: proj });
              }}
              onRemove={() => {
                const path = nameToPath[proj];
                if (path) setRemoveProject({ path, name: proj });
              }}
            />
            <HoverInfo content={t("sidebar.newChat")}><button
              className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded"

              onClick={(e) => {
                e.stopPropagation();
                const path = nameToPath[proj];
                if (path) onSelectProject(path);
                onNewChat();
              }}
            >
              <FontAwesomeIcon icon={["far", "pen-to-square"]} className="text-[10px]" />
            </button></HoverInfo>
          </div>
        </div>
        {isExpanded && (
          <div className="flex flex-col mt-0.5 space-y-0.5">
            {projSessions.length === 0 ? (
              <div className="pl-[34px] py-1 text-[13px] text-text-secondary">{t("sidebar.noChats")}</div>
            ) : (
              projSessions.map((s) => renderSessionItem(s))
            )}
          </div>
        )}
      </div>
    );
  };

  return (
    <aside className="app-conversation-sidebar no-select" aria-label={t("sidebar.projectsAndChats")}>
      <div className="app-conversation-sidebar-header">
        <strong>DeepAgent</strong>
        <Button variant="ghost" size="icon" className="h-7 w-7 rounded-md" onClick={onCollapse}
          aria-label={t("sidebar.collapseSidebar")} title={t("sidebar.collapseSidebar")}>
          <PanelLeft className="h-4 w-4" />
        </Button>
      </div>
      <div className="px-2 pb-4">
        <Button variant="ghost" className="h-9 w-full justify-start gap-2 rounded-lg px-2.5 text-sm font-medium text-text-base" onClick={onNewChat}>
          <SquarePen className="h-4 w-4" />{t("sidebar.newChat")}
        </Button>
      </div>

      {/* Project / session list */}
      <div className="stable-scrollbar-gutter min-h-0 flex-1 overflow-y-auto px-2 space-y-5 pb-3 custom-scrollbar">
        {(pinnedProjectEntries.length > 0 || pinnedRecentSessions.length > 0) && <section aria-label={t("sidebar.pinned")}>
          <div className="sidebar-section-heading mb-1 flex items-center px-2">{t("sidebar.pinned")}</div>
          <div className="space-y-0.5">
            {pinnedProjectEntries.map(([proj, items]) => renderProjectGroup(proj, items))}
            {pinnedRecentSessions.map(session => renderSessionItem(session, true))}
          </div>
        </section>}
        <div className="flex flex-col">
          <div
            className="sidebar-section-heading flex items-center justify-between px-2 mb-1 group cursor-pointer select-none"

            onClick={toggleProjectsSection}
          >
            <span className="flex items-center gap-1">
              {t("sidebar.projects")}
              <ChevronDown
                className={cn("h-3 w-3 transition-transform", projectsCollapsed && "-rotate-90")}
              />
            </span>
            <div
              className={cn("flex items-center space-x-1 transition-opacity", isMoreMenuOpen || isNewProjectMenuOpen ? "opacity-100" : "opacity-0 group-hover:opacity-100")}
              onClick={(e) => e.stopPropagation()}
            >
              <DropdownMenu
                open={isMoreMenuOpen}
                onOpenChange={(next) => {
                  setIsMoreMenuOpen(next);
                  if (next) setIsNewProjectMenuOpen(false);
                }}
              >
                <HoverInfo content={t("sidebar.more")}><DropdownMenuTrigger asChild >
                  <button className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded">
                    <Ellipsis className="h-3 w-3" />
                  </button>
                </DropdownMenuTrigger></HoverInfo>
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
                <HoverInfo content={t("sidebar.newProject")}><DropdownMenuTrigger asChild >
                  <button className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded">
                    <FolderPlus className="h-3 w-3" />
                  </button>
                </DropdownMenuTrigger></HoverInfo>
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
          {!projectsCollapsed && (
            <div className="space-y-0.5">
              {projects.length === 0 && projectEntries.length === 0 && (
                <div className="px-2.5 py-1 text-[13px] text-text-secondary">{t("sidebar.noProjects")}</div>
              )}
              {organizeMode === "time" && projectSessionsByTime.length === 0 && (
                <div className="px-2.5 py-1 text-[13px] text-text-secondary">{t("sidebar.noChats")}</div>
              )}
              {organizeMode === "time" &&
                projectSessionsByTime.map((session) => renderSessionItem(session, true))}
              {regularProjectEntries.map(([proj, projSessions]) => renderProjectGroup(proj, projSessions))}
            </div>
          )}
        </div>

        <div className="flex flex-col">
          <div
            className="sidebar-section-heading flex items-center justify-between px-2 mb-1 group cursor-pointer select-none"

            onClick={toggleRecentSection}
          >
            <span className="flex items-center gap-1">
              {t("sidebar.recent")}
              <ChevronDown
                className={cn("h-3 w-3 transition-transform", recentCollapsed && "-rotate-90")}
              />
            </span>
            <div
              className={cn("flex items-center space-x-1 transition-opacity", isRecentMenuOpen ? "opacity-100" : "opacity-0 group-hover:opacity-100")}
              onClick={(e) => e.stopPropagation()}
            >
              <DropdownMenu open={isRecentMenuOpen} onOpenChange={setIsRecentMenuOpen}>
                <HoverInfo content={t("sidebar.more")}><DropdownMenuTrigger asChild >
                  <button className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded">
                    <Ellipsis className="h-3 w-3" />
                  </button>
                </DropdownMenuTrigger></HoverInfo>
                <DropdownMenuContent align="end" className="min-w-[11rem]">
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
              <HoverInfo content={t("sidebar.newChat")}><button
                type="button"
                className="w-5 h-5 flex items-center justify-center hover:bg-sidebar-highlight rounded"

                onClick={() => {
                  onSelectProject(null);
                  onNewChat();
                }}
              >
                <SquarePen className="h-3 w-3" />
              </button></HoverInfo>
            </div>
          </div>
          {!recentCollapsed && (
            <div className="space-y-0.5">
              {regularRecentSessions.length === 0 && (
                <div className="px-2.5 py-1 text-[13px] text-text-secondary">{t("sidebar.noChats")}</div>
              )}
              {regularRecentSessions.map((session) => renderSessionItem(session, true))}
            </div>
          )}
        </div>
        {[...preferences.sections].sort((a, b) => Number(!!b.pinned) - Number(!!a.pinned)).map((section) => {
          const members = sessions.filter((session) => preferences.session_sections[session.id] === section.id);
          return <SidebarSectionGroup key={section.id} section={section} collapsed={!!collapsedSections[section.id]} sessionCount={members.length}
            onToggle={() => setCollapsedSections((prev) => ({ ...prev, [section.id]: !prev[section.id] }))}
            onNewChat={onNewSectionChat ? () => { setCollapsedSections((prev) => ({ ...prev, [section.id]: false })); onNewSectionChat(section.id); } : undefined}
            onArchive={onArchiveSection ? () => onArchiveSection(section.id) : undefined}
            onAddSection={onAddProjectSection}>
            {members.map((session) => renderSessionItem(session, true))}
          </SidebarSectionGroup>;
        })}
      </div>

      {sessionMenu && menuSession && sessionActions && (
        <SidebarSessionMenu key={menuSession.id} session={menuSession} position={sessionMenu.position}
          open={sessionMenu.open} onOpenChange={(open) => setSessionMenu((prev) => prev ? { ...prev, open } : null)}
          options={sessionMenuOptions?.(menuSession.id) ?? {}}
          actions={sessionActions} onPin={() => onPinSession(menuSession.id, !menuSession.pinned)}
          onArchive={() => onArchiveSession(menuSession.id)}/>
      )}
      <EditProjectDialog
        open={editingProject !== null}
        project={editingProject}
        onClose={() => setEditingProject(null)}
        onSave={async (name, folder) => {
          if (!editingProject) return;
          await onEditProject(editingProject.path, name, folder);
        }}
      />

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
