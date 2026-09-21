import { useRef, useState } from "react";
import { Group as GroupIcon, Ungroup } from "lucide-react";
import type { Viewport } from "@xyflow/react";
import type { WorkflowNode } from "../types";
import {
  GROUP_COLORS,
  boundsOf,
  groupBox,
  groupIdsOf,
  soleGroupOfSelected,
  type GroupBox,
} from "../utils/nodeGroups";

const SURFACE = "rgba(24,24,27,0.92)";
const TOOLBAR_GAP = 40;

interface Props {
  nodes: WorkflowNode[];
  viewport: Viewport;
  onGroup: () => void;
  onUngroup: (groupId: string) => void;
  onRename: (groupId: string, name: string) => void;
  onColor: (groupId: string, color: string) => void;
  onMove: (groupId: string, dx: number, dy: number) => void;
  /** 整组拖动前压一次历史；其余动作在 store 内部已各自压一次。 */
  onBeforeChange: () => void;
}

/**
 * 分组与多选的覆盖层：组框按成员包围盒每帧现算，工具栏浮在选区上方。
 *
 * 覆盖层本体不吃指针事件，只有组标题条和工具栏按钮可以点，
 * 免得挡住框内节点的选中与拖拽。
 */
export function SelectionLayer({
  nodes,
  viewport,
  onGroup,
  onUngroup,
  onRename,
  onColor,
  onMove,
  onBeforeChange,
}: Props) {
  const { zoom } = viewport;
  const dragRef = useRef<{ groupId: string; lastX: number; lastY: number } | null>(null);
  const [editing, setEditing] = useState<{ groupId: string; value: string } | null>(null);

  const toScreen = (box: GroupBox) => ({
    left: box.x * zoom + viewport.x,
    top: box.y * zoom + viewport.y,
    width: box.w * zoom,
    height: box.h * zoom,
  });

  const selectedIds = nodes.filter((node) => node.selected).map((node) => node.id);
  const selectedSet = new Set(selectedIds);
  const groupMode = selectedIds.length >= 2 ? soleGroupOfSelected(nodes, selectedIds) : null;
  const toolbarBox = groupMode
    ? groupBox(nodes, groupMode)
    : selectedIds.length >= 2
      ? boundsOf(nodes.filter((node) => selectedSet.has(node.id)))
      : null;

  const handleDragMove = (event: React.PointerEvent) => {
    const drag = dragRef.current;
    if (!drag) return;
    const dx = (event.clientX - drag.lastX) / zoom;
    const dy = (event.clientY - drag.lastY) / zoom;
    if (!dx && !dy) return;
    drag.lastX = event.clientX;
    drag.lastY = event.clientY;
    onMove(drag.groupId, dx, dy);
  };

  const commitRename = () => {
    if (!editing) return;
    const { groupId, value } = editing;
    setEditing(null);
    onRename(groupId, value);
  };

  return (
    <div className="pointer-events-none absolute inset-0 z-[5] overflow-hidden">
      {groupIdsOf(nodes).map((groupId) => {
        const box = groupBox(nodes, groupId);
        if (!box) return null;
        const screen = toScreen(box);
        const color = nodes.find((node) => node.groupId === groupId)?.groupColor ?? GROUP_COLORS[0];
        const name = nodes.find((node) => node.groupId === groupId)?.groupName ?? "分组";
        const isEditing = editing?.groupId === groupId;
        return (
          <div
            key={groupId}
            className="absolute rounded-[20px]"
            style={{ ...screen, border: `1px solid ${color}66`, boxShadow: `inset 0 0 0 1px ${color}14` }}
          >
            <div
              className="pointer-events-auto absolute flex -translate-y-full items-center gap-1.5 rounded-full px-2.5 py-1"
              style={{ left: 0, top: -6, background: SURFACE, backdropFilter: "blur(20px)", cursor: "grab" }}
              onPointerDown={(event) => {
                event.stopPropagation();
                event.currentTarget.setPointerCapture(event.pointerId);
                onBeforeChange();
                dragRef.current = { groupId, lastX: event.clientX, lastY: event.clientY };
              }}
              onPointerMove={handleDragMove}
              onPointerUp={() => {
                dragRef.current = null;
              }}
              onDoubleClick={(event) => {
                event.stopPropagation();
                setEditing({ groupId, value: name });
              }}
            >
              {isEditing ? (
                <input
                  autoFocus
                  value={editing.value}
                  onChange={(event) => setEditing({ groupId, value: event.target.value })}
                  onBlur={commitRename}
                  onKeyDown={(event) => {
                    event.stopPropagation();
                    if (event.key === "Enter") commitRename();
                    if (event.key === "Escape") setEditing(null);
                  }}
                  className="w-[120px] bg-transparent text-[11px] font-medium text-white/85 outline-none"
                />
              ) : (
                <>
                  <span className="h-1.5 w-1.5 rounded-full" style={{ background: color }} />
                  <span className="text-[11px] font-medium" style={{ color: "rgba(255,255,255,0.8)" }}>
                    {name}
                  </span>
                </>
              )}
            </div>
          </div>
        );
      })}

      {toolbarBox && !editing && (
        <div
          className="pointer-events-auto absolute flex -translate-x-1/2 items-center gap-1 rounded-xl px-1.5 py-1"
          style={{
            left: toScreen(toolbarBox).left + toScreen(toolbarBox).width / 2,
            top: Math.max(8, toScreen(toolbarBox).top - TOOLBAR_GAP),
            background: SURFACE,
            border: "1px solid rgba(255,255,255,0.08)",
            backdropFilter: "blur(20px)",
            boxShadow: "0 10px 30px rgba(0,0,0,0.35)",
          }}
          onPointerDown={(event) => event.stopPropagation()}
          onMouseDown={(event) => event.stopPropagation()}
        >
          <span className="px-1.5 text-[11px]" style={{ color: "rgba(255,255,255,0.45)" }}>
            {groupMode ? "已选整组" : `已选 ${selectedIds.length} 个节点`}
          </span>
          {groupMode ? (
            <button
              type="button"
              onClick={() => onUngroup(groupMode)}
              className="flex h-7 items-center gap-1.5 rounded-lg px-2 text-[11px] text-white/75 transition-colors hover:bg-white/10"
              title="取消打组（节点保留）"
            >
              <Ungroup className="h-3.5 w-3.5" strokeWidth={1.8} />
              解组
            </button>
          ) : (
            <button
              type="button"
              onClick={onGroup}
              className="flex h-7 items-center gap-1.5 rounded-lg px-2 text-[11px] text-white/75 transition-colors hover:bg-white/10"
              title="把选中的节点打成一个组"
            >
              <GroupIcon className="h-3.5 w-3.5" strokeWidth={1.8} />
              打组
            </button>
          )}
          {groupMode && (
            <div className="flex items-center gap-1 px-1">
              {GROUP_COLORS.map((color) => (
                <button
                  key={color}
                  type="button"
                  onClick={() => onColor(groupMode, color)}
                  className="h-4 w-4 rounded-full transition-transform hover:scale-110"
                  style={{
                    background: color,
                    boxShadow: nodes.find((node) => node.groupId === groupMode)?.groupColor === color
                      ? "0 0 0 2px rgba(255,255,255,0.7)"
                      : "none",
                  }}
                  title="组色"
                />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
