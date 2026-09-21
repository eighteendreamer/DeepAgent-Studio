import { Handle, Position } from "@xyflow/react";
import type { NodeProps } from "@xyflow/react";

/**
 * 框选时的"选区节点"：一个临时虚拟节点，左右各一个真实连接桩。
 *
 * 做成节点而不是画一个按钮，是为了让拉线、吸附、松手落到节点或空白处这些行为
 * 与普通节点完全一致：它只存在于传给 ReactFlow 的数组里，不进 store、不进持久化、
 * 也不进发给内核的图。
 */
export const SELECTION_NODE_ID = "__selection-frame__";
export const SELECTION_NODE_TYPE = "selection-frame";
/** 选区框相对成员节点的外扩边距。 */
export const SELECTION_FRAME_PADDING = 12;

const HANDLE_BASE = {
  width: 20,
  height: 20,
  top: "50%",
  transform: "translateY(-50%)",
  background: "transparent",
  border: "none",
  cursor: "crosshair",
  pointerEvents: "all",
} as const;

function HandleDot() {
  return (
    <div
      className="flex items-center justify-center rounded-full"
      style={{
        width: 20,
        height: 20,
        background: "rgba(30,30,35,0.9)",
        border: "1.5px solid rgba(156,163,175,0.5)",
        pointerEvents: "none",
      }}
    >
      <div style={{ width: 6, height: 6, borderRadius: "50%", background: "rgba(156,163,175,0.7)" }} />
    </div>
  );
}

export function SelectionFrameNode(_props: NodeProps) {
  return (
    <div
      className="absolute inset-0 rounded-[6px]"
      style={{
        border: "1px dashed rgba(59,130,246,0.65)",
        background: "rgba(59,130,246,0.03)",
        pointerEvents: "none",
      }}
    >
      <Handle type="target" position={Position.Left} style={{ ...HANDLE_BASE, left: -20 }}>
        <HandleDot />
      </Handle>
      <Handle type="source" position={Position.Right} style={{ ...HANDLE_BASE, right: -20 }}>
        <HandleDot />
      </Handle>
    </div>
  );
}
