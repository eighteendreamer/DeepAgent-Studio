import { ApprovalDialog } from "../../../components/ApprovalDialog";
import { Button } from "../../../components/shadcn/button";
import {
  Dialog, DialogContent, DialogHeader, DialogTitle, DialogDescription, DialogFooter,
} from "../../../components/shadcn/dialog";
import { useCanvasStore } from "../store/canvasStore";
import { cancelWorkflow, respondWorkflowApproval } from "../utils/workflowExecutor";

/** Canvas 只展示/回答统一内核的请求，不拥有另一套审批策略。 */
export function WorkflowApprovalDialog() {
  const view = useCanvasStore((state) => state.workflowApproval);
  const request = view?.requests[0] ?? null;
  return (
    <Dialog open={Boolean(request)}>
      <DialogContent className="max-w-xl" zIndexClass="z-[1200]" closeOnBackdrop={false} closeOnEscape={false}>
        <DialogHeader>
          <div>
            <DialogTitle>工作流工具审批</DialogTitle>
            <DialogDescription>批准仅作用于下方请求及所选作用域；拒绝不授权本次工具执行，取消会通知内核停止工作流。</DialogDescription>
          </div>
        </DialogHeader>
        <div className="space-y-3 overflow-y-auto px-6 pb-2">
        <ApprovalDialog
          request={request}
          queueCount={view?.requests.length}
          disabled={view?.submitting}
          onApprove={(pending, scope) => { void respondWorkflowApproval(pending, true, scope); }}
          onReject={(pending) => { void respondWorkflowApproval(pending, false); }}
        />
        {view?.submitting && <p role="status" className="text-sm text-text-secondary">正在提交审批回答…</p>}
        {view?.error && <p role="alert" className="text-sm text-red-600">{view.error}</p>}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => { void cancelWorkflow(); }}>取消工作流</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
