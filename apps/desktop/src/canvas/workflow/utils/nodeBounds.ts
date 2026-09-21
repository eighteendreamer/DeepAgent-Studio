import type { WorkflowNode } from "../types";

/**
 * 节点包围盒。
 *
 * 尺寸优先取 React Flow 实测值；未测量时回落到与 dagre 布局一致的默认值，
 * 否则刚创建的节点会让包围盒塌成一点。
 */

const FALLBACK_WIDTH = 240;
const FALLBACK_HEIGHT = 93;

export interface NodeBounds {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export function nodeSize(node: WorkflowNode): { w: number; h: number } {
  return {
    w: node.measured?.width ?? (node.width as number | undefined) ?? FALLBACK_WIDTH,
    h: node.measured?.height ?? (node.height as number | undefined) ?? FALLBACK_HEIGHT,
  };
}

export function boundsOfNodes(nodes: WorkflowNode[]): NodeBounds | null {
  if (!nodes.length) return null;
  let left = Infinity;
  let top = Infinity;
  let right = -Infinity;
  let bottom = -Infinity;
  for (const node of nodes) {
    const { w, h } = nodeSize(node);
    left = Math.min(left, node.position.x);
    top = Math.min(top, node.position.y);
    right = Math.max(right, node.position.x + w);
    bottom = Math.max(bottom, node.position.y + h);
  }
  return { left, top, right, bottom };
}
