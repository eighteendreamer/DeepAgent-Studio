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

export function ApprovalDialog({ request, queueCount = 0, onApprove, onReject }: Props) {
  const { t } = useTranslation();
  if (!request) return null;
  const remaining = Math.max(0, queueCount - 1);
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 backdrop-blur-sm">
      <div className="w-[480px] max-w-[90vw] bg-white rounded-2xl shadow-xl border border-border-theme overflow-hidden">
        <div className="px-6 py-4 border-b border-border-theme">
          <div className="flex items-center gap-2 mb-2">
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
          <h3 className="text-base font-semibold text-text-base">
            {t("approvalDialog.requestTool")}<span className="font-mono">{request.tool}</span>
          </h3>
        </div>
        <div className="px-6 py-4">
          <p className="text-sm text-text-secondary mb-3">{request.reason}</p>
          <pre className="text-[12px] font-mono text-text-base whitespace-pre-wrap bg-gray-50 border border-border-theme rounded-lg p-3 max-h-60 overflow-y-auto">
            {request.arguments}
          </pre>
        </div>
        <div className="px-6 py-4 border-t border-border-theme flex justify-end gap-3">
          <button
            className="px-4 py-1.5 rounded-full text-sm border border-border-theme text-text-base hover:bg-gray-50 transition-colors"
            onClick={() => onReject(request)}
          >
            {t("approvalDialog.reject")}
          </button>
          <button
            className="px-4 py-1.5 rounded-full text-sm bg-primary text-white hover:bg-opacity-90 transition-colors shadow-sm"
            onClick={() => onApprove(request)}
          >
            {t("approvalDialog.approve")}
          </button>
        </div>
      </div>
    </div>
  );
}
