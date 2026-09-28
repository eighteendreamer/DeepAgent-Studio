import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { searchSessions } from "../api";
import type { Project, SessionSearchHit, SessionSummary } from "../types";
import { Panel } from "./ui/Panel";
import { ListItem } from "./ui/ListItem";

interface Props {
  isOpen: boolean;
  onClose: () => void;
  sessions: SessionSummary[];
  projects: Project[];
  onSelectSession: (id: string) => void;
}

export function SearchModal({ isOpen, onClose, sessions, projects, onSelectSession }: Props) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [searchHits, setSearchHits] = useState<SessionSearchHit[]>([]);

  useEffect(() => {
    if (!isOpen || !query.trim()) {
      setSearchHits([]);
      return;
    }
    setSearchHits([]);
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void searchSessions(query.trim(), undefined, 50)
        .then((hits) => {
          if (!cancelled) setSearchHits(hits);
        })
        .catch((error) => {
          console.error("Failed to search persisted sessions", error);
          if (!cancelled) setSearchHits([]);
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [isOpen, query]);

  const results = useMemo(() => {
    const projectNames = new Set(projects.map((project) => project.name));
    const q = query.trim().toLowerCase();
    const visibleSessions = sessions
      .filter((session) => {
        if (!session.title?.trim()) return false;
        if (session.project && !projectNames.has(session.project)) return false;
        return true;
      })
      .sort((a, b) => b.updated_at - a.updated_at);
    if (!q) {
      return visibleSessions.map((session) => ({ session, sequence: null, snippet: null }));
    }
    const titleMatches = visibleSessions
      .filter(
        (session) =>
          session.title?.toLowerCase().includes(q) || session.project?.toLowerCase().includes(q),
      )
      .map((session) => ({ session, sequence: null, snippet: null }));
    const seen = new Set(titleMatches.map((result) => `${result.session.id}:title`));
    const bodyMatches = searchHits
      .filter((hit) => !seen.has(`${hit.session.id}:${hit.sequence}`))
      .map((hit) => ({
        session: hit.session,
        sequence: hit.sequence,
        snippet: hit.snippet,
      }));
    return [...titleMatches, ...bodyMatches];
  }, [projects, query, searchHits, sessions]);

  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.ctrlKey && /^[1-9]$/.test(e.key)) {
        const index = Number(e.key) - 1;
        const result = results[index];
        if (result) {
          e.preventDefault();
          onSelectSession(result.session.id);
          onClose();
        }
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onClose, onSelectSession, results]);

  useEffect(() => {
    if (!isOpen) setQuery("");
  }, [isOpen]);

  const handleSelect = (id: string) => {
    onSelectSession(id);
    onClose();
  };

  if (!isOpen) return null;

  return (
        <div className="modal-layer fixed inset-0 z-[100] flex items-center justify-center bg-transparent">
          {/* Backdrop overlay for closing */}
          <div 
            className="absolute inset-0 bg-black/5" 
            onClick={onClose} 
          />
      
          {/* Modal */}
          <Panel menu={false} className="modal-panel relative h-[340px] w-[380px] max-h-[calc(100vh-96px)] max-w-[calc(100vw-32px)] flex flex-col overflow-hidden">
        {/* Header / Input area */}
        <div className="px-4 py-3 border-b border-transparent">
          <input 
            type="text" 
            placeholder={t("searchModal.searchChats")}
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            className="w-full text-[14px] bg-transparent outline-none text-text-base placeholder:text-text-secondary placeholder:font-normal"
          />
        </div>

        {/* List Content */}
        <div className="flex-1 overflow-y-auto px-2 pb-2">
          <div className="px-2 py-1.5 text-[11px] text-text-secondary font-medium">{t("searchModal.recentChats")}</div>
          <div className="flex flex-col space-y-0.5">
            {results.map((result, i) => (
              <ListItem
                key={`${result.session.id}:${result.sequence ?? "title"}`}
                className="px-2 py-2 rounded-lg cursor-pointer group"
                onClick={() => handleSelect(result.session.id)}
              >
                <div className="min-w-0 pr-3 flex-1">
                  <div className="text-[13px] text-text-base truncate">
                    {result.session.title}
                  </div>
                  {result.snippet && (
                    <div className="mt-0.5 text-[11px] text-text-secondary line-clamp-2">
                      {result.snippet}
                    </div>
                  )}
                </div>
                <div className="flex items-center space-x-2 flex-shrink-0">
                  <span className="text-[11px] text-text-secondary truncate max-w-[88px]">
                    {result.session.project}
                  </span>
                  {i < 9 && (
                    <span className="text-[10px] text-gray-400 bg-gray-50 border border-gray-200 rounded px-1.5 py-0.5 font-sans min-w-[38px] text-center group-hover:bg-white transition-colors">
                      Ctrl+{i + 1}
                    </span>
                  )}
                </div>
              </ListItem>
            ))}
            {results.length === 0 && (
              <div className="px-3 py-8 text-center text-[13px] text-text-secondary">
                {t("sidebar.noChats")}
              </div>
            )}
          </div>
        </div>
      </Panel>
    </div>
  );
}
