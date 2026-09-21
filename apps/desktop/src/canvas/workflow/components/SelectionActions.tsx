import { Merge } from "lucide-react";
import type { Viewport } from "@xyflow/react";
import type { WorkflowNode } from "../types";

const SURFACE = "rgba(24,24,27,0.92)";
/** 节点还没被测量时的兜底尺寸，与 dagre 布局默认值一致。 */
const FALLBACK_WIDTH = 240;
const FALLBACK_HEIGHT = 93;
/** 操作条与选区的间距，以及新节点相对选区右边缘的留白。 */
const BAR_GAP = 40;
const NEW_NODE_GAP = 120;

interface Props {
  nodes: WorkflowNode[];
  viewport: Viewport;
  /**
   * @param sourceIds 要接入新节点的选中节点
   * @param world 新节点的落点（画布坐标）
   */
  onCreateDownstream: (sourceIds: string[], world: { x: number; y: number }) => void;
}

/**
 * 框选后的批量操作条：把"逐个拉线到新节点"换成一次框选。
 *
 * 只有操作条本身吃指针事件，不挡住框内节点。
 */
export function SelectionActions({ nodes, viewport, onCreateDownstream }: Props) {
  const selected = nodes.filter((node) => node.selected);
  if (selected.length < 2) return null;

  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const node of selected) {
    const width = node.measured?.width ?? (node.width as number | undefined) ?? FALLBACK_WIDTH;
    const height = node.measured?.height ?? (node.height as number | undefined) ?? FALLBACK_HEIGHT;
    left = Math.min(left, node.position.x);
    top = Math.min(top, node.position.y);
    right = Math.max(right, node.position.x + width);
    bottom = Math.max(bottom, node.position.y + height);
  }

  const { zoom, x: vx, y: vy } = viewport;
  const centerX = ((left + right) / 2) * zoom + vx;
  const topEdge = top * zoom + vy;
  const sourceIds = selected.map((node) => node.id);

  return (
    <div
      className="absolute z-[6] flex -translate-x-1/2 items-center gap-1 rounded-xl px-1.5 py-1"
      style={{
        left: centerX,
        top: Math.max(8, topEdge - BAR_GAP),
        background: SURFACE,
        border: "1px solid rgba(255,255,255,0.08)",
        backdropFilter: "blur(20px)",
        boxShadow: "0 10px 30px rgba(0,0,0,0.35)",
      }}
      onPointerDown={(event) => event.stopPropagation()}
      onMouseDown={(event) => event.stopPropagation()}
    >
      <span className="px-1.5 text-[11px]" style={{ color: "rgba(255,255,255,0.45)" }}>
        已选 {sourceIds.length} 个节点
      </span>
      <button
        type="button"
        onClick={() =>
          onCreateDownstream(sourceIds, {
            x: right + NEW_NODE_GAP / zoom,
            y: (top + bottom) / 2 - FALLBACK_HEIGHT / 2,
          })
        }
        className="flex h-7 items-center gap-1.5 rounded-lg px-2 text-[11px] text-white/75 transition-colors hover:bg-white/10"
        title="新建一个下游节点，并把选中的节点全部接入它"
      >
        <Merge className="h-3.5 w-3.5" strokeWidth={1.8} />
        接入新节点
      </button>
    </div>
  );
}
