import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addEdge,
  Background,
  BackgroundVariant,
  Controls,
  Panel,
  ReactFlow,
  ReactFlowProvider,
  useEdgesState,
  useNodesState,
  type Connection,
  type Edge,
  type Node,
  type ReactFlowInstance,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { CanvasTitleBar } from "./CanvasTitleBar";

const LEGACY_CANVAS_LAYOUT_KEY = "deepagent:studio-canvas-layout:v1";
const CANVAS_VIEWPORT_KEY = "deepagent:studio-canvas-viewport:v1";
const DEFAULT_VIEWPORT: Viewport = { x: 0, y: 0, zoom: 1 };
const SNAP_GRID: [number, number] = [24, 24];

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

function InfiniteCanvas() {
  const initialViewport = useMemo(readViewport, []);
  const instanceRef = useRef<ReactFlowInstance<Node, Edge> | null>(null);
  const [nodes, , onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [zoom, setZoom] = useState(initialViewport.zoom);
  const [isDark, setIsDark] = useState(() => document.documentElement.classList.contains("dark"));

  useEffect(() => {
    window.localStorage.removeItem(LEGACY_CANVAS_LAYOUT_KEY);
    const observer = new MutationObserver(() => {
      setIsDark(document.documentElement.classList.contains("dark"));
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key === "0") {
        event.preventDefault();
        void instanceRef.current?.setViewport(DEFAULT_VIEWPORT, { duration: 180 });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const onConnect = useCallback(
    (connection: Connection) => setEdges((currentEdges) => addEdge(connection, currentEdges)),
    [setEdges],
  );

  const persistViewport = useCallback((viewport: Viewport) => {
    setZoom(viewport.zoom);
    window.localStorage.setItem(CANVAS_VIEWPORT_KEY, JSON.stringify(viewport));
  }, []);

  return (
    <ReactFlow<Node, Edge>
      className="studio-infinite-canvas"
      colorMode={isDark ? "dark" : "light"}
      nodes={nodes}
      edges={edges}
      onNodesChange={onNodesChange}
      onEdgesChange={onEdgesChange}
      onConnect={onConnect}
      onInit={(instance) => {
        instanceRef.current = instance;
      }}
      onMove={(_, viewport) => setZoom(viewport.zoom)}
      onMoveEnd={(_, viewport) => persistViewport(viewport)}
      defaultViewport={initialViewport}
      minZoom={0.1}
      maxZoom={4}
      panOnDrag
      selectionOnDrag={false}
      zoomOnDoubleClick={false}
      snapToGrid
      snapGrid={SNAP_GRID}
      deleteKeyCode={["Backspace", "Delete"]}
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
      <Controls position="bottom-right" showInteractive={false} />
      <Panel position="bottom-left" className="studio-canvas-hint">
        <span>拖动平移</span>
        <span className="studio-canvas-hint-separator" />
        <span>滚轮缩放</span>
        <span className="studio-canvas-hint-separator" />
        <span>{Math.round(zoom * 100)}%</span>
      </Panel>
    </ReactFlow>
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
