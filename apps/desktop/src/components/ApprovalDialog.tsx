import { ChevronDown, ShieldCheck, SquareTerminal } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ApprovalRequest } from "../types";
import { Button } from "./shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./shadcn/dropdown-menu";
import { Panel } from "./ui/Panel";

interface Props {
  request: ApprovalRequest | null;
  /** How many approvals are queued in total (including the current one). */
  queueCount?: number;
  disabled?: boolean;
  /** `scope` widens the answer; omit it to approve this call only. */
  onApprove: (req: ApprovalRequest, scope?: string) => void;
  onReject: (req: ApprovalRequest) => void;
}

function riskClasses(risk: string): string {
  switch (risk.toLowerCase()) {
    case "high":
      return "bg-red-100 text-red-700";
    case "medium":
      return "bg-amber-100 text-amber-700";
    case "ask":
      return "bg-blue-100 text-blue-700";
    default:
      return "bg-gray-100 text-gray-600";
  }
}

function formatArgumentValue(value: unknown): string {
  if (value == null) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (Array.isArray(value)) return value.map(formatArgumentValue).filter(Boolean).join(", ");
  if (typeof value === "object") {
    return Object.entries(value as Record<string, unknown>)
      .map(([key, nested]) => `${key}: ${formatArgumentValue(nested)}`)
      .join("\n");
  }
  return String(value);
}

function approvalContent(argumentsText: string): string[] {
  const raw = argumentsText.trim();
  if (!raw) return [];

  try {
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const obj = parsed as Record<string, unknown>;
      if (typeof obj.command === "string") return [obj.command];
      if (typeof obj.path === "string") return [obj.path];

      const entries = Object.entries(obj)
        .map(([key, value]) => {
          const formatted = formatArgumentValue(value);
          return formatted ? `${key}: ${formatted}` : "";
        })
        .filter(Boolean);
      if (entries.length > 0) return entries;
    }

    const formatted = formatArgumentValue(parsed);
    return formatted ? [formatted] : [];
  } catch {
    return [raw];
  }
}

/**
 * A compact approval card that overlays the composer like the question card,
 * not a fullscreen modal. It shows the tool, risk, reason, and a human-readable
 * summary of the requested action without exposing raw JSON.
 */
export function ApprovalDialog({ request, queueCount = 0, disabled = false, onApprove, onReject }: Props) {
  const { t } = useTranslation();
  const remaining = Math.max(0, queueCount - 1);
  const content = request ? approvalContent(request.arguments) : [];

  if (!request) return null;

  const allowPrefix = request.scopes?.includes("command_prefix") && !!request.command_prefix;
  const allowSession = request.scopes?.includes("tool_session");
  const hasScopes = allowPrefix || allowSession;
  const isTerminal = request.tool === "bash";
  const ToolIcon = isTerminal ? SquareTerminal : ShieldCheck;

  return (
    <Panel
      menu={false}
      role="region"
      aria-label={`${t("approvalDialog.requestTool")}${request.tool}`}
      className="approval-card flex min-h-0 w-full max-w-4xl flex-col overflow-hidden rounded-3xl border border-border-theme bg-bg-base shadow-[0_12px_36px_rgba(0,0,0,0.10)]"
      style={{ maxHeight: "inherit" }}
    >
      <div className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 px-5 pt-4 text-text-secondary">
        <ToolIcon size={15} className="shrink-0" aria-hidden="true" />
        <span className={`min-w-0 break-all text-[12px] ${isTerminal ? "" : "font-mono"}`}>
          {isTerminal ? t("approvalDialog.terminal") : request.tool}
        </span>
        <span
          className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${riskClasses(request.risk)}`}
        >
          {t("approvalDialog.risk", { risk: request.risk })}
        </span>
        {remaining > 0 && (
          <span className="ml-auto text-[11px]">
            {t("approvalDialog.pending", { count: remaining })}
          </span>
        )}
      </div>

      <div className="min-h-0 overflow-y-auto overscroll-contain px-5 pt-2.5 pb-2">
        <p className="text-[14px] font-medium leading-6 text-text-base [overflow-wrap:anywhere]">
          {request.reason || `${t("approvalDialog.requestTool")}${request.tool}`}
        </p>
        {content.length > 0 && (
          <pre className="mt-3 whitespace-pre-wrap font-mono text-[12px] leading-5 text-text-secondary [overflow-wrap:anywhere]">
            {content.join("\n")}
          </pre>
        )}
      </div>

      <div className="flex shrink-0 flex-wrap items-center justify-end gap-2 px-5 pt-2 pb-4">
        <Button
          disabled={disabled}
          variant="outline"
          size="sm"
          className="rounded-full font-normal shadow-none"
          onClick={() => onReject(request)}
        >
          {t("approvalDialog.reject")}
        </Button>
        <div className="flex shrink-0 items-center">
          <Button
            disabled={disabled}
            size="sm"
            className={`rounded-full shadow-none ${hasScopes ? "rounded-r-none" : ""}`}
            onClick={() => onApprove(request)}
          >
            {t("approvalDialog.approve")}
          </Button>
          {hasScopes && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button
                  disabled={disabled}
                  size="icon"
                  className="h-8 w-8 rounded-full rounded-l-none border-l border-white/20 shadow-none"
                  aria-label={t("approvalDialog.moreScopes")}
                >
                  <ChevronDown size={14} aria-hidden="true" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent
                side="top"
                align="end"
                collisionPadding={16}
                className="max-w-[min(360px,calc(100vw-32px))]"
              >
                {/* Drawn only from scopes the backend gate actually honors. */}
                {allowPrefix && (
                  <DropdownMenuItem
                    disabled={disabled}
                    className="whitespace-normal [overflow-wrap:anywhere]"
                    onSelect={() => onApprove(request, "command_prefix")}
                  >
                    {t("approvalDialog.approvePrefix", { prefix: request.command_prefix })}
                  </DropdownMenuItem>
                )}
                {allowSession && (
                  <DropdownMenuItem disabled={disabled} onSelect={() => onApprove(request, "tool_session")}>
                    {t("approvalDialog.approveSession")}
                  </DropdownMenuItem>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>
    </Panel>
  );
}
