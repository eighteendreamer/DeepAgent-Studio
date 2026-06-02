import { useState, useRef, useEffect, useMemo } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
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
  onRemoveProject: (path: string) => void;
  onOpenSearch: () => void;
  onOpenSkills: () => void;
  onOpenKnowledge: () => void;
  onOpenAutomation: () => void;
  onOpenSettings: () => void;
  onLogout: () => void;
  /** Session ids with currently-running agent runs (show spinners). */
  runningSessionIds?: Set<string>;
}

function NavButton({ icon, label, onClick }: { icon: IconProp; label: string; onClick?: () => void }) {
  return (
    <button 
      className="w-full flex items-center px-2.5 py-1.5 rounded-md text-sm text-text-base hover:bg-black/5 transition-colors"
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

export function Sidebar({ sessions, projects, activeProjectPath, activeId, onSelect, onSelectProject, onNewChat, onAddProject, onRemoveProject, onOpenSearch, onOpenSkills, onOpenKnowledge, onOpenAutomation, onOpenSettings, onLogout, runningSessionIds }: Props) {
  const { t } = useTranslation();
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isMoreMenuOpen, setIsMoreMenuOpen] = useState(false);
  const [isNewProjectMenuOpen, setIsNewProjectMenuOpen] = useState(false);
  const [activeProjectMenu, setActiveProjectMenu] = useState<string | null>(null);
  const [pinnedSessionIds, setPinnedSessionIds] = useState<Set<string>>(new Set());
  
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

  // Group sessions by their project (display name). Seed the map from the real
  // projects list so projects with no sessions yet still appear.
  const groupedSessions = useMemo(() => {
    const groups: Record<string, SessionSummary[]> = {};
    for (const p of projects) {
      groups[p.name] = [];
    }
    for (const s of sessions) {
      const proj = s.project || t("sidebar.noProjects");
      if (!groups[proj]) groups[proj] = [];
      groups[proj].push(s);
    }
    return groups;
  }, [sessions, projects, t]);

  // Map a project display name back to its path (for selecting the active one).
  const nameToPath = useMemo(() => {
    const m: Record<string, string> = {};
    for (const p of projects) m[p.name] = p.path;
    return m;
  }, [projects]);

  const pinnedSessions = useMemo(() => {
    return sessions.filter((s) => pinnedSessionIds.has(s.id));
  }, [sessions, pinnedSessionIds]);

  const togglePin = (id: string) => {
    setPinnedSessionIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const [expandedProjects, setExpandedProjects] = useState<Record<string, boolean>>({});

  // Default every project to expanded as projects load.
  useEffect(() => {
    setExpandedProjects((prev) => {
      const next = { ...prev };
      for (const p of projects) {
        if (!(p.name in next)) next[p.name] = true;
      }
      return next;
    });
  }, [projects]);

  const toggleProject = (proj: string) => {
    setExpandedProjects((prev) => ({ ...prev, [proj]: !prev[proj] }));
  };

  const toggleExpandAll = () => {
    const allExpanded = Object.keys(groupedSessions).every((proj) => expandedProjects[proj]);
    if (allExpanded) {
      setExpandedProjects({});
    } else {
      const all: Record<string, boolean> = {};
      Object.keys(groupedSessions).forEach((proj) => {
        all[proj] = true;
      });
      setExpandedProjects(all);
    }
  };

  const renderSessionItem = (s: SessionSummary, isPinnedSection: boolean = false) => {
    const active = s.id === activeId;
    const isPinned = pinnedSessionIds.has(s.id);
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
        <span className="truncate flex-1 pr-2">{s.title}</span>

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
                  onClick={(e) => { e.stopPropagation(); togglePin(s.id); }}
                  className="w-5 h-5 flex items-center justify-center hover:bg-black/10 rounded text-text-secondary"
                  title={isPinned ? t("sidebar.unpin") : t("sidebar.pin")}
                >
                  <FontAwesomeIcon icon={["fas", "thumbtack"]} className="text-[10px]" />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); }}
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

  return (
    <aside className="w-[240px] flex flex-col bg-sidebar-bg h-full no-select flex-shrink-0 pb-2">
      {/* Top actions */}
      <div className="px-3 py-2 space-y-0.5">
        <button
          className="w-full flex items-center px-2.5 py-1.5 rounded-md text-sm text-text-base hover:bg-black/5 transition-colors"
          onClick={onNewChat}
        >
          <FontAwesomeIcon icon={["far", "pen-to-square"]} className="w-5 text-left text-text-secondary" />
          <span className="ml-0.5">{t("sidebar.newChat")}</span>
        </button>
        <NavButton icon={["fas", "magnifying-glass"]} label={t("sidebar.search")} onClick={onOpenSearch} />
        <NavButton icon={["fas", "layer-group"]} label={t("sidebar.skills")} onClick={onOpenSkills} />
        <NavButton icon={["fas", "book"]} label={t("sidebar.knowledge")} onClick={onOpenKnowledge} />
        <NavButton icon={["fas", "puzzle-piece"]} label={t("sidebar.plugins")} />
        <NavButton icon={["far", "clock"]} label={t("sidebar.automation")} onClick={onOpenAutomation} />
      </div>

      {/* Project / session list */}
      <div className="flex-1 overflow-y-auto px-2 mt-4 space-y-3 pb-2 custom-scrollbar">
        {/* Pinned sessions */}
        {pinnedSessions.length > 0 && (
          <div className="flex flex-col">
            <div className="px-2 mb-1 text-[12px] text-text-secondary">{t("sidebar.pinned")}</div>
            <div className="space-y-0.5">
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
                  onClick={() => { setIsMoreMenuOpen(!isMoreMenuOpen); setIsNewProjectMenuOpen(false); }}
                >
                  <FontAwesomeIcon icon={["fas", "ellipsis"]} className="text-[10px]" />
                </button>
                {isMoreMenuOpen && (
                  <div className="absolute top-full right-0 mt-1 w-48 bg-white border border-border-theme rounded-xl shadow-[0_4px_24px_rgb(0,0,0,0.12)] py-1 z-50 flex flex-col">
                    <button className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                      <FontAwesomeIcon icon={["fas", "box-archive"]} className="text-text-secondary mr-2.5 w-4" />
                      {t("sidebar.archiveAll")}
                    </button>
                    <div className="my-1 border-t border-border-theme"></div>
                    <button className="flex items-center justify-between px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                      <div className="flex items-center">
                        <FontAwesomeIcon icon={["far", "folder"]} className="text-text-secondary mr-2.5 w-4" />
                        {t("sidebar.organizeSidebar")}
                      </div>
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-text-secondary text-[10px]" />
                    </button>
                    <button className="flex items-center justify-between px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                      <div className="flex items-center">
                        <FontAwesomeIcon icon={["far", "clock"]} className="text-text-secondary mr-2.5 w-4" />
                        {t("sidebar.sortCriteria")}
                      </div>
                      <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-text-secondary text-[10px]" />
                    </button>
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
                      className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left"
                      onClick={() => { setIsNewProjectMenuOpen(false); onAddProject(); }}
                    >
                      <FontAwesomeIcon icon={["fas", "folder-plus"]} className="text-text-secondary mr-2.5 w-4" />
                      {t("sidebar.newBlankProject")}
                    </button>
                    <button
                      className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left"
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
            {Object.keys(groupedSessions).length === 0 && (
              <div className="px-2.5 py-1 text-[13px] text-text-secondary">{t("sidebar.noProjects")}</div>
            )}
          {Object.entries(groupedSessions).map(([proj, projSessions]) => {
            const isExpanded = expandedProjects[proj];
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
                          <button className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                            <FontAwesomeIcon icon={["fas", "thumbtack"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.pinProject")}
                          </button>
                          <button className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                            <FontAwesomeIcon icon={["far", "folder"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.openInExplorer")}
                          </button>
                          <button className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                            <FontAwesomeIcon icon={["fas", "pen"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.renameProject")}
                          </button>
                          <div className="my-1 border-t border-border-theme"></div>
                          <button className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left">
                            <FontAwesomeIcon icon={["fas", "box-archive"]} className="text-text-secondary mr-2.5 w-4" />
                            {t("sidebar.archive")}
                          </button>
                          <button
                            className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left"
                            onClick={() => {
                              setActiveProjectMenu(null);
                              const path = nameToPath[proj];
                              if (path) onRemoveProject(path);
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
                    {projSessions.length === 0 || (projSessions.length === 1 && !projSessions[0].title) ? (
                      <div className="pl-8 py-1 text-[12px] text-gray-400">{t("sidebar.noChats")}</div>
                    ) : (
                      projSessions.map((s) => {
                        if (!s.title) return null;
                        return renderSessionItem(s);
                      })
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
              className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left"
              onClick={() => {
                setIsSettingsOpen(false);
                onOpenSettings();
              }}
            >
              <FontAwesomeIcon icon={["fas", "gear"]} className="text-text-secondary mr-2.5 w-4" />
              {t("sidebar.settings")}
            </button>
            <button 
              className="flex items-center px-3 py-2 text-[13px] text-text-base hover:bg-gray-50 transition-colors w-full text-left"
              onClick={() => { setIsSettingsOpen(false); onLogout(); }}
            >
              <FontAwesomeIcon icon={["fas", "arrow-right-from-bracket"]} className="text-text-secondary mr-2.5 w-4" />
              {t("sidebar.logout")}
            </button>
          </div>
        )}
      </div>
    </aside>
  );
}
