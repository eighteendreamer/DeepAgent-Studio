import { cancelWorkflow } from "../../utils/workflowExecutor";
import { Ferrofluid } from "./Ferrofluid";

interface Props {
  nodeId: string;
  label: string;
}

/**
 * 生成中的节点遮罩：暗色底 + Ferrofluid 流体动效 + 文案与取消。
 *
 * 卡片本身可拖拽，所以浮层的手势必须就地拦下；流体也关掉鼠标交互，
 * 免得在这么小的盒子里跟节点抢指针。
 */
export function GeneratingOverlay({ nodeId, label }: Props) {
  const stopGesture = (event: React.SyntheticEvent) => event.stopPropagation();
  return (
    <div
      className="absolute inset-0 overflow-hidden rounded-lg"
      style={{ background: "rgba(15,23,42,0.6)", backdropFilter: "blur(8px)" }}
      onPointerDown={stopGesture}
      onMouseDown={stopGesture}
    >
      <div className="pointer-events-none absolute inset-0">
        <Ferrofluid mouseInteraction={false} />
      </div>
      <div className="relative flex h-full w-full flex-col items-center justify-center gap-3">
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
    </div>
  );
}
