import type { ApprovalRequest } from "../types";

interface Props {
  request: ApprovalRequest | null;
  onApprove: (req: ApprovalRequest) => void;
  onReject: (req: ApprovalRequest) => void;
}

export function ApprovalDialog({ request, onApprove, onReject }: Props) {
  if (!request) return null;
  return (
    <div className="dialog-backdrop">
      <div className="dialog">
        <div className="dialog-header">
          <span className="risk-pill">{request.risk.toUpperCase()} RISK</span>
          <h3>Approve tool: {request.tool}</h3>
        </div>
        <p className="dialog-reason">{request.reason}</p>
        <pre className="dialog-args">{request.arguments}</pre>
        <div className="dialog-actions">
          <button className="btn btn-reject" onClick={() => onReject(request)}>
            Reject
          </button>
          <button className="btn btn-approve" onClick={() => onApprove(request)}>
            Approve &amp; Run
          </button>
        </div>
      </div>
    </div>
  );
}
