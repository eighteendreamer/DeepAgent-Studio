// DTOs mirroring deepagent-app-core::dto (the kernel↔UI contract).

export interface SessionSummary {
  id: string;
  title: string | null;
  created_at: number;
  updated_at: number;
  ended: boolean;
}

export interface TimelineEntry {
  sequence: number;
  timestamp: number;
  kind: string;
  icon: string;
  label: string;
  detail: string | null;
  duration_ms: number | null;
}

export interface SessionStats {
  event_count: number;
  messages: number;
  tool_calls: number;
  tool_successes: number;
  tool_failures: number;
  total_tool_ms: number;
  tool_success_rate: number | null;
  duration_ms: number;
}

export interface SessionDetail {
  summary: SessionSummary;
  timeline: TimelineEntry[];
  stats: SessionStats;
}
