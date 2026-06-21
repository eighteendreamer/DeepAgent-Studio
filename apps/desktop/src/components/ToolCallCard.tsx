import { useState } from "react";
import type { MouseEvent } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { ToolCall } from "../types";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";

type WebSearchAttemptView = {
  provider: string;
  ok: boolean;
  count?: number;
  error?: string;
};

type WebSearchView = {
  provider: string;
  count: number;
  query?: string;
  error?: string;
  attempts: WebSearchAttemptView[];
};

type OfficeToolView = {
  kind: "docx" | "xlsx" | "read";
  title: string;
  path?: string;
  fileName?: string;
  error?: string;
  summary: string;
  stats: Array<{ label: string; value: string }>;
};

type SkillToolView = {
  id: string;
  name: string;
  body?: string;
  baseDir?: string;
  resources: string[];
  error?: string;
  summary: string;
  stats: Array<{ label: string; value: string }>;
};

/** Map a tool name to a representative icon. */
function iconFor(name: string): IconProp {
  if (name.startsWith("code_map")) return ["fas", "share-nodes"];
  if (name === "skill") return ["fas", "book"];
  if (name.includes("search")) return ["fas", "magnifying-glass"];
  if (name.includes("fetch") || name.includes("web")) return ["fas", "globe"];
  if (name === "bash" || name.includes("terminal")) return ["fas", "terminal"];
  if (name.includes("read") || name.includes("file") || name.includes("edit") || name.includes("write"))
    return ["far", "file-lines"];
  if (name.includes("dir") || name.includes("glob") || name.includes("list"))
    return ["far", "folder-open"];
  if (name.includes("grep")) return ["fas", "magnifying-glass"];
  if (name.includes("todo") || name.includes("task")) return ["fas", "list-check"];
  return ["fas", "wrench"];
}

function parseDetail(detail?: string): any | null {
  if (!detail) return null;
  try {
    return JSON.parse(detail);
  } catch {
    return null;
  }
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function numberFrom(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function formatRawOutput(output?: unknown): string | null {
  if (output == null) return null;
  if (typeof output === "string") return output;
  try {
    return JSON.stringify(output, null, 2);
  } catch {
    return String(output);
  }
}

function parseJsonObject(text?: string): Record<string, unknown> | null {
  if (!text) return null;
  try {
    return asRecord(JSON.parse(text));
  } catch {
    return null;
  }
}

function fileNameFromPath(path?: string): string | undefined {
  if (!path) return undefined;
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

function compactPath(path?: string): string | undefined {
  if (!path) return undefined;
  const fileName = fileNameFromPath(path);
  if (!fileName || path.length <= 72) return path;
  return `.../${fileName}`;
}

function markdownStats(markdown: string): Array<{ label: string; value: string }> {
  const headings = markdown.match(/^#{1,6}\s+/gm)?.length ?? 0;
  const tableRows = markdown
    .split(/\r?\n/)
    .filter((line) => line.trim().startsWith("|")).length;
  const chars = Array.from(markdown).length;
  const stats = [{ label: "characters", value: String(chars) }];
  if (headings > 0) stats.push({ label: "headings", value: String(headings) });
  if (tableRows > 0) stats.push({ label: "table rows", value: String(tableRows) });
  return stats;
}

function parseOfficeToolView(tool: ToolCall): OfficeToolView | null {
  if (
    tool.name !== "office_docx_create" &&
    tool.name !== "office_xlsx_create" &&
    tool.name !== "office_read"
  ) {
    return null;
  }
  const args = parseJsonObject(tool.args);
  const output = asRecord(tool.output) ?? parseJsonObject(tool.detail);
  const error =
    (output && typeof output.error === "string" ? output.error : undefined) ||
    (tool.status === "error" || tool.status === "blocked" ? tool.detail : undefined);

  if (tool.name === "office_docx_create") {
    const outPath =
      (output && typeof output.path === "string" ? output.path : undefined) ||
      (args && typeof args.outPath === "string" ? args.outPath : undefined);
    const title = args && typeof args.title === "string" && args.title.trim() ? args.title : "Word document";
    const markdown = args && typeof args.markdown === "string" ? args.markdown : "";
    const fileName = fileNameFromPath(outPath);
    return {
      kind: "docx",
      title,
      path: outPath,
      fileName,
      error,
      summary: error
        ? `Word document failed: ${error}`
        : `Word document created${fileName ? `: ${fileName}` : ""}`,
      stats: markdownStats(markdown),
    };
  }

  if (tool.name === "office_xlsx_create") {
    const outPath =
      (output && typeof output.path === "string" ? output.path : undefined) ||
      (args && typeof args.outPath === "string" ? args.outPath : undefined);
    const sheets = Array.isArray(args?.sheets) ? args.sheets : [];
    const rowCount = sheets.reduce((sum, sheet) => {
      const record = asRecord(sheet);
      const rows = Array.isArray(record?.rows) ? record.rows.length : 0;
      return sum + rows;
    }, 0);
    const fileName = fileNameFromPath(outPath);
    return {
      kind: "xlsx",
      title: "Excel workbook",
      path: outPath,
      fileName,
      error,
      summary: error
        ? `Excel workbook failed: ${error}`
        : `Excel workbook created${fileName ? `: ${fileName}` : ""}`,
      stats: [
        { label: "sheets", value: String(sheets.length) },
        { label: "rows", value: String(rowCount) },
      ],
    };
  }

  const path = args && typeof args.path === "string" ? args.path : undefined;
  const text = output && typeof output.text === "string" ? output.text : "";
  return {
    kind: "read",
    title: "Office document read",
    path,
    fileName: fileNameFromPath(path),
    error,
    summary: error ? `Document read failed: ${error}` : `Document read${path ? `: ${fileNameFromPath(path)}` : ""}`,
    stats: text ? [{ label: "characters", value: String(Array.from(text).length) }] : [],
  };
}

function parseSkillToolView(tool: ToolCall): SkillToolView | null {
  if (tool.name !== "skill") return null;
  const args = parseJsonObject(tool.args);
  const output = asRecord(tool.output) ?? parseJsonObject(tool.detail);
  const requestedId = args && typeof args.id === "string" ? args.id : "skill";
  const error =
    (output && typeof output.error === "string" ? output.error : undefined) ||
    (tool.status === "error" || tool.status === "blocked" ? tool.detail : undefined);
  const id = output && typeof output.id === "string" && output.id.trim() ? output.id : requestedId;
  const name = output && typeof output.name === "string" && output.name.trim() ? output.name : id;
  const body = output && typeof output.body === "string" ? output.body : undefined;
  const baseDir = output && typeof output.base_dir === "string" ? output.base_dir : undefined;
  const outputResources = output?.resources;
  const resources = Array.isArray(outputResources)
    ? outputResources.filter((item): item is string => typeof item === "string")
    : [];
  const stats: Array<{ label: string; value: string }> = [];
  if (body) stats.push({ label: "characters", value: String(Array.from(body).length) });
  stats.push({ label: "resources", value: String(resources.length) });

  return {
    id,
    name,
    body,
    baseDir,
    resources,
    error,
    summary: error ? `Skill failed: ${error}` : `Skill loaded: ${name}`,
    stats,
  };
}

function parseWebSearchView(tool: ToolCall): WebSearchView | null {
  if (tool.name !== "web_search") return null;
  const parsedDetail = parseDetail(tool.detail);
  const data = asRecord(tool.output) ?? asRecord(parsedDetail);
  if (!data) return null;

  const attempts: WebSearchAttemptView[] = Array.isArray(data.attempts)
    ? data.attempts
        .map((item): WebSearchAttemptView | null => {
          const attempt = asRecord(item);
          if (!attempt) return null;
          const provider = typeof attempt.provider === "string" ? attempt.provider : "";
          if (!provider) return null;
          const view: WebSearchAttemptView = {
            provider,
            ok: attempt.ok === true,
          };
          const count = numberFrom(attempt.count);
          if (typeof count === "number") view.count = count;
          if (typeof attempt.error === "string") view.error = attempt.error;
          return view;
        })
        .filter((item): item is WebSearchAttemptView => item !== null)
    : [];

  const resultCount =
    numberFrom(data.count) ??
    (Array.isArray(data.results) ? data.results.length : undefined) ??
    attempts.find((attempt) => attempt.ok)?.count ??
    0;
  const provider =
    (typeof data.provider === "string" && data.provider) ||
    attempts.find((attempt) => attempt.ok)?.provider ||
    "unknown";
  const error = typeof data.error === "string" ? data.error : undefined;
  const query = typeof data.query === "string" ? data.query : undefined;

  if (!error && provider === "unknown" && resultCount === 0 && attempts.length === 0) {
    return null;
  }

  return {
    provider,
    count: resultCount,
    query,
    error,
    attempts,
  };
}

function webSearchSummary(meta: WebSearchView): string {
  if (meta.error) return meta.error;
  return `web_search: ${meta.provider} returned ${meta.count} result(s)`;
}

function nodeLabel(node: any): string {
  if (!node || typeof node !== "object") return "";
  return node.file_path || node.name || node.node_id || "";
}

function codeMapSummary(name: string, detail?: string): string | null {
  if (!name.startsWith("code_map")) return null;
  const data = parseDetail(detail);
  if (!data) return detail || null;

  if (name === "code_map_overview") {
    const status = data.status ?? {};
    const languages = Array.isArray(data.languages) ? data.languages.slice(0, 3).join(", ") : "";
    const frameworks = Array.isArray(data.frameworks) ? data.frameworks.slice(0, 3).join(", ") : "";
    const extras = [languages && `语言 ${languages}`, frameworks && `框架 ${frameworks}`].filter(Boolean).join(" · ");
    return `项目地图：${status.nodes ?? 0} 个节点、${status.edges ?? 0} 条关系、${status.files ?? 0} 个文件${extras ? ` · ${extras}` : ""}`;
  }

  if (name === "code_map_search" && Array.isArray(data)) {
    const preview = data.slice(0, 3).map(nodeLabel).filter(Boolean).join("、");
    return `搜索到 ${data.length} 个地图节点${preview ? `：${preview}` : ""}`;
  }

  if (name === "code_map_neighbors") {
    const counts = [
      ["imports", data.imports?.length ?? 0],
      ["imported_by", data.imported_by?.length ?? 0],
      ["calls", data.calls?.length ?? 0],
      ["called_by", data.called_by?.length ?? 0],
      ["related", data.related?.length ?? 0],
    ];
    const total = counts.reduce((sum, [, count]) => sum + Number(count), 0);
    const parts = counts.filter(([, count]) => Number(count) > 0).map(([label, count]) => `${label} ${count}`);
    return `关系查询：${nodeLabel(data.node) || "当前节点"}，共 ${total} 条关系${parts.length ? `（${parts.join(" / ")}）` : ""}`;
  }

  if (name === "code_map_impact") {
    const direct = data.direct?.length ?? 0;
    const indirect = data.indirect?.length ?? 0;
    return `影响分析：${nodeLabel(data.target) || "目标"}，直接影响 ${direct} 个，间接影响 ${indirect} 个`;
  }

  return detail || null;
}

function WebSearchAttemptPills({ attempts }: { attempts: WebSearchAttemptView[] }) {
  if (attempts.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-1.5">
      {attempts.map((attempt, index) => {
        const className = attempt.ok
          ? "border-green-200 bg-green-50 text-green-700"
          : "border-red-200 bg-red-50 text-red-600";
        const count = typeof attempt.count === "number" ? ` (${attempt.count})` : "";
        return (
          <span
            key={`${attempt.provider}-${index}`}
            className={`inline-flex max-w-full items-center rounded-md border px-1.5 py-0.5 text-[11px] ${className}`}
            title={attempt.error || undefined}
          >
            <span className="truncate">
              {attempt.provider}: {attempt.ok ? "ok" : "failed"}
              {count}
            </span>
          </span>
        );
      })}
    </div>
  );
}

function OfficeToolSummary({ view }: { view: OfficeToolView }) {
  const accent =
    view.kind === "docx"
      ? "bg-blue-50 text-blue-700 border-blue-200"
      : view.kind === "xlsx"
      ? "bg-green-50 text-green-700 border-green-200"
      : "bg-gray-50 text-text-secondary border-border-theme";
  return (
    <div className="rounded-lg border border-border-theme bg-white p-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className={`inline-flex items-center rounded-md border px-1.5 py-0.5 text-[11px] ${accent}`}>
          {view.kind.toUpperCase()}
        </span>
        <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-text-base">
          {view.title}
        </span>
      </div>
      {view.path && (
        <div className="mt-1 truncate font-mono text-[11px] text-text-secondary" title={view.path}>
          {compactPath(view.path)}
        </div>
      )}
      {view.stats.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {view.stats.map((item) => (
            <span
              key={`${item.label}-${item.value}`}
              className="inline-flex items-center rounded-md border border-border-theme bg-[#FbFcFd] px-1.5 py-0.5 text-[11px] text-text-secondary"
            >
              {item.label}: {item.value}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}

function SkillToolSummary({ view }: { view: SkillToolView }) {
  return (
    <div className="rounded-lg border border-border-theme bg-white p-2">
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="inline-flex items-center rounded-md border border-amber-200 bg-amber-50 px-1.5 py-0.5 text-[11px] text-amber-700">
          SKILL
        </span>
        <span className="min-w-0 flex-1 truncate text-[12px] font-medium text-text-base">
          {view.name}
        </span>
        <span className="font-mono text-[11px] text-text-secondary">{view.id}</span>
      </div>
      {view.baseDir && (
        <div className="mt-1 truncate font-mono text-[11px] text-text-secondary" title={view.baseDir}>
          {compactPath(view.baseDir)}
        </div>
      )}
      {view.stats.length > 0 && (
        <div className="mt-2 flex flex-wrap gap-1.5">
          {view.stats.map((item) => (
            <span
              key={`${item.label}-${item.value}`}
              className="inline-flex items-center rounded-md border border-border-theme bg-[#FbFcFd] px-1.5 py-0.5 text-[11px] text-text-secondary"
            >
              {item.label}: {item.value}
            </span>
          ))}
        </div>
      )}
      {view.resources.length > 0 && (
        <div className="mt-2 max-h-16 overflow-auto rounded-md border border-border-theme bg-[#FbFcFd] p-1.5 font-mono text-[11px] text-text-secondary">
          {view.resources.slice(0, 8).map((resource) => (
            <div key={resource} className="truncate" title={resource}>
              {resource}
            </div>
          ))}
          {view.resources.length > 8 && <div>+{view.resources.length - 8} more</div>}
        </div>
      )}
    </div>
  );
}

/** A single inline tool-call card: name + status + collapsible args/detail. */
export function ToolCallCard({ tool }: { tool: ToolCall }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const webSearch = parseWebSearchView(tool);
  const officeTool = parseOfficeToolView(tool);
  const skillTool = parseSkillToolView(tool);
  const readableDetail = webSearch
    ? webSearchSummary(webSearch)
    : officeTool
    ? officeTool.summary
    : skillTool
    ? skillTool.summary
    : codeMapSummary(tool.name, tool.detail) ?? tool.detail;
  const isCodeMap = tool.name.startsWith("code_map");
  const isWebSearch = tool.name === "web_search";
  const isOfficeTool = Boolean(officeTool);
  const isSkillTool = Boolean(skillTool);
  const rawOutput = formatRawOutput(tool.output);
  const openProjectMap = (event: MouseEvent) => {
    event.stopPropagation();
    window.dispatchEvent(new CustomEvent(PROJECT_MAP_OPEN_EVENT));
  };
  const copyToolResult = (event: MouseEvent) => {
    event.stopPropagation();
    navigator.clipboard?.writeText(rawOutput || tool.detail || readableDetail || "").catch(() => {});
  };

  const statusMeta: Record<
    ToolCall["status"],
    { dot: string; label: string; labelClass: string }
  > = {
    running: { dot: "bg-blue-500 animate-pulse", label: t("toolCard.running"), labelClass: "text-blue-500" },
    ok: { dot: "bg-green-500", label: t("toolCard.ok"), labelClass: "text-green-600" },
    error: { dot: "bg-red-500", label: t("toolCard.error"), labelClass: "text-red-500" },
    blocked: { dot: "bg-amber-500", label: t("toolCard.blocked"), labelClass: "text-amber-600" },
  };
  const meta = statusMeta[tool.status];

  return (
    <div className="my-2 w-full border border-border-theme rounded-xl bg-[#FbFcFd] overflow-hidden">
      <div
        className="flex items-center px-3 py-2 cursor-pointer hover:bg-gray-50 transition-colors"
        onClick={() => setOpen((v) => !v)}
      >
        <FontAwesomeIcon icon={iconFor(tool.name)} className="text-text-secondary w-4 mr-2.5 text-[13px]" />
        <span className="text-[13px] font-medium text-text-base">{tool.name}</span>
        <span className={`ml-2 w-1.5 h-1.5 rounded-full ${meta.dot}`} />
        <span className={`ml-1.5 text-[11px] ${meta.labelClass}`}>{meta.label}</span>
        {typeof tool.durationMs === "number" && tool.status !== "running" && (
          <span className="ml-2 text-[11px] text-text-secondary tabular-nums">{tool.durationMs}ms</span>
        )}
        {webSearch && (
          <span className="ml-2 max-w-[180px] truncate rounded-md border border-border-theme bg-white px-1.5 py-0.5 text-[11px] text-text-secondary">
            {webSearch.provider} / {webSearch.count}
          </span>
        )}
        {officeTool && (
          <span className="ml-2 max-w-[180px] truncate rounded-md border border-border-theme bg-white px-1.5 py-0.5 text-[11px] text-text-secondary">
            {officeTool.kind.toUpperCase()}
            {officeTool.fileName ? ` / ${officeTool.fileName}` : ""}
          </span>
        )}
        {skillTool && (
          <span className="ml-2 max-w-[180px] truncate rounded-md border border-amber-200 bg-amber-50 px-1.5 py-0.5 text-[11px] text-amber-700">
            {skillTool.name}
          </span>
        )}
        <span className="flex-1" />
        {isCodeMap && (
          <div className="mr-2 flex items-center gap-1">
            <button
              type="button"
              className="h-6 rounded-md px-2 text-[11px] text-text-secondary hover:bg-gray-100 hover:text-text-base"
              onClick={openProjectMap}
            >
              打开项目地图
            </button>
            {tool.detail && (
              <button
                type="button"
                className="h-6 rounded-md px-2 text-[11px] text-text-secondary hover:bg-gray-100 hover:text-text-base"
                onClick={copyToolResult}
              >
                复制结果
              </button>
            )}
          </div>
        )}
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-up" : "chevron-down"]}
          className="text-[10px] text-text-secondary"
        />
      </div>

      {/* One-line detail preview when collapsed (and present). */}
      {!open && readableDetail && (
        <div className="px-3 pb-2 -mt-0.5">
          <div
            className={`text-[12px] truncate ${
              tool.status === "error" || tool.status === "blocked"
                ? "text-red-500"
                : "text-text-secondary"
            }`}
          >
            {readableDetail}
          </div>
          {webSearch && webSearch.attempts.length > 0 && (
            <div className="mt-1">
              <WebSearchAttemptPills attempts={webSearch.attempts} />
            </div>
          )}
        </div>
      )}

      {open && (
        <div className="border-t border-border-theme px-3 py-2 space-y-2">
          {tool.args && !isOfficeTool && !isSkillTool && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-text-secondary mb-1">
                {t("toolCard.arguments")}
              </div>
              <pre className="text-[12px] text-text-base bg-white border border-border-theme rounded-lg p-2 overflow-x-auto whitespace-pre-wrap break-words">
                {tool.args}
              </pre>
            </div>
          )}
          {webSearch && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-text-secondary mb-1">
                Search provider
              </div>
              <div className="flex flex-wrap items-center gap-1.5">
                <span className="inline-flex max-w-full items-center rounded-md border border-border-theme bg-white px-1.5 py-0.5 text-[11px] text-text-secondary">
                  <span className="truncate">provider: {webSearch.provider}</span>
                </span>
                <span className="inline-flex items-center rounded-md border border-border-theme bg-white px-1.5 py-0.5 text-[11px] text-text-secondary">
                  {webSearch.count} result(s)
                </span>
                {webSearch.query && (
                  <span className="inline-flex max-w-full items-center rounded-md border border-border-theme bg-white px-1.5 py-0.5 text-[11px] text-text-secondary">
                    <span className="truncate">query: {webSearch.query}</span>
                  </span>
                )}
              </div>
              <div className="mt-1.5">
                <WebSearchAttemptPills attempts={webSearch.attempts} />
              </div>
            </div>
          )}
          {officeTool && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-text-secondary mb-1">
                Document
              </div>
              <OfficeToolSummary view={officeTool} />
            </div>
          )}
          {skillTool && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-text-secondary mb-1">
                Skill
              </div>
              <SkillToolSummary view={skillTool} />
            </div>
          )}
          {readableDetail && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-text-secondary mb-1">
                {tool.status === "error" || tool.status === "blocked"
                  ? t("toolCard.errorDetail")
                  : t("toolCard.result")}
              </div>
              <div
                className={`text-[12px] whitespace-pre-wrap break-words ${
                  tool.status === "error" || tool.status === "blocked"
                    ? "text-red-500"
                    : "text-text-secondary"
                }`}
              >
                {readableDetail}
              </div>
              {isCodeMap && tool.detail && readableDetail !== tool.detail && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-[11px] text-text-secondary hover:text-text-base">
                    查看原始 JSON
                  </summary>
                  <pre className="mt-1 text-[11px] text-text-secondary bg-white border border-border-theme rounded-lg p-2 overflow-x-auto whitespace-pre-wrap break-words">
                    {tool.detail}
                  </pre>
                </details>
              )}
              {isWebSearch && rawOutput && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-[11px] text-text-secondary hover:text-text-base">
                    Raw JSON
                  </summary>
                  <pre className="mt-1 text-[11px] text-text-secondary bg-white border border-border-theme rounded-lg p-2 overflow-x-auto whitespace-pre-wrap break-words">
                    {rawOutput}
                  </pre>
                </details>
              )}
              {isOfficeTool && tool.args && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-[11px] text-text-secondary hover:text-text-base">
                    Raw arguments
                  </summary>
                  <pre className="mt-1 max-h-64 text-[11px] text-text-secondary bg-white border border-border-theme rounded-lg p-2 overflow-auto whitespace-pre-wrap break-words">
                    {tool.args}
                  </pre>
                </details>
              )}
              {isOfficeTool && rawOutput && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-[11px] text-text-secondary hover:text-text-base">
                    Raw result
                  </summary>
                  <pre className="mt-1 text-[11px] text-text-secondary bg-white border border-border-theme rounded-lg p-2 overflow-x-auto whitespace-pre-wrap break-words">
                    {rawOutput}
                  </pre>
                </details>
              )}
              {isSkillTool && tool.args && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-[11px] text-text-secondary hover:text-text-base">
                    Raw arguments
                  </summary>
                  <pre className="mt-1 text-[11px] text-text-secondary bg-white border border-border-theme rounded-lg p-2 overflow-x-auto whitespace-pre-wrap break-words">
                    {tool.args}
                  </pre>
                </details>
              )}
              {isSkillTool && rawOutput && (
                <details className="mt-2">
                  <summary className="cursor-pointer text-[11px] text-text-secondary hover:text-text-base">
                    Raw skill body
                  </summary>
                  <pre className="mt-1 max-h-64 text-[11px] text-text-secondary bg-white border border-border-theme rounded-lg p-2 overflow-auto whitespace-pre-wrap break-words">
                    {rawOutput}
                  </pre>
                </details>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
