import { create } from "zustand";
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  type Connection,
  type EdgeChange,
  type NodeChange,
} from "@xyflow/react";
import type { CreativeNodeData, CreativeNodeKind, WorkflowEdge, WorkflowNode } from "../types";

const SNAP_GRID = 24;

function createDefaultCreativeData(kind: CreativeNodeKind): CreativeNodeData {
  const base: CreativeNodeData = { label: "", kind, status: "idle" };
  switch (kind) {
    case "text-gen":
      return { ...base, label: "文本生成", prompt: "" };
    case "image-gen":
      return { ...base, label: "图片生成", imagePrompt: "", aspectRatio: "1:1" };
    case "image-compare":
      return { ...base, label: "图片对比" };
    case "image-edit":
      return { ...base, label: "图片编辑", editMode: "crop" };
    case "script-gen":
      return { ...base, label: "脚本生成", prompt: "" };
    case "video-gen":
      return { ...base, label: "视频生成", videoService: "sora", videoPrompt: "" };
    case "video-stitch":
      return { ...base, label: "视频拼接" };
  }
}

interface CreativeState {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];

  onNodesChange: (changes: NodeChange[]) => void;
  onEdgesChange: (changes: EdgeChange[]) => void;
  onConnect: (connection: Connection) => void;

  addNode: (kind: CreativeNodeKind, x: number, y: number) => string;
  addNodeAt: (kind: CreativeNodeKind, x: number, y: number, data: Partial<CreativeNodeData>) => string;
  removeNode: (id: string) => void;
  updateNodeData: (id: string, data: Partial<CreativeNodeData>) => void;
  insertNodeBetween: (edgeId: string, kind: CreativeNodeKind) => void;
  setSelectedIds: (ids: string[]) => void;

  past: Array<{ nodes: WorkflowNode[]; edges: WorkflowEdge[] }>;
  future: Array<{ nodes: WorkflowNode[]; edges: WorkflowEdge[] }>;
  pushHistory: () => void;
  undo: () => void;
  redo: () => void;
}

let _nodeIdCounter = 0;
function nextNodeId() {
  return `creative-node-${++_nodeIdCounter}`;
}

export const useCreativeStore = create<CreativeState>((set, get) => ({
  nodes: [],
  edges: [],
  past: [],
  future: [],

  onNodesChange: (changes) => {
    set((s) => ({ nodes: applyNodeChanges(changes, s.nodes) as WorkflowNode[] }));
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
    const id = nextNodeId();
    const data = createDefaultCreativeData(kind);
    const snappedX = Math.round(x / SNAP_GRID) * SNAP_GRID;
    const snappedY = Math.round(y / SNAP_GRID) * SNAP_GRID;
    const node: WorkflowNode = {
      id,
      type: `creative-${kind}`,
      position: { x: snappedX, y: snappedY },
      data,
    };
    set((s) => ({ nodes: [...s.nodes, node] }));
    return id;
  },

  addNodeAt: (kind, x, y, extraData) => {
    get().pushHistory();
    const id = nextNodeId();
    const base = createDefaultCreativeData(kind);
    const snappedX = Math.round(x / SNAP_GRID) * SNAP_GRID;
    const snappedY = Math.round(y / SNAP_GRID) * SNAP_GRID;
    const node: WorkflowNode = {
      id,
      type: `creative-${kind}`,
      position: { x: snappedX, y: snappedY },
      data: { ...base, ...extraData, status: "idle" } as CreativeNodeData,
    };
    set((s) => ({ nodes: [...s.nodes, node] }));
    return id;
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
      ),
    }));
  },

  insertNodeBetween: (edgeId, kind) => {
    const state = get();
    const edge = state.edges.find((e) => e.id === edgeId);
    if (!edge || !edge.source || !edge.target) return;

    const sourceNode = state.nodes.find((n) => n.id === edge.source);
    const targetNode = state.nodes.find((n) => n.id === edge.target);
    if (!sourceNode || !targetNode) return;

    const midX = (sourceNode.position.x + targetNode.position.x) / 2;
    const midY = (sourceNode.position.y + targetNode.position.y) / 2;

    state.pushHistory();
    const newId = nextNodeId();
    const newData = createDefaultCreativeData(kind);
    const snappedX = Math.round(midX / SNAP_GRID) * SNAP_GRID;
    const snappedY = Math.round(midY / SNAP_GRID) * SNAP_GRID;
    const newNode: WorkflowNode = {
      id: newId,
      type: `creative-${kind}`,
      position: { x: snappedX, y: snappedY },
      data: newData,
    };

    set((s) => ({
      nodes: [...s.nodes, newNode],
      edges: [
        ...s.edges.filter((e) => e.id !== edgeId),
        { id: `e-${edge.source}-${newId}`, source: edge.source, target: newId } as WorkflowEdge,
        { id: `e-${newId}-${edge.target}`, source: newId, target: edge.target } as WorkflowEdge,
      ],
    }));
  },

  setSelectedIds: (ids) => {
    const idSet = new Set(ids);
    set((s) => ({
      nodes: s.nodes.map((n) => ({ ...n, selected: idSet.has(n.id) })),
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
