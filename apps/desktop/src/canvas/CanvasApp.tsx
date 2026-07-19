import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  faArrowPointer,
  faCircle,
  faDownload,
  faDrawPolygon,
  faEllipsis,
  faEraser,
  faFont,
  faGripVertical,
  faHand,
  faMagnifyingGlass,
  faNoteSticky,
  faPencil,
  faRotateLeft,
  faRotateRight,
  faSquare,
} from "@fortawesome/free-solid-svg-icons";
import dagre from "@dagrejs/dagre";
import { toPng } from "html-to-image";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MiniMap,
  NodeResizer,
  NodeToolbar,
  Panel,
  Position,
  ReactFlow,
  ReactFlowProvider,
  reconnectEdge,
  SelectionMode,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeProps,
  type ReactFlowInstance,
  type Viewport,
  type XYPosition,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { isTauri } from "../api";
import { CanvasTitleBar } from "./CanvasTitleBar";

const LEGACY_CANVAS_LAYOUT_KEY = "deepagent:studio-canvas-layout:v1";
const CANVAS_VIEWPORT_KEY = "deepagent:studio-canvas-viewport:v1";
const CANVAS_DOCUMENT_KEY = "deepagent:studio-canvas-document:v1";
const DEFAULT_VIEWPORT: Viewport = { x: 0, y: 0, zoom: 1 };
const SNAP_GRID: [number, number] = [24, 24];
const HISTORY_LIMIT = 100;

type CanvasTool = "select" | "pan" | "rectangle" | "draw" | "lasso" | "eraser";
type CanvasNodeKind = "note" | "text" | "shape" | "drawing" | "group";
type ShapeKind = "rectangle" | "ellipse";

type CanvasNodeData = {
  title: string;
  body: string;
  color?: string;
  locked?: boolean;
  shape?: ShapeKind;
  points?: XYPosition[];
};

type CanvasNode = Node<CanvasNodeData, CanvasNodeKind>;
type CanvasEdge = Edge<{ label?: string }>;
type CanvasSnapshot = { nodes: CanvasNode[]; edges: CanvasEdge[] };
type ClipboardPayload = CanvasSnapshot;
type ContextMenuState = { x: number; y: number; nodeId?: string; edgeId?: string } | null;
type DrawDraft =
  | { kind: "rectangle"; start: XYPosition; current: XYPosition }
  | { kind: "draw"; points: XYPosition[] }
  | { kind: "lasso"; points: XYPosition[] }
  | { kind: "eraser"; points: XYPosition[]; nodeIds: string[]; edgeIds: string[] }
  | null;

function cloneSnapshot(snapshot: CanvasSnapshot): CanvasSnapshot {
  return structuredClone(snapshot);
}

function snapshotsMatch(left: CanvasSnapshot, right: CanvasSnapshot) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function readViewport(): Viewport {
  try {
    const saved = window.localStorage.getItem(CANVAS_VIEWPORT_KEY);
    if (!saved) return DEFAULT_VIEWPORT;
    const viewport = JSON.parse(saved) as Partial<Viewport>;
    if (
      typeof viewport.x === "number" &&
      typeof viewport.y === "number" &&
      typeof viewport.zoom === "number"
    ) {
      return {
        x: viewport.x,
        y: viewport.y,
        zoom: Math.min(4, Math.max(0.1, viewport.zoom)),
      };
    }
  } catch {
    window.localStorage.removeItem(CANVAS_VIEWPORT_KEY);
  }
  return DEFAULT_VIEWPORT;
}

function readDocument(): CanvasSnapshot {
  try {
    const saved = window.localStorage.getItem(CANVAS_DOCUMENT_KEY);
    if (!saved) return { nodes: [], edges: [] };
    const document = JSON.parse(saved) as Partial<CanvasSnapshot>;
    if (Array.isArray(document.nodes) && Array.isArray(document.edges)) {
      return { nodes: document.nodes as CanvasNode[], edges: document.edges as CanvasEdge[] };
    }
  } catch {
    window.localStorage.removeItem(CANVAS_DOCUMENT_KEY);
  }
  return { nodes: [], edges: [] };
}

function isEditableTarget(target: EventTarget | null) {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

function downloadBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.style.display = "none";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function nodeSize(node: CanvasNode) {
  const styleWidth = typeof node.style?.width === "number" ? node.style.width : Number(node.style?.width);
  const styleHeight = typeof node.style?.height === "number" ? node.style.height : Number(node.style?.height);
  return {
    width: node.measured?.width ?? (Number.isFinite(styleWidth) ? styleWidth : 240),
    height: node.measured?.height ?? (Number.isFinite(styleHeight) ? styleHeight : 160),
  };
}

function pointInPolygon(point: XYPosition, polygon: XYPosition[]) {
  let inside = false;
  for (let index = 0, previous = polygon.length - 1; index < polygon.length; previous = index++) {
    const currentPoint = polygon[index];
    const previousPoint = polygon[previous];
    const intersects =
      currentPoint.y > point.y !== previousPoint.y > point.y &&
      point.x <
        ((previousPoint.x - currentPoint.x) * (point.y - currentPoint.y)) /
          (previousPoint.y - currentPoint.y || Number.EPSILON) +
          currentPoint.x;
    if (intersects) inside = !inside;
  }
  return inside;
}

function InfiniteCanvas() {
  const initialViewport = useMemo(readViewport, []);
  const initialDocument = useMemo(readDocument, []);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const importInputRef = useRef<HTMLInputElement | null>(null);
  const instanceRef = useRef<ReactFlowInstance<CanvasNode, CanvasEdge> | null>(null);
  const [nodes, setNodes] = useState<CanvasNode[]>(initialDocument.nodes);
  const [edges, setEdges] = useState<CanvasEdge[]>(initialDocument.edges);
  const nodesRef = useRef(nodes);
  const edgesRef = useRef(edges);
  const clipboardRef = useRef<ClipboardPayload | null>(null);
  const pasteOffsetRef = useRef(0);
  const pastRef = useRef<CanvasSnapshot[]>([]);
  const futureRef = useRef<CanvasSnapshot[]>([]);
  const interactionStartRef = useRef<CanvasSnapshot | null>(null);
  const [historyStatus, setHistoryStatus] = useState({ canUndo: false, canRedo: false });
  const [tool, setTool] = useState<CanvasTool>("select");
  const [canvasLocked, setCanvasLocked] = useState(false);
  const [zoom, setZoom] = useState(initialViewport.zoom);
  const [isDark, setIsDark] = useState(() => document.documentElement.classList.contains("dark"));
  const [showMiniMap, setShowMiniMap] = useState(false);
  const [moreOpen, setMoreOpen] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [contextMenu, setContextMenu] = useState<ContextMenuState>(null);
  const [drawDraft, setDrawDraft] = useState<DrawDraft>(null);

  const currentSnapshot = useCallback(
    (): CanvasSnapshot => ({ nodes: nodesRef.current, edges: edgesRef.current }),
    [],
  );

  const replaceCanvas = useCallback((snapshot: CanvasSnapshot) => {
    nodesRef.current = snapshot.nodes;
    edgesRef.current = snapshot.edges;
    setNodes(snapshot.nodes);
    setEdges(snapshot.edges);
  }, []);

  const updateHistoryStatus = useCallback(() => {
    setHistoryStatus({ canUndo: pastRef.current.length > 0, canRedo: futureRef.current.length > 0 });
  }, []);

  const recordHistory = useCallback(
    (before: CanvasSnapshot, after: CanvasSnapshot) => {
      if (snapshotsMatch(before, after)) return;
      pastRef.current = [...pastRef.current.slice(-(HISTORY_LIMIT - 1)), cloneSnapshot(before)];
      futureRef.current = [];
      updateHistoryStatus();
    },
    [updateHistoryStatus],
  );

  const commitCanvas = useCallback(
    (next: CanvasSnapshot) => {
      const before = cloneSnapshot(currentSnapshot());
      replaceCanvas(next);
      recordHistory(before, next);
    },
    [currentSnapshot, recordHistory, replaceCanvas],
  );

  const beginInteraction = useCallback(() => {
    if (!interactionStartRef.current) interactionStartRef.current = cloneSnapshot(currentSnapshot());
  }, [currentSnapshot]);

  const finishInteraction = useCallback(() => {
    const before = interactionStartRef.current;
    interactionStartRef.current = null;
    if (before) recordHistory(before, currentSnapshot());
  }, [currentSnapshot, recordHistory]);

  const undo = useCallback(() => {
    const previous = pastRef.current[pastRef.current.length - 1];
    if (!previous) return;
    pastRef.current = pastRef.current.slice(0, -1);
    futureRef.current = [cloneSnapshot(currentSnapshot()), ...futureRef.current].slice(0, HISTORY_LIMIT);
    replaceCanvas(cloneSnapshot(previous));
    updateHistoryStatus();
  }, [currentSnapshot, replaceCanvas, updateHistoryStatus]);

  const redo = useCallback(() => {
    const next = futureRef.current[0];
    if (!next) return;
    futureRef.current = futureRef.current.slice(1);
    pastRef.current = [...pastRef.current, cloneSnapshot(currentSnapshot())].slice(-HISTORY_LIMIT);
    replaceCanvas(cloneSnapshot(next));
    updateHistoryStatus();
  }, [currentSnapshot, replaceCanvas, updateHistoryStatus]);

  const selectedNodes = useMemo(() => nodes.filter((node) => node.selected), [nodes]);
  const selectedEdges = useMemo(() => edges.filter((edge) => edge.selected), [edges]);

  const updateNodeData = useCallback((nodeId: string, patch: Partial<CanvasNodeData>) => {
    const nextNodes = nodesRef.current.map((node) =>
      node.id === nodeId ? { ...node, data: { ...node.data, ...patch } } : node,
    );
    nodesRef.current = nextNodes;
    setNodes(nextNodes);
  }, []);

  const deleteByIds = useCallback(
    (nodeIds: Set<string>, edgeIds = new Set<string>()) => {
      const expandedNodeIds = new Set(nodeIds);
      let foundDescendant = true;
      while (foundDescendant) {
        foundDescendant = false;
        nodesRef.current.forEach((node) => {
          if (node.parentId && expandedNodeIds.has(node.parentId) && !expandedNodeIds.has(node.id)) {
            expandedNodeIds.add(node.id);
            foundDescendant = true;
          }
        });
      }
      const nextNodes = nodesRef.current.filter((node) => !expandedNodeIds.has(node.id));
      const nextEdges = edgesRef.current.filter(
        (edge) => !edgeIds.has(edge.id) && !expandedNodeIds.has(edge.source) && !expandedNodeIds.has(edge.target),
      );
      if (nextNodes.length === nodesRef.current.length && nextEdges.length === edgesRef.current.length) return;
      commitCanvas({ nodes: nextNodes, edges: nextEdges });
    },
    [commitCanvas],
  );

  const deleteSelection = useCallback(() => {
    deleteByIds(
      new Set(nodesRef.current.filter((node) => node.selected && !node.data.locked).map((node) => node.id)),
      new Set(edgesRef.current.filter((edge) => edge.selected).map((edge) => edge.id)),
    );
  }, [deleteByIds]);

  const selectAll = useCallback(() => {
    const nextNodes = nodesRef.current.map((node) => ({ ...node, selected: true }));
    const nextEdges = edgesRef.current.map((edge) => ({ ...edge, selected: true }));
    replaceCanvas({ nodes: nextNodes, edges: nextEdges });
  }, [replaceCanvas]);

  const copySelection = useCallback(async () => {
    const selectedIds = new Set(nodesRef.current.filter((node) => node.selected).map((node) => node.id));
    if (!selectedIds.size) return null;
    let foundDescendant = true;
    while (foundDescendant) {
      foundDescendant = false;
      nodesRef.current.forEach((node) => {
        if (node.parentId && selectedIds.has(node.parentId) && !selectedIds.has(node.id)) {
          selectedIds.add(node.id);
          foundDescendant = true;
        }
      });
    }
    const payload: ClipboardPayload = {
      nodes: nodesRef.current
        .filter((node) => selectedIds.has(node.id))
        .map((node) => ({
          ...node,
          parentId: node.parentId && selectedIds.has(node.parentId) ? node.parentId : undefined,
          extent: node.parentId && selectedIds.has(node.parentId) ? node.extent : undefined,
          selected: false,
        })),
      edges: edgesRef.current.filter(
        (edge) => selectedIds.has(edge.source) && selectedIds.has(edge.target),
      ),
    };
    clipboardRef.current = cloneSnapshot(payload);
    pasteOffsetRef.current = 0;
    try {
      await navigator.clipboard.writeText(
        JSON.stringify({ type: "deepagent-studio-canvas", version: 1, ...payload }),
      );
    } catch {
      // Internal clipboard remains available when the system clipboard is unavailable.
    }
    return payload;
  }, []);

  const pasteClipboard = useCallback(async () => {
    let payload = clipboardRef.current;
    if (!payload) {
      try {
        const parsed = JSON.parse(await navigator.clipboard.readText()) as {
          type?: string;
          nodes?: CanvasNode[];
          edges?: CanvasEdge[];
        };
        if (parsed.type === "deepagent-studio-canvas" && Array.isArray(parsed.nodes) && Array.isArray(parsed.edges)) {
          payload = { nodes: parsed.nodes, edges: parsed.edges };
        }
      } catch {
        return;
      }
    }
    if (!payload?.nodes.length) return;
    pasteOffsetRef.current += 24;
    const idMap = new Map<string, string>();
    payload.nodes.forEach((node) => idMap.set(node.id, `${node.type}-${crypto.randomUUID()}`));
    const nextNodes = nodesRef.current.map((node) => ({ ...node, selected: false }));
    const pastedNodes = payload.nodes.map((node) => ({
      ...cloneSnapshot({ nodes: [node], edges: [] }).nodes[0],
      id: idMap.get(node.id)!,
      parentId: node.parentId ? idMap.get(node.parentId) : undefined,
      position: {
        x: node.position.x + (node.parentId ? 0 : pasteOffsetRef.current),
        y: node.position.y + (node.parentId ? 0 : pasteOffsetRef.current),
      },
      selected: true,
    }));
    const pastedEdges = payload.edges.map((edge) => ({
      ...edge,
      id: `edge-${crypto.randomUUID()}`,
      source: idMap.get(edge.source) ?? edge.source,
      target: idMap.get(edge.target) ?? edge.target,
      selected: false,
    }));
    commitCanvas({ nodes: [...nextNodes, ...pastedNodes], edges: [...edgesRef.current, ...pastedEdges] });
  }, [commitCanvas]);

  const duplicateSelection = useCallback(async () => {
    const copied = await copySelection();
    if (copied) await pasteClipboard();
  }, [copySelection, pasteClipboard]);

  const cutSelection = useCallback(async () => {
    const copied = await copySelection();
    if (copied) deleteSelection();
  }, [copySelection, deleteSelection]);

  const toggleSelectedLock = useCallback(() => {
    const selected = nodesRef.current.filter((node) => node.selected);
    if (!selected.length) return;
    const shouldLock = selected.some((node) => !node.data.locked);
    commitCanvas({
      nodes: nodesRef.current.map((node) =>
        node.selected
          ? { ...node, draggable: !shouldLock, connectable: !shouldLock, data: { ...node.data, locked: shouldLock } }
          : node,
      ),
      edges: edgesRef.current,
    });
  }, [commitCanvas]);

  const moveSelectionLayer = useCallback(
    (direction: "front" | "back") => {
      if (!nodesRef.current.some((node) => node.selected)) return;
      const zValues = nodesRef.current.map((node) => node.zIndex ?? 0);
      const zIndex = direction === "front" ? Math.max(0, ...zValues) + 1 : Math.min(0, ...zValues) - 1;
      commitCanvas({
        nodes: nodesRef.current.map((node) => (node.selected ? { ...node, zIndex } : node)),
        edges: edgesRef.current,
      });
    },
    [commitCanvas],
  );

  const alignSelection = useCallback(
    (alignment: "left" | "center" | "right" | "top" | "middle" | "bottom") => {
      const selected = nodesRef.current.filter((node) => node.selected && !node.parentId && !node.data.locked);
      if (selected.length < 2) return;
      const rects = selected.map((node) => ({ node, ...nodeSize(node) }));
      const left = Math.min(...rects.map(({ node }) => node.position.x));
      const right = Math.max(...rects.map(({ node, width }) => node.position.x + width));
      const top = Math.min(...rects.map(({ node }) => node.position.y));
      const bottom = Math.max(...rects.map(({ node, height }) => node.position.y + height));
      const center = (left + right) / 2;
      const middle = (top + bottom) / 2;
      commitCanvas({
        nodes: nodesRef.current.map((node) => {
          if (!node.selected || node.parentId || node.data.locked) return node;
          const size = nodeSize(node);
          if (alignment === "left") return { ...node, position: { ...node.position, x: left } };
          if (alignment === "center") return { ...node, position: { ...node.position, x: center - size.width / 2 } };
          if (alignment === "right") return { ...node, position: { ...node.position, x: right - size.width } };
          if (alignment === "top") return { ...node, position: { ...node.position, y: top } };
          if (alignment === "middle") return { ...node, position: { ...node.position, y: middle - size.height / 2 } };
          return { ...node, position: { ...node.position, y: bottom - size.height } };
        }),
        edges: edgesRef.current,
      });
    },
    [commitCanvas],
  );

  const distributeSelection = useCallback(
    (axis: "horizontal" | "vertical") => {
      const selected = nodesRef.current
        .filter((node) => node.selected && !node.parentId && !node.data.locked)
        .sort((a, b) => (axis === "horizontal" ? a.position.x - b.position.x : a.position.y - b.position.y));
      if (selected.length < 3) return;
      const first = selected[0];
      const last = selected[selected.length - 1];
      const firstSize = nodeSize(first);
      const lastSize = nodeSize(last);
      const occupied = selected.reduce(
        (sum, node) => sum + (axis === "horizontal" ? nodeSize(node).width : nodeSize(node).height),
        0,
      );
      const span =
        axis === "horizontal"
          ? last.position.x + lastSize.width - first.position.x
          : last.position.y + lastSize.height - first.position.y;
      const gap = (span - occupied) / (selected.length - 1);
      let cursor = axis === "horizontal" ? first.position.x + firstSize.width + gap : first.position.y + firstSize.height + gap;
      const positions = new Map<string, number>();
      selected.slice(1, -1).forEach((node) => {
        positions.set(node.id, cursor);
        cursor += (axis === "horizontal" ? nodeSize(node).width : nodeSize(node).height) + gap;
      });
      commitCanvas({
        nodes: nodesRef.current.map((node) => {
          const position = positions.get(node.id);
          if (position === undefined) return node;
          return {
            ...node,
            position: axis === "horizontal" ? { ...node.position, x: position } : { ...node.position, y: position },
          };
        }),
        edges: edgesRef.current,
      });
    },
    [commitCanvas],
  );

  const groupSelection = useCallback(() => {
    const selected = nodesRef.current.filter((node) => node.selected && !node.parentId && node.type !== "group");
    if (selected.length < 2) return;
    const minX = Math.min(...selected.map((node) => node.position.x));
    const minY = Math.min(...selected.map((node) => node.position.y));
    const maxX = Math.max(...selected.map((node) => node.position.x + nodeSize(node).width));
    const maxY = Math.max(...selected.map((node) => node.position.y + nodeSize(node).height));
    const groupId = `group-${crypto.randomUUID()}`;
    const group: CanvasNode = {
      id: groupId,
      type: "group",
      position: { x: minX - 32, y: minY - 52 },
      data: { title: "分组", body: "" },
      style: { width: maxX - minX + 64, height: maxY - minY + 84 },
      selected: true,
      zIndex: -1,
    };
    const children = nodesRef.current.map((node) =>
      selected.some((selectedNode) => selectedNode.id === node.id)
        ? {
            ...node,
            parentId: groupId,
            extent: "parent" as const,
            position: { x: node.position.x - group.position.x, y: node.position.y - group.position.y },
            selected: false,
          }
        : node,
    );
    commitCanvas({ nodes: [group, ...children], edges: edgesRef.current });
  }, [commitCanvas]);

  const ungroupSelection = useCallback(() => {
    const groupIds = new Set(
      nodesRef.current.filter((node) => node.selected && node.type === "group").map((node) => node.id),
    );
    if (!groupIds.size) return;
    const groups = new Map(nodesRef.current.filter((node) => groupIds.has(node.id)).map((node) => [node.id, node]));
    const nextNodes = nodesRef.current
      .filter((node) => !groupIds.has(node.id))
      .map((node) => {
        if (!node.parentId || !groupIds.has(node.parentId)) return node;
        const parent = groups.get(node.parentId)!;
        return {
          ...node,
          parentId: undefined,
          extent: undefined,
          position: { x: parent.position.x + node.position.x, y: parent.position.y + node.position.y },
          selected: true,
        };
      });
    commitCanvas({ nodes: nextNodes, edges: edgesRef.current });
  }, [commitCanvas]);

  const autoLayout = useCallback(
    (direction: "LR" | "TB" = "LR") => {
      const layoutNodes = nodesRef.current.filter((node) => !node.parentId && node.type !== "group");
      if (!layoutNodes.length) return;
      const graph = new dagre.graphlib.Graph().setDefaultEdgeLabel(() => ({}));
      graph.setGraph({ rankdir: direction, nodesep: 48, ranksep: 72, marginx: 24, marginy: 24 });
      layoutNodes.forEach((node) => graph.setNode(node.id, nodeSize(node)));
      edgesRef.current.forEach((edge) => {
        if (graph.hasNode(edge.source) && graph.hasNode(edge.target)) graph.setEdge(edge.source, edge.target);
      });
      dagre.layout(graph);
      commitCanvas({
        nodes: nodesRef.current.map((node) => {
          const point = graph.node(node.id) as { x: number; y: number } | undefined;
          if (!point || node.parentId || node.data.locked) return node;
          const size = nodeSize(node);
          return { ...node, position: { x: point.x - size.width / 2, y: point.y - size.height / 2 } };
        }),
        edges: edgesRef.current,
      });
      window.setTimeout(() => void instanceRef.current?.fitView({ duration: 240, padding: 0.18 }), 20);
    },
    [commitCanvas],
  );

  const createNode = useCallback(
    (kind: Exclude<CanvasNodeKind, "drawing" | "group">, position?: XYPosition, dataPatch?: Partial<CanvasNodeData>) => {
      const instance = instanceRef.current;
      const bounds = wrapperRef.current?.getBoundingClientRect();
      let nodePosition = position;
      if (!nodePosition && instance && bounds) {
        const center = instance.screenToFlowPosition({ x: bounds.left + bounds.width / 2, y: bounds.top + bounds.height / 2 });
        const offset = (nodesRef.current.length % 5) * 24;
        nodePosition = { x: center.x - 120 + offset, y: center.y - 80 + offset };
      }
      const rawPosition = nodePosition ?? { x: 48, y: 48 };
      const snappedPosition = {
        x: Math.round(rawPosition.x / SNAP_GRID[0]) * SNAP_GRID[0],
        y: Math.round(rawPosition.y / SNAP_GRID[1]) * SNAP_GRID[1],
      };
      const defaults: Record<typeof kind, { title: string; body: string; width: number; height: number }> = {
        note: { title: "新建便签", body: "", width: 240, height: 160 },
        text: { title: "文本", body: "双击或直接编辑文本", width: 240, height: 96 },
        shape: { title: "", body: "", width: 216, height: 144 },
      };
      const preset = defaults[kind];
      const id = `${kind}-${crypto.randomUUID()}`;
      const nextNode: CanvasNode = {
        id,
        type: kind,
        position: snappedPosition,
        data: { title: preset.title, body: preset.body, shape: kind === "shape" ? "rectangle" : undefined, ...dataPatch },
        style: { width: preset.width, height: preset.height },
        selected: true,
      };
      commitCanvas({
        nodes: [...nodesRef.current.map((node) => ({ ...node, selected: false })), nextNode],
        edges: edgesRef.current.map((edge) => ({ ...edge, selected: false })),
      });
      setTool("select");
      requestAnimationFrame(() => {
        wrapperRef.current?.querySelector<HTMLInputElement>(`[data-id="${id}"] .studio-note-title`)?.select();
      });
    },
    [commitCanvas],
  );

  const centerOnNode = useCallback((node: CanvasNode) => {
    const instance = instanceRef.current;
    if (!instance) return;
    const nextNodes = nodesRef.current.map((candidate) => ({ ...candidate, selected: candidate.id === node.id }));
    nodesRef.current = nextNodes;
    setNodes(nextNodes);
    void instance.fitView({ nodes: [node], duration: 220, padding: 1.4, maxZoom: 1.5 });
    setSearchOpen(false);
  }, []);

  const exportJson = useCallback(async () => {
    const payload = { version: 1, exportedAt: new Date().toISOString(), viewport: instanceRef.current?.getViewport(), ...currentSnapshot() };
    const content = JSON.stringify(payload, null, 2);
    if (isTauri()) {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({ title: "导出工作画布", defaultPath: "deepagent-canvas.json", filters: [{ name: "JSON", extensions: ["json"] }] });
      if (typeof path !== "string" || !path.trim()) return;
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("save_text_file", { path, content });
      return;
    }
    downloadBlob(new Blob([content], { type: "application/json" }), "deepagent-canvas.json");
  }, [currentSnapshot]);

  const importJson = useCallback(
    async (event: ChangeEvent<HTMLInputElement>) => {
      const file = event.target.files?.[0];
      event.target.value = "";
      if (!file) return;
      try {
        const payload = JSON.parse(await file.text()) as Partial<CanvasSnapshot> & { viewport?: Viewport };
        if (!Array.isArray(payload.nodes) || !Array.isArray(payload.edges)) throw new Error("invalid canvas file");
        commitCanvas({ nodes: payload.nodes as CanvasNode[], edges: payload.edges as CanvasEdge[] });
        if (payload.viewport) void instanceRef.current?.setViewport(payload.viewport, { duration: 180 });
      } catch {
        window.alert("无法导入：文件不是有效的 DeepAgent 画布 JSON。");
      }
    },
    [commitCanvas],
  );

  const exportImage = useCallback(async () => {
    const canvas = wrapperRef.current?.querySelector<HTMLElement>(".react-flow");
    if (!canvas) return;
    const previousViewport = instanceRef.current?.getViewport();
    await instanceRef.current?.fitView({ duration: 0, padding: 0.12, minZoom: 0.1, maxZoom: 1.5 });
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    const dataUrl = await toPng(canvas, {
      backgroundColor: isDark ? "#111827" : "#ffffff",
      pixelRatio: 2,
      filter: (element) => !element.classList?.contains("studio-canvas-toolbar") && !element.classList?.contains("react-flow__controls"),
    });
    const response = await fetch(dataUrl);
    const blob = await response.blob();
    if (previousViewport) await instanceRef.current?.setViewport(previousViewport, { duration: 0 });
    if (isTauri()) {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const path = await save({ title: "导出画布图片", defaultPath: "deepagent-canvas.png", filters: [{ name: "PNG", extensions: ["png"] }] });
      if (typeof path !== "string" || !path.trim()) return;
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("save_binary_file", { path, bytes: Array.from(new Uint8Array(await blob.arrayBuffer())) });
      return;
    }
    downloadBlob(blob, "deepagent-canvas.png");
  }, [isDark]);

  const clearCanvas = useCallback(() => {
    if (!nodesRef.current.length && !edgesRef.current.length) return;
    if (!window.confirm("确定清空整个画布吗？清空后仍可使用撤销恢复。")) return;
    commitCanvas({ nodes: [], edges: [] });
  }, [commitCanvas]);

  const isValidConnection = useCallback((connection: Connection | CanvasEdge) => {
    if (!connection.source || !connection.target || connection.source === connection.target) return false;
    const adjacency = new Map<string, string[]>();
    edgesRef.current.forEach((edge) => adjacency.set(edge.source, [...(adjacency.get(edge.source) ?? []), edge.target]));
    const stack = [connection.target];
    const visited = new Set<string>();
    while (stack.length) {
      const current = stack.pop()!;
      if (current === connection.source) return false;
      if (visited.has(current)) continue;
      visited.add(current);
      stack.push(...(adjacency.get(current) ?? []));
    }
    return true;
  }, []);

  const nodeTypes = useMemo(
    () => {
      const toolbar = (id: string, data: CanvasNodeData) => (
        <NodeToolbar position={Position.Top} offset={10} className="studio-node-toolbar">
          <button type="button" title="复制" onClick={() => void duplicateSelection()}>复制</button>
          <button type="button" title={data.locked ? "解锁" : "锁定"} onClick={toggleSelectedLock}>{data.locked ? "解锁" : "锁定"}</button>
          <button type="button" title="置于顶层" onClick={() => moveSelectionLayer("front")}>上移</button>
          <button type="button" title="删除" onClick={() => deleteByIds(new Set([id]))}>删除</button>
        </NodeToolbar>
      );

      const resizer = (selected: boolean, locked?: boolean) => (
        <NodeResizer
          isVisible={selected && !locked}
          minWidth={72}
          minHeight={48}
          onResizeStart={beginInteraction}
          onResizeEnd={finishInteraction}
          lineClassName="studio-note-resize-line"
          handleClassName="studio-note-resize-handle"
        />
      );

      return {
        note: function NoteNode({ id, data, selected }: NodeProps<CanvasNode>) {
          return (
            <div className={`studio-note-node${selected ? " is-selected" : ""}${data.locked ? " is-locked" : ""}`}>
              {toolbar(id, data)}
              {resizer(selected, data.locked)}
              <Handle type="target" position={Position.Left} className="studio-note-handle" />
              <div className="studio-note-header">
                <span className="studio-note-drag-handle" aria-label="拖动便签" title="拖动便签"><FontAwesomeIcon icon={faGripVertical} /></span>
                <input className="nodrag studio-note-title" aria-label="便签标题" value={data.title} placeholder="未命名便签" onFocus={beginInteraction} onBlur={finishInteraction} onChange={(event) => updateNodeData(id, { title: event.target.value })} />
              </div>
              <textarea className="nodrag nowheel studio-note-body" aria-label="便签内容" value={data.body} placeholder="输入内容…" onFocus={beginInteraction} onBlur={finishInteraction} onChange={(event) => updateNodeData(id, { body: event.target.value })} />
              <Handle type="source" position={Position.Right} className="studio-note-handle" />
            </div>
          );
        },
        text: function TextNode({ id, data, selected }: NodeProps<CanvasNode>) {
          return (
            <div className={`studio-text-node${selected ? " is-selected" : ""}${data.locked ? " is-locked" : ""}`}>
              {toolbar(id, data)}
              {resizer(selected, data.locked)}
              <span className="studio-note-drag-handle" aria-label="拖动文本" title="拖动文本"><FontAwesomeIcon icon={faGripVertical} /></span>
              <textarea className="nodrag nowheel studio-text-editor" aria-label="文本内容" value={data.body} onFocus={beginInteraction} onBlur={finishInteraction} onChange={(event) => updateNodeData(id, { body: event.target.value })} />
            </div>
          );
        },
        shape: function ShapeNode({ id, data, selected }: NodeProps<CanvasNode>) {
          return (
            <div className={`studio-shape-node is-${data.shape ?? "rectangle"}${selected ? " is-selected" : ""}${data.locked ? " is-locked" : ""}`}>
              {toolbar(id, data)}
              {resizer(selected, data.locked)}
              <Handle type="target" position={Position.Left} className="studio-note-handle" />
              <span className="studio-shape-drag-label">{data.body}</span>
              <Handle type="source" position={Position.Right} className="studio-note-handle" />
            </div>
          );
        },
        drawing: function DrawingNode({ id, data, selected }: NodeProps<CanvasNode>) {
          const points = data.points ?? [];
          return (
            <div className={`studio-drawing-node${selected ? " is-selected" : ""}${data.locked ? " is-locked" : ""}`}>
              {toolbar(id, data)}
              {resizer(selected, data.locked)}
              <svg viewBox={`0 0 100 100`} preserveAspectRatio="none" aria-label="自由绘图">
                <polyline points={points.map((point) => `${point.x},${point.y}`).join(" ")} vectorEffect="non-scaling-stroke" />
              </svg>
            </div>
          );
        },
        group: function GroupNode({ id, data, selected }: NodeProps<CanvasNode>) {
          return (
            <div className={`studio-group-node${selected ? " is-selected" : ""}${data.locked ? " is-locked" : ""}`}>
              {toolbar(id, data)}
              {resizer(selected, data.locked)}
              <div className="studio-group-title"><FontAwesomeIcon icon={faGripVertical} /> {data.title}</div>
            </div>
          );
        },
      };
    },
    [beginInteraction, deleteByIds, duplicateSelection, finishInteraction, moveSelectionLayer, toggleSelectedLock, updateNodeData],
  );

  useEffect(() => {
    window.localStorage.removeItem(LEGACY_CANVAS_LAYOUT_KEY);
    const observer = new MutationObserver(() => setIsDark(document.documentElement.classList.contains("dark")));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const persistedNodes = nodes.map(({ selected: _selected, dragging: _dragging, ...node }) => node);
      const persistedEdges = edges.map(({ selected: _selected, ...edge }) => edge);
      window.localStorage.setItem(CANVAS_DOCUMENT_KEY, JSON.stringify({ nodes: persistedNodes, edges: persistedEdges }));
    }, 160);
    return () => window.clearTimeout(timer);
  }, [edges, nodes]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditableTarget(event.target)) return;
      const commandKey = event.ctrlKey || event.metaKey;
      const key = event.key.toLowerCase();
      if (commandKey && key === "0") {
        event.preventDefault();
        void instanceRef.current?.setViewport(DEFAULT_VIEWPORT, { duration: 180 });
      } else if (commandKey && key === "z") {
        event.preventDefault();
        event.shiftKey ? redo() : undo();
      } else if (commandKey && key === "y") {
        event.preventDefault();
        redo();
      } else if (commandKey && key === "a") {
        event.preventDefault();
        selectAll();
      } else if (commandKey && key === "c") {
        event.preventDefault();
        void copySelection();
      } else if (commandKey && key === "x") {
        event.preventDefault();
        void cutSelection();
      } else if (commandKey && key === "v") {
        event.preventDefault();
        void pasteClipboard();
      } else if (commandKey && key === "d") {
        event.preventDefault();
        void duplicateSelection();
      } else if (event.key === "Backspace" || event.key === "Delete") {
        event.preventDefault();
        deleteSelection();
      } else if (!commandKey && key === "v") setTool("select");
      else if (!commandKey && key === "h") setTool("pan");
      else if (!commandKey && key === "n") createNode("note");
      else if (!commandKey && key === "t") createNode("text");
      else if (!commandKey && key === "r") setTool("rectangle");
      else if (!commandKey && key === "p") setTool("draw");
      else if (!commandKey && key === "l") setTool("lasso");
      else if (!commandKey && key === "e") setTool("eraser");
      else if (event.key === "Escape") {
        setTool("select");
        setContextMenu(null);
        setMoreOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [copySelection, createNode, cutSelection, deleteSelection, duplicateSelection, pasteClipboard, redo, selectAll, undo]);

  const onNodesChange = useCallback((changes: NodeChange<CanvasNode>[]) => {
    const nextNodes = applyNodeChanges(changes, nodesRef.current);
    nodesRef.current = nextNodes;
    setNodes(nextNodes);
  }, []);

  const onEdgesChange = useCallback((changes: EdgeChange<CanvasEdge>[]) => {
    const nextEdges = applyEdgeChanges(changes, edgesRef.current);
    edgesRef.current = nextEdges;
    setEdges(nextEdges);
  }, []);

  const onConnect = useCallback(
    (connection: Connection) => {
      if (!isValidConnection(connection)) return;
      const nextEdges = addEdge({ ...connection, id: `edge-${crypto.randomUUID()}`, type: "smoothstep" }, edgesRef.current);
      commitCanvas({ nodes: nodesRef.current, edges: nextEdges });
    },
    [commitCanvas, isValidConnection],
  );

  const persistViewport = useCallback((viewport: Viewport) => {
    setZoom(viewport.zoom);
    window.localStorage.setItem(CANVAS_VIEWPORT_KEY, JSON.stringify(viewport));
  }, []);

  const handleDrop = useCallback(
    (event: React.DragEvent<HTMLDivElement>) => {
      event.preventDefault();
      if (!instanceRef.current) return;
      const position = instanceRef.current.screenToFlowPosition({ x: event.clientX, y: event.clientY });
      const files = Array.from(event.dataTransfer.files);
      if (files.length) {
        files.forEach((file, index) => createNode("note", { x: position.x + index * 24, y: position.y + index * 24 }, { title: file.name, body: `${file.type || "文件"}\n${Math.ceil(file.size / 1024)} KB` }));
        return;
      }
      const kind = event.dataTransfer.getData("application/deepagent-canvas-node") as "note" | "text" | "shape";
      if (["note", "text", "shape"].includes(kind)) createNode(kind, position);
    },
    [createNode],
  );

  const pointerPosition = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const bounds = wrapperRef.current!.getBoundingClientRect();
    return { x: event.clientX - bounds.left, y: event.clientY - bounds.top };
  }, []);

  const eraserHitsAt = useCallback((clientX: number, clientY: number) => {
    for (const element of document.elementsFromPoint(clientX, clientY)) {
      const node = element.closest<HTMLElement>(".react-flow__node[data-id]");
      if (node?.dataset.id) return { nodeIds: [node.dataset.id], edgeIds: [] as string[] };
    }
    for (const element of document.elementsFromPoint(clientX, clientY)) {
      const edge = element.closest<SVGGElement>(".react-flow__edge[data-id]");
      if (edge?.dataset.id) return { nodeIds: [] as string[], edgeIds: [edge.dataset.id] };
    }
    return { nodeIds: [] as string[], edgeIds: [] as string[] };
  }, []);

  const finishWhiteboardDraft = useCallback(() => {
    const instance = instanceRef.current;
    const bounds = wrapperRef.current?.getBoundingClientRect();
    if (!drawDraft || !instance || !bounds) return;
    if (drawDraft.kind === "eraser") {
      deleteByIds(new Set(drawDraft.nodeIds), new Set(drawDraft.edgeIds));
    } else if (drawDraft.kind === "lasso") {
      const selectedIds = new Set<string>();
      wrapperRef.current?.querySelectorAll<HTMLElement>(".react-flow__node[data-id]").forEach((element) => {
        const rect = element.getBoundingClientRect();
        const center = { x: rect.left + rect.width / 2 - bounds.left, y: rect.top + rect.height / 2 - bounds.top };
        if (pointInPolygon(center, drawDraft.points)) selectedIds.add(element.dataset.id!);
      });
      const nextNodes = nodesRef.current.map((node) => ({ ...node, selected: selectedIds.has(node.id) }));
      nodesRef.current = nextNodes;
      setNodes(nextNodes);
    } else if (drawDraft.kind === "rectangle") {
      const start = instance.screenToFlowPosition({ x: bounds.left + drawDraft.start.x, y: bounds.top + drawDraft.start.y });
      const end = instance.screenToFlowPosition({ x: bounds.left + drawDraft.current.x, y: bounds.top + drawDraft.current.y });
      const position = { x: Math.min(start.x, end.x), y: Math.min(start.y, end.y) };
      const size = { width: Math.max(48, Math.abs(end.x - start.x)), height: Math.max(48, Math.abs(end.y - start.y)) };
      const node: CanvasNode = { id: `shape-${crypto.randomUUID()}`, type: "shape", position, data: { title: "", body: "", shape: "rectangle" }, style: size, selected: true };
      commitCanvas({ nodes: [...nodesRef.current.map((item) => ({ ...item, selected: false })), node], edges: edgesRef.current });
    } else if (drawDraft.points.length > 1) {
      const flowPoints = drawDraft.points.map((point) => instance.screenToFlowPosition({ x: bounds.left + point.x, y: bounds.top + point.y }));
      const minX = Math.min(...flowPoints.map((point) => point.x));
      const minY = Math.min(...flowPoints.map((point) => point.y));
      const maxX = Math.max(...flowPoints.map((point) => point.x));
      const maxY = Math.max(...flowPoints.map((point) => point.y));
      const width = Math.max(24, maxX - minX);
      const height = Math.max(24, maxY - minY);
      const normalized = flowPoints.map((point) => ({ x: ((point.x - minX) / width) * 100, y: ((point.y - minY) / height) * 100 }));
      const node: CanvasNode = { id: `drawing-${crypto.randomUUID()}`, type: "drawing", position: { x: minX, y: minY }, data: { title: "自由绘图", body: "", points: normalized }, style: { width, height }, selected: true };
      commitCanvas({ nodes: [...nodesRef.current.map((item) => ({ ...item, selected: false })), node], edges: edgesRef.current });
    }
    setDrawDraft(null);
    setTool("select");
  }, [commitCanvas, deleteByIds, drawDraft]);

  const searchResults = useMemo(() => {
    const query = searchQuery.trim().toLowerCase();
    if (!query) return nodes.slice(0, 8);
    return nodes.filter((node) => `${node.data.title} ${node.data.body}`.toLowerCase().includes(query)).slice(0, 12);
  }, [nodes, searchQuery]);

  return (
    <div
      ref={wrapperRef}
      className="relative h-full w-full"
      onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = "copy"; }}
      onDrop={handleDrop}
    >
      <input ref={importInputRef} className="hidden" type="file" accept="application/json,.json" onChange={(event) => void importJson(event)} />
      <ReactFlow<CanvasNode, CanvasEdge>
        className={`studio-infinite-canvas is-tool-${tool}`}
        colorMode={isDark ? "dark" : "light"}
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onReconnect={(oldEdge, connection) => commitCanvas({ nodes: nodesRef.current, edges: reconnectEdge(oldEdge, connection, edgesRef.current) })}
        isValidConnection={isValidConnection}
        onNodeDragStart={beginInteraction}
        onNodeDragStop={finishInteraction}
        onNodeClick={(_, node) => { if (tool === "eraser") deleteByIds(new Set([node.id])); }}
        onEdgeClick={(_, edge) => { if (tool === "eraser") deleteByIds(new Set(), new Set([edge.id])); }}
        onEdgeDoubleClick={(_, edge) => {
          const label = window.prompt("连线标签", String(edge.label ?? ""));
          if (label === null) return;
          commitCanvas({ nodes: nodesRef.current, edges: edgesRef.current.map((item) => item.id === edge.id ? { ...item, label } : item) });
        }}
        onNodeContextMenu={(event, node) => {
          event.preventDefault();
          if (!node.selected) {
            const nextNodes = nodesRef.current.map((item) => ({ ...item, selected: item.id === node.id }));
            nodesRef.current = nextNodes;
            setNodes(nextNodes);
          }
          setContextMenu({ x: event.clientX, y: event.clientY, nodeId: node.id });
        }}
        onEdgeContextMenu={(event, edge) => {
          event.preventDefault();
          if (!edge.selected) {
            const nextEdges = edgesRef.current.map((item) => ({ ...item, selected: item.id === edge.id }));
            edgesRef.current = nextEdges;
            setEdges(nextEdges);
          }
          setContextMenu({ x: event.clientX, y: event.clientY, edgeId: edge.id });
        }}
        onPaneContextMenu={(event) => { event.preventDefault(); setContextMenu({ x: event.clientX, y: event.clientY }); }}
        onPaneClick={(event) => {
          setContextMenu(null);
          setMoreOpen(false);
          if (event.detail !== 2 || !instanceRef.current || canvasLocked) return;
          createNode("note", instanceRef.current.screenToFlowPosition({ x: event.clientX, y: event.clientY }));
        }}
        onInit={(instance) => { instanceRef.current = instance; }}
        onMove={(_, viewport) => setZoom(viewport.zoom)}
        onMoveEnd={(_, viewport) => persistViewport(viewport)}
        defaultViewport={initialViewport}
        minZoom={0.1}
        maxZoom={4}
        panOnDrag={tool === "pan" ? [0, 1, 2] : [1, 2]}
        selectionOnDrag={tool === "select"}
        selectionMode={SelectionMode.Partial}
        nodesDraggable={!canvasLocked && tool === "select"}
        nodesConnectable={!canvasLocked && tool === "select"}
        elementsSelectable={!canvasLocked && (tool === "select" || tool === "eraser")}
        zoomOnDoubleClick={false}
        snapToGrid
        snapGrid={SNAP_GRID}
        deleteKeyCode={null}
        multiSelectionKeyCode={["Control", "Meta", "Shift"]}
        selectionKeyCode={null}
        onlyRenderVisibleElements
        proOptions={{ hideAttribution: true }}
      >
        <Background id="studio-canvas-grid" variant={BackgroundVariant.Lines} gap={24} size={1} color={isDark ? "rgba(148, 163, 184, 0.12)" : "rgba(100, 116, 139, 0.16)"} />

        <Panel position="top-left" className="studio-canvas-toolbar" aria-label="画布工具栏">
          <button type="button" className={tool === "select" ? "is-active" : ""} aria-label="选择工具" aria-pressed={tool === "select"} title="选择与框选 (V)" onClick={() => setTool("select")}><FontAwesomeIcon icon={faArrowPointer} /></button>
          <button type="button" className={tool === "pan" ? "is-active" : ""} aria-label="拖动画布" aria-pressed={tool === "pan"} title="拖动画布 (H)" onClick={() => setTool("pan")}><FontAwesomeIcon icon={faHand} /></button>
          <span className="studio-toolbar-divider" />
          <button type="button" aria-label="新建便签" title="新建便签 (N)" draggable onDragStart={(event) => event.dataTransfer.setData("application/deepagent-canvas-node", "note")} onClick={() => createNode("note")}><FontAwesomeIcon icon={faNoteSticky} /></button>
          <button type="button" aria-label="新建文本" title="新建文本 (T)" draggable onDragStart={(event) => event.dataTransfer.setData("application/deepagent-canvas-node", "text")} onClick={() => createNode("text")}><FontAwesomeIcon icon={faFont} /></button>
          <button type="button" className={tool === "rectangle" ? "is-active" : ""} aria-label="矩形工具" title="绘制矩形 (R)" onClick={() => setTool("rectangle")}><FontAwesomeIcon icon={faSquare} /></button>
          <button type="button" aria-label="新建圆形" title="新建圆形" draggable onDragStart={(event) => event.dataTransfer.setData("application/deepagent-canvas-node", "shape")} onClick={() => createNode("shape", undefined, { shape: "ellipse" })}><FontAwesomeIcon icon={faCircle} /></button>
          <button type="button" className={tool === "draw" ? "is-active" : ""} aria-label="画笔工具" title="自由画笔 (P)" onClick={() => setTool("draw")}><FontAwesomeIcon icon={faPencil} /></button>
          <button type="button" className={tool === "lasso" ? "is-active" : ""} aria-label="套索工具" title="套索选择 (L)" onClick={() => setTool("lasso")}><FontAwesomeIcon icon={faDrawPolygon} /></button>
          <button type="button" className={tool === "eraser" ? "is-active" : ""} aria-label="橡皮擦" title="删除节点或连线 (E)" onClick={() => setTool("eraser")}><FontAwesomeIcon icon={faEraser} /></button>
          <span className="studio-toolbar-divider" />
          <button type="button" aria-label="撤销" title="撤销 (Ctrl+Z)" disabled={!historyStatus.canUndo} onClick={undo}><FontAwesomeIcon icon={faRotateLeft} /></button>
          <button type="button" aria-label="重做" title="重做 (Ctrl+Y)" disabled={!historyStatus.canRedo} onClick={redo}><FontAwesomeIcon icon={faRotateRight} /></button>
          <span className="studio-toolbar-divider" />
          <button type="button" aria-label="搜索节点" title="搜索节点" className={searchOpen ? "is-active" : ""} onClick={() => { setSearchOpen((open) => !open); setMoreOpen(false); }}><FontAwesomeIcon icon={faMagnifyingGlass} /></button>
          <button type="button" aria-label="更多操作" title="更多操作" className={moreOpen ? "is-active" : ""} onClick={() => { setMoreOpen((open) => !open); setSearchOpen(false); }}><FontAwesomeIcon icon={faEllipsis} /></button>
        </Panel>

        {searchOpen && (
          <Panel position="top-right" className="studio-search-panel">
            <input autoFocus value={searchQuery} onChange={(event) => setSearchQuery(event.target.value)} placeholder="搜索节点…" aria-label="搜索节点" />
            <div className="studio-search-results">
              {searchResults.map((node) => <button type="button" key={node.id} onClick={() => centerOnNode(node)}><span>{node.data.title || node.type}</span><small>{node.data.body.slice(0, 36)}</small></button>)}
              {!searchResults.length && <p>没有匹配节点</p>}
            </div>
          </Panel>
        )}

        {moreOpen && (
          <Panel position="top-left" className="studio-more-menu">
            <div className="studio-menu-section"><span>编辑</span><button onClick={() => void copySelection()}>复制 <kbd>Ctrl+C</kbd></button><button onClick={() => void pasteClipboard()}>粘贴 <kbd>Ctrl+V</kbd></button><button onClick={() => void duplicateSelection()}>创建副本 <kbd>Ctrl+D</kbd></button><button onClick={selectAll}>全选 <kbd>Ctrl+A</kbd></button></div>
            <div className="studio-menu-section"><span>对象</span><button onClick={toggleSelectedLock}>{selectedNodes.some((node) => node.data.locked) ? "解锁选中" : "锁定选中"}</button><button onClick={() => moveSelectionLayer("front")}>置于顶层</button><button onClick={() => moveSelectionLayer("back")}>置于底层</button><button onClick={groupSelection}>组合</button><button onClick={ungroupSelection}>取消组合</button></div>
            <div className="studio-menu-section"><span>对齐</span><button onClick={() => alignSelection("left")}>左对齐</button><button onClick={() => alignSelection("center")}>水平居中</button><button onClick={() => alignSelection("top")}>顶部对齐</button><button onClick={() => alignSelection("middle")}>垂直居中</button><button onClick={() => distributeSelection("horizontal")}>水平等距</button><button onClick={() => distributeSelection("vertical")}>垂直等距</button></div>
            <div className="studio-menu-section"><span>画布</span><button onClick={() => autoLayout("LR")}>横向自动布局</button><button onClick={() => autoLayout("TB")}>纵向自动布局</button><button onClick={() => setShowMiniMap((show) => !show)}>{showMiniMap ? "隐藏小地图" : "显示小地图"}</button><button onClick={() => setCanvasLocked((locked) => !locked)}>{canvasLocked ? "解锁画布" : "锁定画布"}</button></div>
            <div className="studio-menu-section"><span>文件</span><button onClick={() => importInputRef.current?.click()}>导入 JSON</button><button onClick={exportJson}>导出 JSON</button><button onClick={() => void exportImage()}><FontAwesomeIcon icon={faDownload} /> 导出 PNG</button><button className="is-danger" onClick={clearCanvas}>清空画布</button></div>
          </Panel>
        )}

        {showMiniMap && <MiniMap className="studio-canvas-minimap" pannable zoomable nodeColor={(node) => node.type === "group" ? "#cbd5e1" : node.type === "shape" ? "#93c5fd" : "#e2e8f0"} />}
        <Controls position="bottom-right" showInteractive={false} />
        <Panel position="bottom-left" className="studio-canvas-hint"><span>{canvasLocked ? "画布已锁定" : tool === "pan" ? "左键拖动画布" : tool === "select" ? "左键框选或拖动节点" : tool === "rectangle" ? "拖动绘制矩形" : tool === "draw" ? "拖动自由绘制" : tool === "lasso" ? "圈选节点" : "拖动擦除对象"}</span><span className="studio-canvas-hint-separator" /><span>{selectedNodes.length || selectedEdges.length ? `已选 ${selectedNodes.length + selectedEdges.length} 项` : "双击新建便签"}</span><span className="studio-canvas-hint-separator" /><span>{Math.round(zoom * 100)}%</span></Panel>
      </ReactFlow>

      {(tool === "rectangle" || tool === "draw" || tool === "lasso" || tool === "eraser") && !canvasLocked && (
        <div
          className="studio-whiteboard-overlay"
          onPointerDown={(event) => {
            event.currentTarget.setPointerCapture(event.pointerId);
            const point = pointerPosition(event);
            if (tool === "rectangle") setDrawDraft({ kind: "rectangle", start: point, current: point });
            else if (tool === "eraser") setDrawDraft({ kind: "eraser", points: [point], ...eraserHitsAt(event.clientX, event.clientY) });
            else setDrawDraft({ kind: tool, points: [point] });
          }}
          onPointerMove={(event) => {
            if (!drawDraft) return;
            const point = pointerPosition(event);
            if (drawDraft.kind === "rectangle") setDrawDraft({ ...drawDraft, current: point });
            else if (drawDraft.kind === "eraser") {
              const hits = eraserHitsAt(event.clientX, event.clientY);
              setDrawDraft({ ...drawDraft, points: [...drawDraft.points, point], nodeIds: [...new Set([...drawDraft.nodeIds, ...hits.nodeIds])], edgeIds: [...new Set([...drawDraft.edgeIds, ...hits.edgeIds])] });
            } else setDrawDraft({ ...drawDraft, points: [...drawDraft.points, point] });
          }}
          onPointerUp={finishWhiteboardDraft}
        >
          {drawDraft?.kind === "rectangle" && <div className="studio-shape-draft" style={{ left: Math.min(drawDraft.start.x, drawDraft.current.x), top: Math.min(drawDraft.start.y, drawDraft.current.y), width: Math.abs(drawDraft.current.x - drawDraft.start.x), height: Math.abs(drawDraft.current.y - drawDraft.start.y) }} />}
          {drawDraft?.kind === "draw" && <svg><polyline points={drawDraft.points.map((point) => `${point.x},${point.y}`).join(" ")} /></svg>}
          {drawDraft?.kind === "lasso" && <svg><polygon className="studio-lasso-draft" points={drawDraft.points.map((point) => `${point.x},${point.y}`).join(" ")} /></svg>}
          {drawDraft?.kind === "eraser" && <svg><polyline className="studio-eraser-draft" points={drawDraft.points.map((point) => `${point.x},${point.y}`).join(" ")} /></svg>}
        </div>
      )}

      {contextMenu && (
        <div className="studio-context-menu" style={{ left: contextMenu.x, top: contextMenu.y }} onClick={() => setContextMenu(null)}>
          {contextMenu.nodeId ? <><button onClick={() => void duplicateSelection()}>创建副本</button><button onClick={toggleSelectedLock}>锁定/解锁</button><button onClick={() => moveSelectionLayer("front")}>置于顶层</button><button onClick={() => deleteByIds(new Set([contextMenu.nodeId!]))}>删除节点</button></> : contextMenu.edgeId ? <><button onClick={() => deleteByIds(new Set(), new Set([contextMenu.edgeId!]))}>删除连线</button></> : <><button onClick={() => createNode("note", instanceRef.current?.screenToFlowPosition({ x: contextMenu.x, y: contextMenu.y }))}>新建便签</button><button onClick={() => createNode("text", instanceRef.current?.screenToFlowPosition({ x: contextMenu.x, y: contextMenu.y }))}>新建文本</button><button onClick={() => void pasteClipboard()}>粘贴</button></>}
        </div>
      )}
    </div>
  );
}

export function CanvasApp() {
  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-white text-text-base">
      <CanvasTitleBar />
      <div className="min-h-0 flex-1">
        <ReactFlowProvider><InfiniteCanvas /></ReactFlowProvider>
      </div>
    </div>
  );
}
