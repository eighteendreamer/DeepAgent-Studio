// API bridge to the Rust kernel via Tauri commands.
//
// When running inside Tauri, calls go through `invoke` to the Rust
// `deepagent-app-core::AppService`. When running in a plain browser (e.g.
// `vite dev` without Tauri, or the smoke build), it falls back to deterministic
// mock data so the UI is always runnable and the build never breaks.

import type {
  ApprovalRequest,
  Command,
  DiffResult,
  SessionDetail,
  SessionSummary,
} from "./types";

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

export async function getCommands(query: string): Promise<Command[]> {
  const invoke = getInvoke();
  if (invoke) return invoke<Command[]>("commands", { query });
  return filterMock(query);
}

export async function computeDiff(oldText: string, newText: string): Promise<DiffResult> {
  const invoke = getInvoke();
  if (invoke) return invoke<DiffResult>("compute_diff", { old: oldText, new: newText });
  return mockDiff(oldText, newText);
}

/** Pending approvals are pushed by the runtime; mocked here for preview. */
export async function getPendingApprovals(): Promise<ApprovalRequest[]> {
  return mockApprovals();
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

const MOCK_COMMANDS: Command[] = [
  { id: "session.new", title: "New Session", category: "Session", shortcut: "Ctrl+N" },
  { id: "session.end", title: "End Session", category: "Session", shortcut: null },
  { id: "session.refresh", title: "Refresh Sessions", category: "Session", shortcut: "Ctrl+R" },
  { id: "view.timeline", title: "Show Timeline", category: "View", shortcut: "Ctrl+1" },
  { id: "view.metrics", title: "Toggle Metrics Panel", category: "View", shortcut: "Ctrl+2" },
  { id: "view.diff", title: "Open Diff View", category: "View", shortcut: "Ctrl+D" },
  { id: "approvals.review", title: "Review Pending Approvals", category: "Approvals", shortcut: "Ctrl+Shift+A" },
  { id: "mcp.list", title: "List MCP Servers", category: "MCP", shortcut: null },
  { id: "theme.toggle", title: "Toggle Theme", category: "View", shortcut: null },
];

function isSubsequence(needle: string, haystack: string): boolean {
  let i = 0;
  for (const hc of haystack) {
    if (i >= needle.length) break;
    if (needle[i] === hc) i++;
  }
  return i >= needle.length;
}

function filterMock(query: string): Command[] {
  const q = query.trim().toLowerCase().replace(/\s+/g, "");
  if (!q) return MOCK_COMMANDS;
  return MOCK_COMMANDS.filter((c) =>
    isSubsequence(q, `${c.title} ${c.category}`.toLowerCase())
  );
}

function mockApprovals(): ApprovalRequest[] {
  return [
    {
      call_id: "call_demo_1",
      tool: "shell",
      risk: "high",
      arguments: '{\n  "cmd": "rm -rf ./build"\n}',
      reason: "high-risk tool requires explicit approval",
    },
  ];
}

// A tiny LCS diff mirroring the Rust implementation, for browser preview.
function mockDiff(oldText: string, newText: string): DiffResult {
  const a = oldText === "" ? [] : oldText.split("\n");
  const b = newText === "" ? [] : newText.split("\n");
  const n = a.length;
  const m = b.length;
  const t: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = 1; i <= n; i++)
    for (let j = 1; j <= m; j++)
      t[i][j] = a[i - 1] === b[j - 1] ? t[i - 1][j - 1] + 1 : Math.max(t[i - 1][j], t[i][j - 1]);
  const lines: DiffResult["lines"] = [];
  let added = 0;
  let removed = 0;
  let i = n;
  let j = m;
  const rev: DiffResult["lines"] = [];
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && a[i - 1] === b[j - 1]) {
      rev.push({ kind: "context", old_line: i, new_line: j, content: a[i - 1] });
      i--;
      j--;
    } else if (j > 0 && (i === 0 || t[i][j - 1] >= t[i - 1][j])) {
      rev.push({ kind: "added", old_line: null, new_line: j, content: b[j - 1] });
      added++;
      j--;
    } else {
      rev.push({ kind: "removed", old_line: i, new_line: null, content: a[i - 1] });
      removed++;
      i--;
    }
  }
  rev.reverse();
  lines.push(...rev);
  return { lines, added, removed };
}
