import { create } from "zustand";
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  type Connection,
  type EdgeChange,
  type NodeChange,
} from "@xyflow/react";
import type {
  NodeAlignMode,
  ProfessionalNodeData,
  ProfessionalNodeKind,
  WorkflowEdge,
  WorkflowNode,
} from "../types";
import { createDefaultNodeData, normalizeProfessionalNode } from "../utils/nodeRegistry";

const SNAP_GRID = 24;

interface ProfessionalState {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];

  onNodesChange: (changes: NodeChange[]) => void;
  onEdgesChange: (changes: EdgeChange[]) => void;
  onConnect: (connection: Connection) => void;
  /** 一次建立多条连线（句柄留空，与手动拉线一致）：一条历史、重复连线跳过。 */
  connectMany: (connections: Array<{ source: string; target: string }>) => void;

  addNode: (kind: ProfessionalNodeKind, x: number, y: number) => string;
  addNodeAt: (kind: ProfessionalNodeKind, x: number, y: number, data: Partial<ProfessionalNodeData>) => string;
  insertFragment: (fragment: { nodes: WorkflowNode[]; edges: WorkflowEdge[] }) => void;
  removeNode: (id: string) => void;
  updateNodeData: (id: string, data: Partial<ProfessionalNodeData>) => void;
  setNodes: (nodes: WorkflowNode[]) => void;
  setEdges: (edges: WorkflowEdge[]) => void;
  setSelectedIds: (ids: string[]) => void;
  alignNodes: (mode: NodeAlignMode) => void;

  past: Array<{ nodes: WorkflowNode[]; edges: WorkflowEdge[] }>;
  future: Array<{ nodes: WorkflowNode[]; edges: WorkflowEdge[] }>;
  pushHistory: () => void;
  undo: () => void;
  redo: () => void;
}

let _nodeIdCounter = 0;
// 计数器重载归零，而已持久化节点 id 仍在——必须跳过已占用 id，否则新建节点与旧节点同 id 互相覆盖
function nextNodeId(existingIds: string[]): string {
  const taken = new Set(existingIds);
  let id: string;
  do {
    id = `pro-node-${++_nodeIdCounter}`;
  } while (taken.has(id));
  return id;
}

// 拖动过程 position change 频次远高于帧率，按 rAF 批处理并按 id 取最后一次位置
let pendingPosChanges: import("@xyflow/react").NodeChange[] = [];
let posFrame: number | null = null;

export const useProfessionalStore = create<ProfessionalState>((set, get) => ({
  nodes: [],
  edges: [],
  past: [],
  future: [],

  onNodesChange: (changes) => {
    const nonPos: import("@xyflow/react").NodeChange[] = [];
    for (const c of changes) {
      if (c.type === "position") pendingPosChanges.push(c);
      else nonPos.push(c);
    }
    if (nonPos.length > 0) {
      set((s) => ({ nodes: applyNodeChanges(nonPos, s.nodes) as WorkflowNode[] }));
    }
    if (pendingPosChanges.length > 0 && posFrame === null) {
      posFrame = requestAnimationFrame(() => {
        posFrame = null;
        const batch = pendingPosChanges;
        pendingPosChanges = [];
        if (batch.length === 0) return;
        const lastById = new Map<string, import("@xyflow/react").NodeChange>();
        for (const c of batch) {
          if (c.type === "position") lastById.set(c.id, c);
        }
        set((s) => ({ nodes: applyNodeChanges(Array.from(lastById.values()), s.nodes) as WorkflowNode[] }));
      });
    }
  },

  onEdgesChange: (changes) => {
    set((s) => ({ edges: applyEdgeChanges(changes, s.edges) as WorkflowEdge[] }));
  },

  onConnect: (connection) => {
    get().pushHistory();
    set((s) => ({ edges: addEdge(connection, s.edges) as WorkflowEdge[] }));
  },

  addNode: (kind, x, y) => {
    get().pushHistory();
    const id = nextNodeId(get().nodes.map((n: WorkflowNode) => n.id));
    const data = createDefaultNodeData(kind);
    const snappedX = Math.round(x / SNAP_GRID) * SNAP_GRID;
    const snappedY = Math.round(y / SNAP_GRID) * SNAP_GRID;
    const node: WorkflowNode = {
      id,
      type: `professional-${kind}`,
      position: { x: snappedX, y: snappedY },
      data,
    };
    set((s) => ({ nodes: [...s.nodes, node] }));
    return id;
  },

  addNodeAt: (kind, x, y, extraData) => {
    get().pushHistory();
    const id = nextNodeId(get().nodes.map((n: WorkflowNode) => n.id));
    const base = createDefaultNodeData(kind);
    const snappedX = Math.round(x / SNAP_GRID) * SNAP_GRID;
    const snappedY = Math.round(y / SNAP_GRID) * SNAP_GRID;
    const node: WorkflowNode = {
      id,
      type: `professional-${kind}`,
      position: { x: snappedX, y: snappedY },
      data: { ...base, ...extraData, kind, status: "idle" } as ProfessionalNodeData,
    };
    set((s) => ({ nodes: [...s.nodes, node] }));
    return id;
  },

  insertFragment: (fragment) => {
    // Clone before committing so neither snippet edits nor callers can mutate history.
    const inserted = structuredClone(fragment);
    if (!inserted.nodes.length) throw new Error("片段没有可插入的节点。");
    const nodes = inserted.nodes.map(normalizeProfessionalNode);
    set((s) => {
      const taken = new Set([...s.nodes, ...s.edges].map((item) => item.id));
      const nodeIds = new Set(nodes.map((node) => node.id));
      for (const item of [...nodes, ...inserted.edges]) {
        if (!item.id || taken.has(item.id)) throw new Error("片段 ID 冲突，请重新插入。");
        taken.add(item.id);
      }
      if (inserted.edges.some((edge) => !nodeIds.has(edge.source) || !nodeIds.has(edge.target))
        || nodes.some((node) => node.parentId !== undefined && !nodeIds.has(node.parentId))) {
        throw new Error("片段包含外部连线或父节点，请完整选择关联节点。");
      }
      return {
        nodes: [...s.nodes.map((node) => node.selected ? { ...node, selected: false } : node), ...nodes],
        edges: [...s.edges, ...inserted.edges],
        past: [...s.past.slice(-99), { nodes: s.nodes, edges: s.edges }],
        future: [],
      };
    });
  },

  connectMany: (connections) => {
    const wanted = connections.filter(
      (c) => !get().edges.some((edge) => edge.source === c.source && edge.target === c.target),
    );
    if (!wanted.length) return;
    get().pushHistory();
    set((s) => ({
      edges: wanted.reduce(
        (acc, c) => addEdge({ ...c, sourceHandle: null, targetHandle: null }, acc),
        s.edges,
      ) as WorkflowEdge[],
    }));
  },

  removeNode: (id) => {
    get().pushHistory();
    set((s) => ({
      nodes: s.nodes.filter((n) => n.id !== id),
      edges: s.edges.filter((e) => e.source !== id && e.target !== id),
    }));
  },

  updateNodeData: (id, data) => {
    set((s) => ({
      nodes: s.nodes.map((n) =>
        n.id === id ? { ...n, data: { ...n.data, ...data } } : n,
      ) as WorkflowNode[],
    }));
  },

  setNodes: (nodes) => {
    set({ nodes: nodes.map(normalizeProfessionalNode) });
  },

  setEdges: (edges) => {
    set({ edges });
  },

  setSelectedIds: (ids) => {
    const idSet = new Set(ids);
    set((s) => ({
      nodes: s.nodes.map((n) => ({ ...n, selected: idSet.has(n.id) })),
    }));
  },

  alignNodes: (mode) => {
    const state = get();
    const selected = state.nodes.filter((n) => n.selected);
    if (selected.length < 2) return;

    const metrics = selected.map((n) => {
      const width = n.measured?.width ?? n.width ?? 0;
      const height = n.measured?.height ?? n.height ?? 0;
      return { id: n.id, width, height, left: n.position.x, top: n.position.y };
    });
    const bounds = {
      left: Math.min(...metrics.map((m) => m.left)),
      right: Math.max(...metrics.map((m) => m.left + m.width)),
      top: Math.min(...metrics.map((m) => m.top)),
      bottom: Math.max(...metrics.map((m) => m.top + m.height)),
    };
    const centerX = (bounds.left + bounds.right) / 2;
    const centerY = (bounds.top + bounds.bottom) / 2;
    const metricById = new Map(metrics.map((m) => [m.id, m]));

    state.pushHistory();
    set((s) => ({
      nodes: s.nodes.map((n) => {
        const m = metricById.get(n.id);
        if (!m) return n;
        let x = n.position.x;
        let y = n.position.y;
        switch (mode) {
          case "left":
            x = bounds.left;
            break;
          case "center-x":
            x = centerX - m.width / 2;
            break;
          case "right":
            x = bounds.right - m.width;
            break;
          case "top":
            y = bounds.top;
            break;
          case "center-y":
            y = centerY - m.height / 2;
            break;
          case "bottom":
            y = bounds.bottom - m.height;
            break;
        }
        return { ...n, position: { x, y } };
      }),
    }));
  },

  pushHistory: () => {
    set((s) => ({
      past: [...s.past.slice(-99), { nodes: s.nodes, edges: s.edges }],
      future: [],
    }));
  },

  undo: () => {
    set((s) => {
      if (s.past.length === 0) return s;
      const prev = s.past[s.past.length - 1];
      return {
        nodes: prev.nodes,
        edges: prev.edges,
        past: s.past.slice(0, -1),
        future: [{ nodes: s.nodes, edges: s.edges }, ...s.future],
      };
    });
  },

  redo: () => {
    set((s) => {
      if (s.future.length === 0) return s;
      const next = s.future[0];
      return {
        nodes: next.nodes,
        edges: next.edges,
        past: [...s.past, { nodes: s.nodes, edges: s.edges }],
        future: s.future.slice(1),
      };
    });
  },
}));
