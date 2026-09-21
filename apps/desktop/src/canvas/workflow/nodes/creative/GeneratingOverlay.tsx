import { cancelWorkflow } from "../../utils/workflowExecutor";

interface Props {
  nodeId: string;
  label: string;
}

/**
 * 生成中的节点遮罩，视觉取自 Penguin-Magic InfiniteCanvas 的生成中态：
 * 毛玻璃压暗 + 旋转指示环 + 文案 + 取消胶囊。
 *
 * 卡片本身可拖拽，所以浮层的手势必须就地拦下，不能冒泡到节点。
 */
export function GeneratingOverlay({ nodeId, label }: Props) {
  const stopGesture = (event: React.SyntheticEvent) => event.stopPropagation();
  return (
    <div
      className="absolute inset-0 flex flex-col items-center justify-center gap-3 rounded-lg"
      style={{ background: "rgba(15,23,42,0.6)", backdropFilter: "blur(8px)" }}
      onPointerDown={stopGesture}
      onMouseDown={stopGesture}
    >
      <span className="h-8 w-8 animate-spin rounded-full border-[2.5px] border-white/20 border-t-white/80" />
      <span className="text-[12px] font-medium text-white/80">{label}</span>
      <button
        type="button"
        onClick={() => void cancelWorkflow(nodeId)}
        className="mt-1 rounded-full px-4 py-1.5 text-[11px] font-medium text-white/70 transition-all hover:bg-white/10 hover:text-white"
        style={{
          background: "rgba(255,255,255,0.06)",
          boxShadow: "0 0 0 1px rgba(255,255,255,0.2)",
        }}
      >
        取消
      </button>
    </div>
  );
}
