import type { SessionStats } from "../types";
import { formatDuration } from "../util";

interface Props {
  stats: SessionStats | null;
}

export function Inspector({ stats }: Props) {
  return (
    <aside className="inspector">
      <h2>Metrics</h2>
      {!stats && <div className="empty">No session selected.</div>}
      {stats && (
        <div className="metric-grid">
          <Metric value={String(stats.event_count)} label="Events" />
          <Metric value={String(stats.messages)} label="Messages" />
          <Metric value={String(stats.tool_calls)} label="Tool calls" />
          <Metric
            value={
              stats.tool_success_rate == null
                ? "—"
                : `${Math.round(stats.tool_success_rate * 100)}%`
            }
            label="Tool success"
            tone={
              stats.tool_failures > 0
                ? "bad"
                : stats.tool_successes > 0
                ? "good"
                : undefined
            }
          />
          <Metric value={formatDuration(stats.total_tool_ms)} label="Tool time" />
          <Metric value={formatDuration(stats.duration_ms)} label="Duration" />
        </div>
      )}
    </aside>
  );
}

function Metric({ value, label, tone }: { value: string; label: string; tone?: "good" | "bad" }) {
  return (
    <div className="metric">
      <div className={`value${tone ? ` ${tone}` : ""}`}>{value}</div>
      <div className="label">{label}</div>
    </div>
  );
}
