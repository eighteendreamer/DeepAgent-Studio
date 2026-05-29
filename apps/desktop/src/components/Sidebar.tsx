import type { SessionSummary } from "../types";
import { formatRelative } from "../util";

interface Props {
  sessions: SessionSummary[];
  activeId: string | null;
  onSelect: (id: string) => void;
}

export function Sidebar({ sessions, activeId, onSelect }: Props) {
  return (
    <aside className="sidebar">
      <h2>Sessions</h2>
      {sessions.length === 0 && <div className="empty">No sessions yet.</div>}
      {sessions.map((s) => (
        <div
          key={s.id}
          className={`session-item${s.id === activeId ? " active" : ""}`}
          onClick={() => onSelect(s.id)}
        >
          <div className="title">{s.title ?? "Untitled session"}</div>
          <div className="meta">
            <span>{formatRelative(s.updated_at)}</span>
            <span className={`badge ${s.ended ? "ended" : "running"}`}>
              {s.ended ? "ended" : "active"}
            </span>
          </div>
        </div>
      ))}
    </aside>
  );
}
