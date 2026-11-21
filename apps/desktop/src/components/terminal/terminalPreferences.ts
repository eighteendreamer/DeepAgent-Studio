export type CommandHistory = { command: string; scope: string; count: number; lastAt: number; exitCode: number | null; favorite: boolean };
export type QuickCommand = { id: string; name: string; command: string; group: string; description: string; color: string; pinned: boolean };
export type HighlightRule = { id: string; pattern: string; color: string; enabled: boolean };
export const defaultHighlightRules: HighlightRule[] = [
  { id: "errors", pattern: "\\b(error|failed|fatal|panic)\\b", color: "#ef4444", enabled: true },
  { id: "warnings", pattern: "\\b(warn|warning)\\b", color: "#f59e0b", enabled: true },
];
export function updateHistory(rows: CommandHistory[], result: { command: string; exitCode: number | null }, scope: string, at = Date.now()): CommandHistory[] {
  const command = result.command.trim();
  if (!command || command.length > 8192 || /(?:password|passwd|api[_-]?key|token|secret|authorization)\s*(?:=|:|\s)|--(?:password|token|secret)\b/i.test(command)) return rows;
  const existing = rows.find(row => row.command === command && row.scope === scope);
  return [{ command, scope, count: (existing?.count ?? 0) + 1, lastAt: at, exitCode: result.exitCode, favorite: existing?.favorite ?? false }, ...rows.filter(row => row !== existing)].slice(0, 500);
}
export function historyForScope(rows: CommandHistory[], scope: string) { return rows.filter(row => row.scope === scope); }
export type TerminalPreferences = { version: 1; historyEnabled: boolean; history: CommandHistory[]; quick: QuickCommand[]; highlights: HighlightRule[] };
export function readTerminalPreferences(storage: Pick<Storage, "getItem">): TerminalPreferences {
  const fallback: TerminalPreferences = { version: 1, historyEnabled: false, history: [], quick: [], highlights: defaultHighlightRules };
  try {
    const raw = storage.getItem("deepagent.terminal.preferences.v1"); if (!raw || raw.length > 1024 * 1024) return fallback;
    const value = JSON.parse(raw); if (value.version !== 1) return fallback;
    return { version: 1, historyEnabled: value.historyEnabled === true,
      history: Array.isArray(value.history) ? value.history.filter((v: CommandHistory) => typeof v.command === "string" && v.command.length <= 8192 && typeof v.scope === "string" && Number.isFinite(v.count) && Number.isFinite(v.lastAt) && (v.exitCode === null || Number.isFinite(v.exitCode))).slice(0, 500) : [],
      quick: Array.isArray(value.quick) ? value.quick.filter((v: QuickCommand) => typeof v.id === "string" && typeof v.name === "string" && typeof v.command === "string" && v.command.length <= 8192 && typeof v.group === "string" && typeof v.description === "string" && /^#[0-9a-f]{6}$/i.test(v.color)).slice(0, 100) : [],
      highlights: Array.isArray(value.highlights) ? value.highlights.filter((v: HighlightRule) => typeof v.id === "string" && typeof v.pattern === "string" && v.pattern.length <= 128 && /^#[0-9a-f]{6}$/i.test(v.color)).slice(0, 8) : defaultHighlightRules };
  } catch { return fallback; }
}
