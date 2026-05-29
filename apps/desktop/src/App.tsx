import { useCallback, useEffect, useState } from "react";
import { getPendingApprovals, getSessionDetail, isTauri, listSessions } from "./api";
import type { ApprovalRequest, Command, SessionDetail, SessionSummary } from "./types";
import { Sidebar } from "./components/Sidebar";
import { Timeline } from "./components/Timeline";
import { Inspector } from "./components/Inspector";
import { CommandPalette } from "./components/CommandPalette";
import { ApprovalDialog } from "./components/ApprovalDialog";
import { DiffView } from "./components/DiffView";

export function App() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Interactive overlays.
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [diffOpen, setDiffOpen] = useState(false);
  const [approval, setApproval] = useState<ApprovalRequest | null>(null);
  const [showMetrics, setShowMetrics] = useState(true);

  const refresh = useCallback(() => {
    listSessions()
      .then((s) => {
        setSessions(s);
        if (s.length > 0 && !activeId) setActiveId(s[0].id);
      })
      .catch((e) => setError(String(e)));
  }, [activeId]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    if (!activeId) {
      setDetail(null);
      return;
    }
    getSessionDetail(activeId)
      .then(setDetail)
      .catch((e) => setError(String(e)));
  }, [activeId]);

  // Global keyboard shortcuts (Codex-style).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPaletteOpen(true);
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "d") {
        e.preventDefault();
        setDiffOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const runCommand = useCallback(
    (cmd: Command) => {
      switch (cmd.id) {
        case "session.refresh":
          refresh();
          break;
        case "view.diff":
          setDiffOpen(true);
          break;
        case "view.metrics":
          setShowMetrics((m) => !m);
          break;
        case "approvals.review":
          getPendingApprovals().then((a) => setApproval(a[0] ?? null));
          break;
        default:
          // Other commands are no-ops in this read-only preview build.
          break;
      }
    },
    [refresh]
  );

  return (
    <div className="app">
      <header className="topbar">
        <span className="logo">
          DeepAgent<span className="dot">.</span>Studio
        </span>
        <span className="spacer" />
        <button className="topbar-btn" onClick={() => setPaletteOpen(true)}>
          ⌘K Commands
        </button>
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

      {showMetrics ? <Inspector stats={detail?.stats ?? null} /> : <div />}

      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} onRun={runCommand} />
      <DiffView open={diffOpen} onClose={() => setDiffOpen(false)} />
      <ApprovalDialog
        request={approval}
        onApprove={() => setApproval(null)}
        onReject={() => setApproval(null)}
      />
    </div>
  );
}
