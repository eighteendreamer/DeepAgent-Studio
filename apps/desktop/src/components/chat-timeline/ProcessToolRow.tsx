import { useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { ToolCall } from "../../types";
import { DiffText } from "../git/GitDiffViewer";
import { formatMs } from "./format";

type ParsedToolPayload = {
  args: Record<string, unknown> | null;
  output: Record<string, unknown> | null;
  detailObject: Record<string, unknown> | null;
};

type ToolSummary = {
  inlineMeta: string[];
  monoText: string;
  detail: string;
  matches: string[];
  isDiff: boolean;
  isError: boolean;
};

type ToolIcon =
  | "folder"
  | "terminal"
  | "file"
  | "magnifying-glass"
  | "wrench"
  | "code"
  | "code-branch"
  | "list-check"
  | "robot"
  | "globe";

function parseJsonObject(value?: string): Record<string, unknown> | null {
  if (!value?.trim()) return null;
  try {
    const parsed = JSON.parse(value);
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : null;
  } catch {
    return null;
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function readString(record: Record<string, unknown> | null | undefined, key: string): string {
  const value = record?.[key];
  return typeof value === "string" ? value : "";
}

function readNumber(record: Record<string, unknown> | null | undefined, key: string): number | null {
  const value = record?.[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readStringArray(record: Record<string, unknown> | null, key: string): string[] {
  const value = record?.[key];
  if (!Array.isArray(value)) return [];
  return value.filter((item): item is string => typeof item === "string" && item.trim().length > 0);
}

function compactText(value: string, max = 120): string {
  const oneLine = value.replace(/\s+/g, " ").trim();
  if (oneLine.length <= max) return oneLine;
  return `${oneLine.slice(0, max - 1).trimEnd()}…`;
}

function prettyJson(value: unknown): string {
  if (value == null) return "";
  if (typeof value === "string") {
    const parsed = parseJsonObject(value);
    return parsed ? JSON.stringify(parsed, null, 2) : value;
  }
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function parsePayload(tool: ToolCall): ParsedToolPayload {
  return {
    args: parseJsonObject(tool.args),
    output: asRecord(tool.output),
    detailObject: parseJsonObject(tool.detail),
  };
}

function stripToolName(summary: string, name: string): string {
  const trimmed = summary.trim();
  const prefix = `${name} `;
  if (trimmed.toLowerCase().startsWith(prefix.toLowerCase())) {
    return trimmed.slice(prefix.length).trim();
  }
  return trimmed;
}

function toolIcon(name: string, kind?: string): ToolIcon {
  if (kind === "command_execution") return "terminal";
  if (kind === "file_change") return "code";
  if (kind === "file_read") return name.toLowerCase().includes("list") ? "folder" : "file";
  if (kind === "search") return "magnifying-glass";
  if (kind === "git") return "code-branch";
  if (kind === "planning") return "list-check";
  if (kind === "agent") return "robot";

  const normalized = name.toLowerCase();
  if (normalized.includes("glob") || normalized.includes("list") || normalized.includes("ls")) return "folder";
  if (normalized.includes("bash") || normalized.includes("shell") || normalized.includes("exec")) return "terminal";
  if (normalized.includes("fetch") || normalized.includes("web")) return "globe";
  if (normalized.includes("read") || normalized.includes("file")) return "file";
  if (normalized.includes("grep") || normalized.includes("search") || normalized.includes("find")) return "magnifying-glass";
  if (normalized.includes("edit") || normalized.includes("write") || normalized.includes("patch")) return "code";
  return "wrench";
}

function statusMeta(tool: ToolCall): { label: string; className: string; dotClassName: string } {
  if (tool.status === "running") {
    return { label: "执行中", className: "text-blue-600", dotClassName: "bg-blue-500" };
  }
  if (tool.status === "ok") {
    return { label: "完成", className: "text-green-600", dotClassName: "bg-green-500" };
  }
  if (tool.status === "blocked") {
    return { label: "已阻止", className: "text-orange-600", dotClassName: "bg-orange-500" };
  }
  return { label: "失败", className: "text-red-600", dotClassName: "bg-red-500" };
}

function looksLikeUnifiedDiff(text: string): boolean {
  return /^diff --git /m.test(text) || (/^@@\s/m.test(text) && (/^\+/m.test(text) || /^-/m.test(text)));
}

function summarizeTool(tool: ToolCall, payload: ParsedToolPayload): ToolSummary {
  const name = tool.name.toLowerCase();
  const data = payload.output ?? payload.detailObject;
  const meta = tool.meta ?? null;
  const matches = readStringArray(data, "matches");
  const matchesCount = readNumber(meta, "matches_count");
  const provider = readString(meta, "provider");
  const pattern = readString(meta, "pattern") || readString(payload.args, "pattern") || readString(data, "pattern");
  const path =
    tool.filePath ||
    readString(meta, "path") ||
    readString(payload.args, "path") ||
    readString(payload.args, "file_path") ||
    readString(payload.args, "file") ||
    readString(data, "path");
  const command = readString(meta, "command") || readString(payload.args, "command") || readString(payload.args, "cmd");
  const query = readString(meta, "query") || readString(payload.args, "query") || readString(payload.args, "text");
  const inlineMeta: string[] = [];

  if (matches.length > 0) inlineMeta.push(`${matches.length} matches`);
  else if (typeof matchesCount === "number") inlineMeta.push(`${matchesCount} matches`);
  if (provider) inlineMeta.push(provider);

  let monoText = "";
  if (matches.length > 0) {
    monoText = pattern || matches.slice(0, 2).join("  ");
  } else if (name.includes("glob") && data && Array.isArray(data.matches) && data.matches.length === 0) {
    monoText = pattern || "no matches";
  } else if (path) {
    monoText = path;
  } else if (command) {
    monoText = command;
  } else if (query) {
    monoText = query;
  } else if (tool.summary) {
    monoText = stripToolName(tool.summary, tool.name);
  } else if (tool.detail) {
    monoText = tool.detail;
  }

  const detail =
    matches.length > 0
      ? matches.join("\n")
      : prettyJson(tool.output ?? payload.detailObject ?? tool.detail ?? payload.args ?? "");

  return {
    inlineMeta: Array.from(new Set(inlineMeta)),
    monoText: compactText(monoText, 140),
    detail,
    matches,
    isDiff: looksLikeUnifiedDiff(detail),
    isError: tool.status === "error" || tool.status === "blocked",
  };
}

export function ProcessToolRow({ tool }: { tool: ToolCall }) {
  const [open, setOpen] = useState(false);
  const payload = useMemo(() => parsePayload(tool), [tool]);
  const summary = useMemo(() => summarizeTool(tool, payload), [tool, payload]);
  const status = statusMeta(tool);
  const canOpen = Boolean(summary.detail.trim());
  const icon = toolIcon(tool.name, tool.toolKind);
  const running = tool.status === "running";

  return (
    <div className="min-w-0">
      <button
        type="button"
        onClick={() => canOpen && tool.status !== "running" && setOpen((value) => !value)}
        className={`group/tool flex w-full min-w-0 items-center gap-2 rounded-md px-1 py-0.5 text-left text-[13.5px] leading-6 transition ${
          canOpen ? "cursor-pointer hover:bg-gray-50/70" : "cursor-default"
        }`}
      >
        <span className="flex h-5 w-5 shrink-0 items-center justify-center text-text-secondary">
          {running ? (
            <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin text-[13px] text-primary" />
          ) : (
            <FontAwesomeIcon icon={["fas", icon]} className="text-[13px]" />
          )}
        </span>
        <span className="flex min-w-0 flex-1 items-baseline gap-2">
          <span className="shrink-0 font-semibold text-text-base">{tool.name}</span>
          <span className={`inline-flex shrink-0 items-center gap-1 text-[12px] font-medium ${status.className}`}>
            <span className={`h-1.5 w-1.5 rounded-full ${status.dotClassName}`} />
            {status.label}
          </span>
          {typeof tool.durationMs === "number" && (
            <span className="shrink-0 text-[12px] tabular-nums text-text-secondary">{formatMs(tool.durationMs)}</span>
          )}
          {summary.inlineMeta.map((item) => (
            <span key={item} className="shrink-0 text-[12px] text-text-secondary">
              {item}
            </span>
          ))}
          {summary.monoText && (
            <code className="min-w-0 truncate bg-transparent p-0 font-mono text-[12.5px] text-text-secondary" title={summary.monoText}>
              {summary.monoText}
            </code>
          )}
        </span>
        {canOpen && tool.status !== "running" && (
          <FontAwesomeIcon
            icon={["fas", open ? "chevron-down" : "chevron-right"]}
            className="shrink-0 text-[10px] text-text-secondary opacity-45 transition group-hover/tool:opacity-75"
          />
        )}
      </button>
      {open && (
        <div className="ml-7 mt-1 overflow-hidden rounded-lg bg-gray-50/75">
          {summary.isDiff ? (
            <div className="max-h-80 overflow-auto">
              <DiffText text={summary.detail} />
            </div>
          ) : summary.matches.length > 0 ? (
            <div className="max-h-64 overflow-auto px-3 py-2 font-mono text-[12px] leading-5 text-text-base">
              {summary.matches.map((match) => (
                <div key={match} className="truncate" title={match}>
                  {match}
                </div>
              ))}
            </div>
          ) : summary.isError ? (
            <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words bg-orange-50/80 px-3 py-2.5 font-mono text-[12px] leading-5 text-orange-900">
              {summary.detail}
            </pre>
          ) : (
            <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words px-3 py-2.5 font-mono text-[12px] leading-5 text-text-base">
              {summary.detail}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}
