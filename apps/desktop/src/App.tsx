import { useEffect, useState } from "react";
import { getSessionDetail, isTauri, listSessions } from "./api";
import type { SessionDetail, SessionSummary } from "./types";
import { Sidebar } from "./components/Sidebar";
import { Timeline } from "./components/Timeline";
import { Inspector } from "./components/Inspector";

export function App() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Load session list on mount.
  useEffect(() => {
    listSessions()
      .then((s) => {
        setSessions(s);
        if (s.length > 0) setActiveId(s[0].id);
      })
      .catch((e) => setError(String(e)));
  }, []);

  // Load detail when the active session changes.
  useEffect(() => {
    if (!activeId) {
      setDetail(null);
      return;
    }
    getSessionDetail(activeId)
      .then(setDetail)
      .catch((e) => setError(String(e)));
  }, [activeId]);

  return (
    <div className="app">
      <header className="topbar">
        <span className="logo">
          DeepAgent<span className="dot">.</span>Studio
        </span>
        <span className="spacer" />
        <span className="env-pill">{isTauri() ? "tauri" : "preview"}</span>
      </header>

      <Sidebar sessions={sessions} activeId={activeId} onSelect={setActiveId} />

      <main className="main">
        {error && <div className="empty">⚠ {error}</div>}
        {!error && !detail && <div className="empty">Select a session to view its timeline.</div>}
        {detail && (
          <>
            <div className="session-title">{detail.summary.title ?? "Untitled session"}</div>
            <div className="session-sub">{detail.summary.id}</div>
            <Timeline entries={detail.timeline} />
          </>
        )}
      </main>

      <Inspector stats={detail?.stats ?? null} />
    </div>
  );
}
