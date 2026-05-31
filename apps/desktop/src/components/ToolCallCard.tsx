import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { ToolCall } from "../types";

/** Map a tool name to a representative icon. */
function iconFor(name: string): IconProp {
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

/** A single inline tool-call card: name + status + collapsible args/detail. */
export function ToolCallCard({ tool }: { tool: ToolCall }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

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
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-up" : "chevron-down"]}
          className="text-[10px] text-text-secondary"
        />
      </div>

      {/* One-line detail preview when collapsed (and present). */}
      {!open && tool.detail && (
        <div className="px-3 pb-2 -mt-0.5">
          <div
            className={`text-[12px] truncate ${
              tool.status === "error" || tool.status === "blocked"
                ? "text-red-500"
                : "text-text-secondary"
            }`}
          >
            {tool.detail}
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
          {tool.detail && (
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
                {tool.detail}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
