import type { WorkflowNode } from "../types";

/**
 * 节点分组的纯逻辑。
 *
 * 组不是节点、也不是 data 字段，而是成员节点共享的一组标记（groupId/groupName/
 * groupColor），组框每帧按成员包围盒现算，所以不存在"组尺寸过期"的问题。
 * 因为字段挂在节点顶层，`serializeNodes` 只读 node.id/kind/data，组永远不会
 * 被送进内核执行图。
 */

/** 组框相对成员的外扩边距。 */
export const GROUP_FRAME_PADDING = 12;

/** 节点尚未被 React Flow 测量时的兜底尺寸，与 dagre 布局默认值一致。 */
const FALLBACK_WIDTH = 240;
const FALLBACK_HEIGHT = 93;

export const GROUP_COLORS = ["#8b5cf6", "#3b82f6", "#10b981", "#f59e0b", "#ef4444", "#ec4899"];

let _groupCounter = 0;

export function nextGroupId(): string {
  _groupCounter += 1;
  return `group-${Date.now().toString(36)}-${_groupCounter}`;
}

export interface GroupBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 分组动作；两个模式的画布 store 共用同一套语义。 */
export interface GroupActions {
  /** 把当前选中的 ≥2 个节点打成一个组，返回新组 id；不足 2 个返回空串。 */
  groupSelected: () => string;
  ungroup: (groupId: string) => void;
  renameGroup: (groupId: string, name: string) => void;
  colorGroup: (groupId: string, color: string) => void;
  /** 整组平移。历史由调用方在拖起时压一次，避免每帧落一条。 */
  moveGroup: (groupId: string, dx: number, dy: number) => void;
}

function nodeSize(node: WorkflowNode): { w: number; h: number } {
  return {
    w: node.measured?.width ?? (node.width as number | undefined) ?? FALLBACK_WIDTH,
    h: node.measured?.height ?? (node.height as number | undefined) ?? FALLBACK_HEIGHT,
  };
}

export function membersOf(nodes: WorkflowNode[], groupId: string): WorkflowNode[] {
  return nodes.filter((node) => node.groupId === groupId);
}

export function groupIdsOf(nodes: WorkflowNode[]): string[] {
  const ids: string[] = [];
  for (const node of nodes) {
    if (node.groupId && !ids.includes(node.groupId)) ids.push(node.groupId);
  }
  return ids;
}

/** 任意一组节点的包围盒 + 外扩。 */
export function boundsOf(members: WorkflowNode[], padding = GROUP_FRAME_PADDING): GroupBox | null {
  if (!members.length) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const member of members) {
    const { w, h } = nodeSize(member);
    left = Math.min(left, member.position.x);
    top = Math.min(top, member.position.y);
    right = Math.max(right, member.position.x + w);
    bottom = Math.max(bottom, member.position.y + h);
  }
  return {
    x: left - padding,
    y: top - padding,
    w: right - left + padding * 2,
    h: bottom - top + padding * 2,
  };
}

/** 组框：成员包围盒 + 外扩，没有成员就没有框。 */
export function groupBox(
  nodes: WorkflowNode[],
  groupId: string,
  padding = GROUP_FRAME_PADDING,
): GroupBox | null {
  return boundsOf(membersOf(nodes, groupId), padding);
}

export function withGroup(
  nodes: WorkflowNode[],
  ids: string[],
  groupId: string,
  groupName: string,
  groupColor: string,
): WorkflowNode[] {
  const picked = new Set(ids);
  return nodes.map((node) =>
    picked.has(node.id) ? { ...node, groupId, groupName, groupColor } : node,
  );
}

export function withoutGroup(nodes: WorkflowNode[], groupId: string): WorkflowNode[] {
  return nodes.map((node) =>
    node.groupId === groupId
      ? { ...node, groupId: undefined, groupName: undefined, groupColor: undefined }
      : node,
  );
}

export function renamedGroup(nodes: WorkflowNode[], groupId: string, groupName: string): WorkflowNode[] {
  return nodes.map((node) => (node.groupId === groupId ? { ...node, groupName } : node));
}

export function coloredGroup(nodes: WorkflowNode[], groupId: string, groupColor: string): WorkflowNode[] {
  return nodes.map((node) => (node.groupId === groupId ? { ...node, groupColor } : node));
}

/** 整组平移：只动成员位置，组框由包围盒现算。 */
export function translateGroup(nodes: WorkflowNode[], groupId: string, dx: number, dy: number): WorkflowNode[] {
  if (!dx && !dy) return nodes;
  return nodes.map((node) =>
    node.groupId === groupId
      ? { ...node, position: { x: node.position.x + dx, y: node.position.y + dy } }
      : node,
  );
}

/** 选中的节点恰好是同一个组的全部成员时返回该组 id，否则返回 null。 */
export function soleGroupOfSelected(nodes: WorkflowNode[], selectedIds: string[]): string | null {
  const picked = new Set(selectedIds);
  let found: string | null = null;
  for (const node of nodes) {
    if (!picked.has(node.id)) continue;
    if (!node.groupId) return null;
    if (found && found !== node.groupId) return null;
    found = node.groupId;
  }
  if (!found) return null;
  return membersOf(nodes, found).every((member) => picked.has(member.id)) ? found : null;
}
