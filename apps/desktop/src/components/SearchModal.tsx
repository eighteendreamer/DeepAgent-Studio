import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { searchSessions } from "../api";
import type { Project, SessionSearchReport, SessionSummary } from "../types";
import { Button } from "./shadcn/button";
import { Input } from "./shadcn/input";
import { GlobalModal } from "./ui/GlobalModal";
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
  const [report, setReport] = useState<SessionSearchReport | null>(null);
  const [searchError, setSearchError] = useState(false);
  const [loading, setLoading] = useState(false);
  const [retry, setRetry] = useState(0);

  useEffect(() => {
    if (!isOpen || !query.trim()) {
      setReport(null);
      setSearchError(false);
      setLoading(false);
      return;
    }
    setReport(null);
    setSearchError(false);
    setLoading(true);
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void searchSessions(query.trim(), undefined, 50)
        .then((result) => {
          if (!cancelled) setReport(result);
        })
        .catch(() => {
          // Parser/SQLite errors may contain paths or transcript excerpts.
          // Do not dump them into the browser console or user-facing status.
          if (!cancelled) setSearchError(true);
        })
        .finally(() => {
          if (!cancelled) setLoading(false);
        });
    }, 200);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [isOpen, query, retry]);

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
    const bodyMatches = (report?.results ?? [])
      .filter((hit) => !seen.has(`${hit.session.id}:${hit.sequence}`))
      .map((hit) => ({
        session: hit.session,
        sequence: hit.sequence,
        snippet: hit.snippet,
      }));
    return [...titleMatches, ...bodyMatches];
  }, [projects, query, report, sessions]);

  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (e: KeyboardEvent) => {
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
        <GlobalModal open={isOpen} onClose={onClose} zIndexClass="z-[100]" backdropClassName="!bg-black/5"
          panelClassName="h-[340px] w-[380px] max-h-[calc(100vh-96px)] max-w-[calc(100vw-32px)] flex flex-col overflow-hidden">
        {/* Header / Input area */}
        <div className="px-4 py-3 border-b border-transparent">
          <Input
            type="text" 
            placeholder={t("searchModal.searchChats")}
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            aria-label={t("searchModal.searchChats")}
            className="h-auto w-full rounded-none border-0 p-0 text-[14px] bg-transparent shadow-none focus:ring-0 text-text-base placeholder:text-text-secondary placeholder:font-normal"
          />
        </div>

        {/* List Content */}
        <div className="flex-1 overflow-y-auto px-2 pb-2">
          {(loading || searchError || report?.complete === false) && (
            <div role="status" aria-live="polite" className="px-2 py-1.5 text-[12px] text-text-secondary">
              {loading ? t("searchModal.searching", "正在搜索正文…")
                : searchError ? t("searchModal.failed", "正文搜索失败；标题结果仍可用，请重试。")
                : t("searchModal.incomplete", {
                    count: report?.unavailable.length ?? 0,
                    defaultValue: "正文结果不完整：有 {{count}} 个会话暂不可搜索，请稍后重试。",
                  })}
              {!loading && (
                <Button variant="ghost" size="sm" className="ml-1 h-6 px-1.5"
                  onClick={() => setRetry((value) => value + 1)}>
                  {t("searchModal.retry", "重试")}
                </Button>
              )}
            </div>
          )}
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
            {results.length === 0 && !loading && !searchError && report?.complete !== false && (
              <div className="px-3 py-8 text-center text-[13px] text-text-secondary">
                {t("sidebar.noChats")}
              </div>
            )}
          </div>
        </div>
    </GlobalModal>
  );
}
