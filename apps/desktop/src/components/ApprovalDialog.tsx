import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { ApprovalRequest } from "../types";

interface Props {
  request: ApprovalRequest | null;
  /** How many approvals are queued in total (including the current one). */
  queueCount?: number;
  onApprove: (req: ApprovalRequest) => void;
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
 * A compact approval card that floats directly above the composer rather than
 * a fullscreen modal. It shows the tool, risk, reason, and a human-readable
 * summary of the requested action without exposing raw JSON.
 */
export function ApprovalDialog({ request, queueCount = 0, onApprove, onReject }: Props) {
  const { t } = useTranslation();
  const remaining = Math.max(0, queueCount - 1);
  const content = request ? approvalContent(request.arguments) : [];

  if (!request) return null;

  return (
        <div
          key={request.call_id}
          className="approval-card w-full bg-white rounded-2xl shadow-[0_8px_30px_rgb(0,0,0,0.12)] border border-border-theme overflow-hidden"
        >
          <div className="px-4 pt-3 pb-2 flex items-center gap-2">
            <FontAwesomeIcon
              icon={["fas", "shield-halved"]}
              className="text-text-secondary text-[13px]"
            />
            <span className="text-[13px] font-semibold text-text-base">
              {t("approvalDialog.requestTool")}
              <span className="font-mono">{request.tool}</span>
            </span>
            <span
              className={`text-[10px] font-semibold uppercase tracking-wide rounded-full px-2 py-0.5 ${riskClasses(
                request.risk
              )}`}
            >
              {t("approvalDialog.risk", { risk: request.risk })}
            </span>
            {remaining > 0 && (
              <span className="text-[10px] text-text-secondary border border-border-theme rounded-full px-2 py-0.5">
                {t("approvalDialog.pending", { count: remaining })}
              </span>
            )}
          </div>

          <div className="px-4 pb-3">
            {request.reason && (
              <p className="text-[12px] text-text-secondary mb-2">{request.reason}</p>
            )}
            {content.length > 0 && (
              <div className="text-[13px] text-text-base leading-relaxed bg-gray-50 border border-border-theme rounded-lg px-3 py-2.5 max-h-44 overflow-y-auto">
                {content.map((line, index) => (
                  <div key={`${request.call_id}-${index}`} className="break-words whitespace-pre-wrap">
                    {line}
                  </div>
                ))}
              </div>
            )}
          </div>

          <div className="px-4 py-2.5 border-t border-border-theme flex justify-end gap-2 bg-[#FbFcFd]">
            <button
              className="px-3.5 py-1.5 rounded-full text-[13px] border border-border-theme text-text-base hover:bg-gray-100 transition-colors"
              onClick={() => onReject(request)}
            >
              {t("approvalDialog.reject")}
            </button>
            <button
              className="px-3.5 py-1.5 rounded-full text-[13px] bg-primary text-white hover:bg-opacity-90 transition-colors shadow-sm"
              onClick={() => onApprove(request)}
            >
              {t("approvalDialog.approve")}
            </button>
          </div>
        </div>
  );
}
