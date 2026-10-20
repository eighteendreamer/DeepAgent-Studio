import type { MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from "react";
import type { NodeProps } from "@xyflow/react";
import { Plus } from "lucide-react";
import type { WorkflowNode } from "../types";

export const SELECTION_NODE_ID = "__selection-frame__";
export const SELECTION_NODE_TYPE = "selection-frame";
/** 选区框相对成员节点的外扩边距。 */
export const SELECTION_FRAME_PADDING = 12;

export type SelectionFrameConnectionSide = "left" | "right";

export interface SelectionFrameNodeData extends Record<string, unknown> {
  selectedNodeIds: string[];
  onConnectionPointerDown: (
    nodeIds: string[],
    side: SelectionFrameConnectionSide,
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => void;
  onConnectionPointerMove: (
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => void;
  onConnectionPointerUp: (
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => void;
  onConnectionPointerCancel: (
    event: ReactPointerEvent<HTMLButtonElement>,
  ) => void;
}

function ConnectionButton({
  side,
  label,
  nodeIds,
  data,
}: {
  side: SelectionFrameConnectionSide;
  label: string;
  nodeIds: string[];
  data: SelectionFrameNodeData;
}) {
  const stopClick = (event: ReactMouseEvent<HTMLButtonElement>) => {
    event.preventDefault();
    event.stopPropagation();
  };

  return (
    <button
      type="button"
      className="nokey nodrag nopan"
      data-nc-multi-connect-handle="true"
      aria-label={label}
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.stopPropagation();
        event.currentTarget.setPointerCapture(event.pointerId);
        data.onConnectionPointerDown(nodeIds, side, event);
      }}
      onPointerMove={(event) => {
        event.stopPropagation();
        data.onConnectionPointerMove(event);
      }}
      onPointerUp={(event) => {
        event.preventDefault();
        event.stopPropagation();
        data.onConnectionPointerUp(event);
      }}
      onPointerCancel={(event) => {
        event.stopPropagation();
        data.onConnectionPointerCancel(event);
      }}
      onLostPointerCapture={(event) => {
        event.stopPropagation();
        data.onConnectionPointerCancel(event);
      }}
      onClick={stopClick}
      style={{
        position: "absolute",
        // React Flow's node Handle has a 48px invisible hit area. Place the
        // group control beyond that area so it cannot be intercepted by the
        // outermost selected node's ordinary Handle.
        ...(side === "left" ? { left: -44 } : { right: -44 }),
        top: "50%",
        zIndex: 2,
        display: "flex",
        width: 24,
        height: 24,
        alignItems: "center",
        justifyContent: "center",
        transform: "translateY(-50%)",
        padding: 0,
        border: "1px solid rgba(95,201,209,0.52)",
        borderRadius: 9999,
        background: "rgba(30,30,35,0.94)",
        color: "rgba(148,225,229,0.9)",
        cursor: "crosshair",
        pointerEvents: "all",
        touchAction: "none",
        userSelect: "none",
      }}
    >
      <Plus aria-hidden="true" style={{ width: 13, height: 13 }} strokeWidth={2.2} />
    </button>
  );
}

export function SelectionFrameNode({ data }: NodeProps<WorkflowNode>) {
  // 该渲染节点是画布临时节点，不属于持久化的工作流数据联合类型。
  const connectionData = data as unknown as SelectionFrameNodeData;
  const nodeIds = connectionData.selectedNodeIds ?? [];

  return (
    <div
      className="absolute inset-0 rounded-[6px]"
      style={{
        border: "1px dashed rgba(59,130,246,0.65)",
        background: "rgba(59,130,246,0.03)",
        pointerEvents: "none",
      }}
    >
      <ConnectionButton
        side="left"
        label="批量连接到选中节点"
        nodeIds={nodeIds}
        data={connectionData}
      />
      <ConnectionButton
        side="right"
        label="批量连接选中节点"
        nodeIds={nodeIds}
        data={connectionData}
      />
    </div>
  );
}
