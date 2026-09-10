import dagre from "@dagrejs/dagre";
import type { Edge } from "@xyflow/react";
import type { WorkflowNode } from "../types";

export interface LayoutOptions {
  direction: "TB" | "LR"; // TB = top-bottom, LR = left-right
  nodeSpacing: number;
  rankSpacing: number;
}

const DEFAULT_OPTIONS: LayoutOptions = {
  direction: "TB",
  nodeSpacing: 80,
  rankSpacing: 120,
};

export function applyDagreLayout(
  nodes: WorkflowNode[],
  edges: Edge[],
  options: Partial<LayoutOptions> = {},
): WorkflowNode[] {
  const opts = { ...DEFAULT_OPTIONS, ...options };
  const g = new dagre.graphlib.Graph();

  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({
    rankdir: opts.direction,
    nodesep: opts.nodeSpacing,
    ranksep: opts.rankSpacing,
  });

  // Add nodes with estimated dimensions
  for (const node of nodes) {
    const width = node.measured?.width ?? 240;
    const height = node.measured?.height ?? 93;
    g.setNode(node.id, { width, height });
  }

  // Add edges
  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  // Run layout
  dagre.layout(g);

  // Apply new positions
  return nodes.map((node) => {
    const pos = g.node(node.id);
    if (!pos) return node;
    return {
      ...node,
      position: {
        x: pos.x - (node.measured?.width ?? 240) / 2,
        y: pos.y - (node.measured?.height ?? 93) / 2,
      },
    };
  });
}
