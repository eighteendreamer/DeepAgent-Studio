import type { TimelineEntry } from "../types";
import { formatDuration, formatTime } from "../util";

interface Props {
  entries: TimelineEntry[];
}

export function Timeline({ entries }: Props) {
  if (entries.length === 0) {
    return <div className="empty">No timeline entries.</div>;
  }
  return (
    <div className="timeline">
      {entries.map((e) => (
        <div className="tl-entry" key={e.sequence}>
          <span className="icon">{e.icon}</span>
          <span className="stamp">{formatTime(e.timestamp)}</span>
          <div className="label">
            {e.label}
            {e.duration_ms != null && <span className="dur">{formatDuration(e.duration_ms)}</span>}
          </div>
          {e.detail && <div className="detail">{e.detail}</div>}
        </div>
      ))}
    </div>
  );
}
