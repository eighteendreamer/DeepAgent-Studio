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
  CreativeNodeData,
  CreativeNodeKind,
  NodeAlignMode,
  WorkflowEdge,
  WorkflowNode,
} from "../types";
import { canvasPromptProfilePatch } from "../utils/canvasPromptProfile";

const SNAP_GRID = 24;

/**
 * Bind a freshly created node to its backend prompt profile: keep the profile
 * reference, and while the user text is still empty show the profile's default
 * prompt. An already edited prompt is never overwritten.
 */
async function bindPromptProfile(
  nodeId: string,
  kind: CreativeNodeKind,
  data: CreativeNodeData,
): Promise<void> {
  const patchData = await canvasPromptProfilePatch(kind, data);
  if (!patchData) return;
  useCreativeStore.getState().updateNodeData(nodeId, patchData);
}

function createDefaultCreativeData(kind: CreativeNodeKind): CreativeNodeData {
  const base: CreativeNodeData = { label: "", kind, status: "idle" };
  switch (kind) {
    case "category-picker":
      return { ...base, label: "选择节点类型" };
    case "text-gen":
      return { ...base, label: "文本生成", prompt: "" };
    case "image-input":
      return { ...base, label: "图片输入", imageUrl: "" };
    case "image-gen":
      return { ...base, label: "图片生成", imageModel: "", imagePrompt: "", aspectRatio: "1:1", size: "1024x1024" };
    case "image-compare":
      return { ...base, label: "图片对比" };
    case "image-edit":
      return { ...base, label: "图片编辑", editMode: "crop" };
    case "script-gen":
      return { ...base, label: "脚本生成", prompt: "" };
    case "video-gen":
      return { ...base, label: "视频生成", videoModel: "", videoPrompt: "" };
    case "video-stitch":
      return { ...base, label: "视频拼接" };
    case "camera":
      return { ...base, label: "摄像机" };
    case "lens":
      return { ...base, label: "镜头" };
    case "focal-length":
      return { ...base, label: "焦距" };
    case "aperture":
      return { ...base, label: "光圈" };
    case "director":
      return { ...base, label: "微表情导演" };
    case "creative-template":
      return { ...base, label: "创意模板" };
    case "character-face":
      return { ...base, label: "角色工作室 · 面部" };
    case "character-body":
      return { ...base, label: "角色工作室 · 身体" };
    case "character-style":
      return { ...base, label: "角色工作室 · 风格" };
    case "audio":
      // 契约要求显式方向，创建时就写死默认值，界面选中态与配置保持一致。
      return {
        ...base,
        label: "音频",
        audioOperation: "speech_transcribe",
        audioFormat: "mp3",
      };
    case "storyboard-grid":
      return { ...base, label: "分镜格子" };
  }
}

interface CreativeState {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];

  onNodesChange: (changes: NodeChange[]) => void;
  onEdgesChange: (changes: EdgeChange[]) => void;
  onConnect: (connection: Connection) => void;
  /** 一次建立多条连线（句柄留空，与手动拉线一致）：一条历史、重复连线跳过。 */
  connectMany: (connections: Array<{ source: string; target: string }>) => void;

  addNode: (kind: CreativeNodeKind, x: number, y: number) => string;
  addNodeAt: (kind: CreativeNodeKind, x: number, y: number, data: Partial<CreativeNodeData>) => string;
  refineNode: (id: string, kind: CreativeNodeKind, data?: Partial<CreativeNodeData>) => void;
  createImageToPromptPair: (id: string, data?: Partial<CreativeNodeData>) => string | null;
  removeNode: (id: string) => void;
  updateNodeData: (id: string, data: Partial<CreativeNodeData>) => void;
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
    id = `creative-node-${++_nodeIdCounter}`;
  } while (taken.has(id));
  return id;
}

// 拖动过程 position change 频次远高于帧率，按 rAF 批处理并按 id 取最后一次位置；
// 选区/添加/删除/尺寸等其他 change 仍即时落库，避免点击反馈延迟
let pendingPosChanges: import("@xyflow/react").NodeChange[] = [];
let posFrame: number | null = null;

export const useCreativeStore = create<CreativeState>((set, get) => ({
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
    void bindPromptProfile(id, kind, data);
    return id;
  },

  addNodeAt: (kind, x, y, extraData) => {
    get().pushHistory();
    const id = nextNodeId(get().nodes.map((n: WorkflowNode) => n.id));
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
    void bindPromptProfile(id, kind, node.data as CreativeNodeData);
    return id;
  },

  refineNode: (id, kind, extraData = {}) => {
    get().pushHistory();
    set((s) => ({
      nodes: s.nodes.map((node) => {
        if (node.id !== id) return node;
        const base = createDefaultCreativeData(kind);
        return {
          ...node,
          type: `creative-${kind}`,
          data: { ...base, ...extraData, status: "idle" } as CreativeNodeData,
        };
      }),
    }));
  },

  createImageToPromptPair: (id, extraData = {}) => {
    const state = get();
    const outputNode = state.nodes.find((node) => node.id === id);
    if (!outputNode) return null;

    // 一次历史记录完成复合节点创建，撤销时不会留下半成品节点或孤立连线。
    state.pushHistory();
    const inputId = nextNodeId(state.nodes.map((node) => node.id));
    const outputBase = createDefaultCreativeData("text-gen");
    const inputBase = createDefaultCreativeData("image-input");
    const inputNode: WorkflowNode = {
      id: inputId,
      type: "creative-image-input",
      position: { x: outputNode.position.x - 288, y: outputNode.position.y },
      data: {
        ...inputBase,
        label: "图片输入",
        creativeCategory: "文本",
        creativeCategoryKey: "text",
        creativeAction: "图片反推提示词",
        creativeActionKey: "image-to-prompt-input",
        status: "idle",
      },
    };
    const outputNodeNext: WorkflowNode = {
      ...outputNode,
      type: "creative-text-gen",
      data: {
        ...outputBase,
        ...extraData,
        label: "图片反推提示词",
        creativeCategory: "文本",
        creativeCategoryKey: "text",
        creativeAction: "图片反推提示词",
        creativeActionKey: "image-to-prompt",
        status: "idle",
      } as CreativeNodeData,
    };
    set((current) => ({
      nodes: current.nodes.map((node) => (node.id === id ? outputNodeNext : node)).concat(inputNode),
      edges: addEdge({ source: inputId, target: id, sourceHandle: null, targetHandle: null }, current.edges) as WorkflowEdge[],
    }));
    return inputId;
  },

  connectMany: (connections) => {
    const wanted = connections.filter(
      (c) =>
        !get().edges.some((edge) => edge.source === c.source && edge.target === c.target),
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
    set({ nodes });
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
