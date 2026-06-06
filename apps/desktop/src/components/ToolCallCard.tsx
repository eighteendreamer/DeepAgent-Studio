import { useState } from "react";
import type { MouseEvent } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { ToolCall } from "../types";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";

/** Map a tool name to a representative icon. */
function iconFor(name: string): IconProp {
  if (name.startsWith("code_map")) return ["fas", "share-nodes"];
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

/** A single inline tool-call card: name + status + collapsible args/detail. */
export function ToolCallCard({ tool }: { tool: ToolCall }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const readableDetail = codeMapSummary(tool.name, tool.detail) ?? tool.detail;
  const isCodeMap = tool.name.startsWith("code_map");
  const openProjectMap = (event: MouseEvent) => {
    event.stopPropagation();
    window.dispatchEvent(new CustomEvent(PROJECT_MAP_OPEN_EVENT));
  };
  const copyToolResult = (event: MouseEvent) => {
    event.stopPropagation();
    navigator.clipboard?.writeText(tool.detail || readableDetail || "").catch(() => {});
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
        </div>
      )}

      {open && (
        <div className="border-t border-border-theme px-3 py-2 space-y-2">
          {tool.args && (
            <div>
              <div className="text-[10px] uppercase tracking-wide text-text-secondary mb-1">
                {t("toolCard.arguments")}
              </div>
              <pre className="text-[12px] text-text-base bg-white border border-border-theme rounded-lg p-2 overflow-x-auto whitespace-pre-wrap break-words">
                {tool.args}
              </pre>
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
            </div>
          )}
        </div>
      )}
    </div>
  );
}
