import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  faArrowPointer,
  faGripVertical,
  faHand,
  faNoteSticky,
  faRotateLeft,
  faRotateRight,
} from "@fortawesome/free-solid-svg-icons";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addEdge,
  applyEdgeChanges,
  applyNodeChanges,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  NodeResizer,
  Panel,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Connection,
  type Edge,
  type EdgeChange,
  type Node,
  type NodeChange,
  type NodeProps,
  type ReactFlowInstance,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { CanvasTitleBar } from "./CanvasTitleBar";

const LEGACY_CANVAS_LAYOUT_KEY = "deepagent:studio-canvas-layout:v1";
const CANVAS_VIEWPORT_KEY = "deepagent:studio-canvas-viewport:v1";
const CANVAS_DOCUMENT_KEY = "deepagent:studio-canvas-document:v1";
const DEFAULT_VIEWPORT: Viewport = { x: 0, y: 0, zoom: 1 };
const SNAP_GRID: [number, number] = [24, 24];
const HISTORY_LIMIT = 100;

type CanvasTool = "select" | "pan";
type NoteData = { title: string; body: string };
type CanvasNode = Node<NoteData, "note">;
type CanvasEdge = Edge;
type CanvasSnapshot = { nodes: CanvasNode[]; edges: CanvasEdge[] };

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

function InfiniteCanvas() {
  const initialViewport = useMemo(readViewport, []);
  const initialDocument = useMemo(readDocument, []);
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const instanceRef = useRef<ReactFlowInstance<CanvasNode, CanvasEdge> | null>(null);
  const [nodes, setNodes] = useState<CanvasNode[]>(initialDocument.nodes);
  const [edges, setEdges] = useState<CanvasEdge[]>(initialDocument.edges);
  const nodesRef = useRef(nodes);
  const edgesRef = useRef(edges);
  const pastRef = useRef<CanvasSnapshot[]>([]);
  const futureRef = useRef<CanvasSnapshot[]>([]);
  const interactionStartRef = useRef<CanvasSnapshot | null>(null);
  const [historyStatus, setHistoryStatus] = useState({ canUndo: false, canRedo: false });
  const [tool, setTool] = useState<CanvasTool>("pan");
  const [zoom, setZoom] = useState(initialViewport.zoom);
  const [isDark, setIsDark] = useState(() => document.documentElement.classList.contains("dark"));

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
    setHistoryStatus({
      canUndo: pastRef.current.length > 0,
      canRedo: futureRef.current.length > 0,
    });
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
    if (!interactionStartRef.current) {
      interactionStartRef.current = cloneSnapshot(currentSnapshot());
    }
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

  const updateNodeData = useCallback((nodeId: string, patch: Partial<NoteData>) => {
    const nextNodes = nodesRef.current.map((node) =>
      node.id === nodeId ? { ...node, data: { ...node.data, ...patch } } : node,
    );
    nodesRef.current = nextNodes;
    setNodes(nextNodes);
  }, []);

  const noteNodeTypes = useMemo(
    () => ({
      note: function NoteNode({ id, data, selected }: NodeProps<CanvasNode>) {
        return (
          <div className={`studio-note-node${selected ? " is-selected" : ""}`}>
            <NodeResizer
              isVisible={selected}
              minWidth={192}
              minHeight={120}
              onResizeStart={beginInteraction}
              onResizeEnd={finishInteraction}
              lineClassName="studio-note-resize-line"
              handleClassName="studio-note-resize-handle"
            />
            <Handle type="target" position={Position.Left} className="studio-note-handle" />
            <div className="studio-note-header">
              <span className="studio-note-drag-handle" aria-label="拖动便签" title="拖动便签">
                <FontAwesomeIcon icon={faGripVertical} />
              </span>
              <input
                className="nodrag studio-note-title"
                aria-label="便签标题"
                value={data.title}
                placeholder="未命名便签"
                onFocus={beginInteraction}
                onBlur={finishInteraction}
                onChange={(event) => updateNodeData(id, { title: event.target.value })}
              />
            </div>
            <textarea
              className="nodrag nowheel studio-note-body"
              aria-label="便签内容"
              value={data.body}
              placeholder="输入内容…"
              onFocus={beginInteraction}
              onBlur={finishInteraction}
              onChange={(event) => updateNodeData(id, { body: event.target.value })}
            />
            <Handle type="source" position={Position.Right} className="studio-note-handle" />
          </div>
        );
      },
    }),
    [beginInteraction, finishInteraction, updateNodeData],
  );

  const createNote = useCallback(
    (position?: { x: number; y: number }) => {
      const instance = instanceRef.current;
      const bounds = wrapperRef.current?.getBoundingClientRect();
      let notePosition = position;
      if (!notePosition && instance && bounds) {
        const center = instance.screenToFlowPosition({
          x: bounds.left + bounds.width / 2,
          y: bounds.top + bounds.height / 2,
        });
        const offset = (nodesRef.current.length % 5) * 24;
        notePosition = { x: center.x - 120 + offset, y: center.y - 80 + offset };
      }
      const rawPosition = notePosition ?? { x: 48, y: 48 };
      const snappedPosition = {
        x: Math.round(rawPosition.x / SNAP_GRID[0]) * SNAP_GRID[0],
        y: Math.round(rawPosition.y / SNAP_GRID[1]) * SNAP_GRID[1],
      };
      const id = `note-${crypto.randomUUID()}`;
      const nextNode: CanvasNode = {
        id,
        type: "note",
        position: snappedPosition,
        data: { title: "新建便签", body: "" },
        style: { width: 240, height: 160 },
        selected: true,
      };
      const nextNodes = nodesRef.current.map((node) => ({ ...node, selected: false }));
      commitCanvas({ nodes: [...nextNodes, nextNode], edges: edgesRef.current });
      setTool("select");
      requestAnimationFrame(() => {
        wrapperRef.current?.querySelector<HTMLInputElement>(`[data-id="${id}"] .studio-note-title`)?.select();
      });
    },
    [commitCanvas],
  );

  const deleteSelection = useCallback(() => {
    const selectedNodeIds = new Set(nodesRef.current.filter((node) => node.selected).map((node) => node.id));
    const nextNodes = nodesRef.current.filter((node) => !selectedNodeIds.has(node.id));
    const nextEdges = edgesRef.current.filter(
      (edge) => !edge.selected && !selectedNodeIds.has(edge.source) && !selectedNodeIds.has(edge.target),
    );
    if (nextNodes.length === nodesRef.current.length && nextEdges.length === edgesRef.current.length) return;
    commitCanvas({ nodes: nextNodes, edges: nextEdges });
  }, [commitCanvas]);

  useEffect(() => {
    window.localStorage.removeItem(LEGACY_CANVAS_LAYOUT_KEY);
    const observer = new MutationObserver(() => {
      setIsDark(document.documentElement.classList.contains("dark"));
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      const persistedNodes = nodes.map(({ selected: _selected, dragging: _dragging, ...node }) => node);
      const persistedEdges = edges.map(({ selected: _selected, ...edge }) => edge);
      window.localStorage.setItem(
        CANVAS_DOCUMENT_KEY,
        JSON.stringify({ nodes: persistedNodes, edges: persistedEdges }),
      );
    }, 160);
    return () => window.clearTimeout(timer);
  }, [edges, nodes]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (isEditableTarget(event.target)) return;
      const commandKey = event.ctrlKey || event.metaKey;
      if (commandKey && event.key === "0") {
        event.preventDefault();
        void instanceRef.current?.setViewport(DEFAULT_VIEWPORT, { duration: 180 });
        return;
      }
      if (commandKey && event.key.toLowerCase() === "z") {
        event.preventDefault();
        if (event.shiftKey) redo();
        else undo();
        return;
      }
      if (commandKey && event.key.toLowerCase() === "y") {
        event.preventDefault();
        redo();
        return;
      }
      if (event.key === "Backspace" || event.key === "Delete") {
        event.preventDefault();
        deleteSelection();
        return;
      }
      if (event.key.toLowerCase() === "v") setTool("select");
      if (event.key.toLowerCase() === "h") setTool("pan");
      if (event.key.toLowerCase() === "n") createNote();
      if (event.key === "Escape") setTool("select");
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [createNote, deleteSelection, redo, undo]);

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
      const nextEdges = addEdge({ ...connection, type: "smoothstep" }, edgesRef.current);
      commitCanvas({ nodes: nodesRef.current, edges: nextEdges });
    },
    [commitCanvas],
  );

  const persistViewport = useCallback((viewport: Viewport) => {
    setZoom(viewport.zoom);
    window.localStorage.setItem(CANVAS_VIEWPORT_KEY, JSON.stringify(viewport));
  }, []);

  return (
    <div ref={wrapperRef} className="h-full w-full">
      <ReactFlow<CanvasNode, CanvasEdge>
        className={`studio-infinite-canvas is-tool-${tool}`}
        colorMode={isDark ? "dark" : "light"}
        nodes={nodes}
        edges={edges}
        nodeTypes={noteNodeTypes}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onNodeDragStart={beginInteraction}
        onNodeDragStop={finishInteraction}
        onPaneClick={(event) => {
          if (event.detail !== 2) return;
          if (!instanceRef.current) return;
          createNote(instanceRef.current.screenToFlowPosition({ x: event.clientX, y: event.clientY }));
        }}
        onInit={(instance) => {
          instanceRef.current = instance;
        }}
        onMove={(_, viewport) => setZoom(viewport.zoom)}
        onMoveEnd={(_, viewport) => persistViewport(viewport)}
        defaultViewport={initialViewport}
        minZoom={0.1}
        maxZoom={4}
        panOnDrag={tool === "pan" ? true : [1, 2]}
        selectionOnDrag={tool === "select"}
        nodesDraggable={tool === "select"}
        nodesConnectable={tool === "select"}
        elementsSelectable={tool === "select"}
        zoomOnDoubleClick={false}
        snapToGrid
        snapGrid={SNAP_GRID}
        deleteKeyCode={null}
        multiSelectionKeyCode={["Control", "Meta"]}
        selectionKeyCode="Shift"
        onlyRenderVisibleElements
        proOptions={{ hideAttribution: true }}
      >
        <Background
          id="studio-canvas-grid"
          variant={BackgroundVariant.Lines}
          gap={24}
          size={1}
          color={isDark ? "rgba(148, 163, 184, 0.12)" : "rgba(100, 116, 139, 0.16)"}
        />
        <Panel position="top-left" className="studio-canvas-toolbar" aria-label="画布工具栏">
          <button
            type="button"
            className={tool === "select" ? "is-active" : ""}
            aria-label="选择工具"
            aria-pressed={tool === "select"}
            title="选择与框选 (V)"
            onClick={() => setTool("select")}
          >
            <FontAwesomeIcon icon={faArrowPointer} />
          </button>
          <button
            type="button"
            className={tool === "pan" ? "is-active" : ""}
            aria-label="拖动画布"
            aria-pressed={tool === "pan"}
            title="拖动画布 (H)"
            onClick={() => setTool("pan")}
          >
            <FontAwesomeIcon icon={faHand} />
          </button>
          <span className="studio-toolbar-divider" />
          <button type="button" aria-label="新建便签" title="新建便签 (N)" onClick={() => createNote()}>
            <FontAwesomeIcon icon={faNoteSticky} />
          </button>
          <span className="studio-toolbar-divider" />
          <button type="button" aria-label="撤销" title="撤销 (Ctrl+Z)" disabled={!historyStatus.canUndo} onClick={undo}>
            <FontAwesomeIcon icon={faRotateLeft} />
          </button>
          <button type="button" aria-label="重做" title="重做 (Ctrl+Y)" disabled={!historyStatus.canRedo} onClick={redo}>
            <FontAwesomeIcon icon={faRotateRight} />
          </button>
        </Panel>
        <Controls position="bottom-right" showInteractive={false} />
        <Panel position="bottom-left" className="studio-canvas-hint">
          <span>{tool === "pan" ? "拖动画布" : "框选或拖动节点"}</span>
          <span className="studio-canvas-hint-separator" />
          <span>双击新建便签</span>
          <span className="studio-canvas-hint-separator" />
          <span>{Math.round(zoom * 100)}%</span>
        </Panel>
      </ReactFlow>
    </div>
  );
}

export function CanvasApp() {
  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-white text-text-base">
      <CanvasTitleBar />
      <div className="min-h-0 flex-1">
        <ReactFlowProvider>
          <InfiniteCanvas />
        </ReactFlowProvider>
      </div>
    </div>
  );
}
