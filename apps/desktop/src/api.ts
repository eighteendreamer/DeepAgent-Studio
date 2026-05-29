// API bridge to the Rust kernel via Tauri commands.
//
// When running inside Tauri, calls go through `invoke` to the Rust
// `deepagent-app-core::AppService`. When running in a plain browser (e.g.
// `vite dev` without Tauri, or the smoke build), it falls back to deterministic
// mock data so the UI is always runnable and the build never breaks.

import type { SessionDetail, SessionSummary } from "./types";

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

function getInvoke(): InvokeFn | null {
  // Tauri injects __TAURI_INTERNALS__ / the api module at runtime.
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
  if (typeof window !== "undefined" && w.__TAURI_INTERNALS__) {
    // Lazy import so the browser build does not hard-require the module.
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return (async (cmd: string, args?: Record<string, unknown>) => {
      const mod = await import("@tauri-apps/api/core");
      return mod.invoke(cmd, args);
    }) as InvokeFn;
  }
  return null;
}

export async function listSessions(): Promise<SessionSummary[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<SessionSummary[]>("list_sessions");
  return mockSessions();
}

export async function getSessionDetail(id: string): Promise<SessionDetail> {
  const invoke = getInvoke();
  if (invoke) return invoke<SessionDetail>("session_detail", { sessionId: id });
  return mockDetail(id);
}

export function isTauri(): boolean {
  return getInvoke() !== null;
}

// ---- Mock data (browser/dev fallback) ------------------------------------

function mockSessions(): SessionSummary[] {
  const now = Date.now();
  return [
    {
      id: "ses_demo000000000000000000000000001",
      title: "Implement payment retry",
      created_at: now - 3_600_000,
      updated_at: now - 120_000,
      ended: false,
    },
    {
      id: "ses_demo000000000000000000000000002",
      title: "Refactor context pipeline",
      created_at: now - 7_200_000,
      updated_at: now - 5_400_000,
      ended: true,
    },
  ];
}

function mockDetail(id: string): SessionDetail {
  const base = Date.now() - 600_000;
  return {
    summary: {
      id,
      title: "Implement payment retry",
      created_at: base,
      updated_at: base + 480_000,
      ended: false,
    },
    timeline: [
      { sequence: 0, timestamp: base, kind: "session", icon: "🟢", label: "Session started", detail: "Implement payment retry", duration_ms: null },
      { sequence: 1, timestamp: base + 1000, kind: "task", icon: "📋", label: "Task created: add retry with backoff", detail: null, duration_ms: null },
      { sequence: 2, timestamp: base + 2000, kind: "task", icon: "🔄", label: "Task Queued → Running", detail: null, duration_ms: null },
      { sequence: 3, timestamp: base + 5000, kind: "tool", icon: "🔧", label: "Tool requested: read_file", detail: '{"path":"payment/retry.rs"}', duration_ms: null },
      { sequence: 4, timestamp: base + 5200, kind: "tool", icon: "✅", label: "Tool completed", detail: "read 1.2 KB", duration_ms: 180 },
      { sequence: 5, timestamp: base + 9000, kind: "tool", icon: "🔧", label: "Tool requested: write_file", detail: '{"path":"payment/retry.rs"}', duration_ms: null },
      { sequence: 6, timestamp: base + 9400, kind: "tool", icon: "✅", label: "Tool completed", detail: "wrote 1.5 KB", duration_ms: 240 },
      { sequence: 7, timestamp: base + 12000, kind: "note", icon: "📝", label: "Note", detail: "verification passed", duration_ms: null },
      { sequence: 8, timestamp: base + 12500, kind: "message", icon: "💬", label: "Assistant message", detail: "Added exponential backoff with jitter to the retry path.", duration_ms: null },
    ],
    stats: {
      event_count: 9,
      messages: 1,
      tool_calls: 2,
      tool_successes: 2,
      tool_failures: 0,
      total_tool_ms: 420,
      tool_success_rate: 1,
      duration_ms: 12500,
    },
  };
}
