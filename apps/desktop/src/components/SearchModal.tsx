import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Project, SessionSummary } from "../types";

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

  const results = useMemo(() => {
    const projectNames = new Set(projects.map((project) => project.name));
    const q = query.trim().toLowerCase();
    return sessions
      .filter((session) => {
        if (!session.title?.trim()) return false;
        if (!session.project || !projectNames.has(session.project)) return false;
        if (!q) return true;
        return (
          session.title.toLowerCase().includes(q) ||
          session.project.toLowerCase().includes(q)
        );
      })
      .sort((a, b) => b.updated_at - a.updated_at);
  }, [projects, query, sessions]);

  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.ctrlKey && /^[1-9]$/.test(e.key)) {
        const index = Number(e.key) - 1;
        const session = results[index];
        if (session) {
          e.preventDefault();
          onSelectSession(session.id);
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
          <div className="modal-panel relative w-[380px] h-[340px] max-w-[calc(100vw-32px)] max-h-[calc(100vh-96px)] bg-white rounded-xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] border border-border-theme flex flex-col overflow-hidden">
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
            {results.map((item, i) => (
              <div 
                key={item.id}
                className="flex items-center justify-between px-2 py-2 rounded-lg cursor-pointer hover:bg-gray-100 group transition-colors"
                onClick={() => handleSelect(item.id)}
              >
                <div className="text-[13px] text-text-base truncate pr-3 flex-1">
                  {item.title}
                </div>
                <div className="flex items-center space-x-2 flex-shrink-0">
                  <span className="text-[11px] text-text-secondary truncate max-w-[88px]">
                    {item.project}
                  </span>
                  {i < 9 && (
                    <span className="text-[10px] text-gray-400 bg-gray-50 border border-gray-200 rounded px-1.5 py-0.5 font-sans min-w-[38px] text-center group-hover:bg-white transition-colors">
                      Ctrl+{i + 1}
                    </span>
                  )}
                </div>
              </div>
            ))}
            {results.length === 0 && (
              <div className="px-3 py-8 text-center text-[13px] text-text-secondary">
                {t("sidebar.noChats")}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
