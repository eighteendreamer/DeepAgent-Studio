import { useCallback, useEffect, useRef, useState } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  type ReactFlowInstance,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { CanvasTitleBar } from "./CanvasTitleBar";
import { useCanvasStore } from "./workflow/store/canvasStore";
import { useCreativeStore } from "./workflow/store/creativeStore";
import { useProfessionalStore } from "./workflow/store/professionalStore";
import { ModeSwitcher } from "./workflow/components/ModeSwitcher";
import { BottomBar as WorkflowBottomBar } from "./workflow/components/BottomBar";
import { MiniMap } from "./workflow/components/MiniMap";
import { NodePicker } from "./workflow/components/NodePicker";
import { ConfigPanel } from "./workflow/components/ConfigPanel";
import { ContextMenu, useContextMenu } from "./workflow/components/ContextMenu";
import { WorkflowNodeShell } from "./workflow/components/WorkflowNodeShell";
import { WorkflowEdge } from "./workflow/components/WorkflowEdge";
import { useWorkflowPersistence } from "./workflow/hooks/useWorkflowPersistence";
import { CREATIVE_NODE_CATEGORIES, PROFESSIONAL_NODE_CATEGORIES } from "./workflow/types";

function buildNodeTypes(prefix: string, kinds: string[]) {
  const map: Record<string, React.ComponentType<any>> = {};
  for (const kind of kinds) {
    map[`${prefix}-${kind}`] = WorkflowNodeShell;
  }
  return map;
}

const CREATIVE_KINDS = CREATIVE_NODE_CATEGORIES.flatMap((c) => c.items.map((i) => i.kind));
const PROFESSIONAL_KINDS = PROFESSIONAL_NODE_CATEGORIES.flatMap((c) => c.items.map((i) => i.kind));
const creativeNodeTypes = buildNodeTypes("creative", CREATIVE_KINDS);
const professionalNodeTypes = buildNodeTypes("professional", PROFESSIONAL_KINDS);
const edgeTypes = { default: WorkflowEdge };

function isEditableTarget(target: EventTarget | null) {
  return target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || (target instanceof HTMLElement && target.isContentEditable);
}

function WorkflowCanvasInner() {
  useWorkflowPersistence();
  const mode = useCanvasStore((s) => s.mode);
  const gridVisible = useCanvasStore((s) => s.gridVisible);
  const snapToGrid = useCanvasStore((s) => s.snapToGrid);
  const selectedNodeId = useCanvasStore((s) => s.selectedNodeId);
  const setSelectedNodeId = useCanvasStore((s) => s.setSelectedNodeId);
  const openNodePicker = useCanvasStore((s) => s.openNodePicker);
  const closeNodePicker = useCanvasStore((s) => s.closeNodePicker);
  const setViewport = useCanvasStore((s) => s.setViewport);

  const creativeNodes = useCreativeStore((s) => s.nodes);
  const creativeEdges = useCreativeStore((s) => s.edges);
  const creativeOnNodesChange = useCreativeStore((s) => s.onNodesChange);
  const creativeOnEdgesChange = useCreativeStore((s) => s.onEdgesChange);
  const creativeOnConnect = useCreativeStore((s) => s.onConnect);

  const proNodes = useProfessionalStore((s) => s.nodes);
  const proEdges = useProfessionalStore((s) => s.edges);
  const proOnNodesChange = useProfessionalStore((s) => s.onNodesChange);
  const proOnEdgesChange = useProfessionalStore((s) => s.onEdgesChange);
  const proOnConnect = useProfessionalStore((s) => s.onConnect);

  const [rfInstance, setRfInstance] = useState<ReactFlowInstance | null>(null);
  const [viewport, setLocalViewport] = useState<Viewport>({ x: 0, y: 0, zoom: 1 });
  const containerRef = useRef<HTMLDivElement>(null);
  const { menu, openMenu, close: closeMenu } = useContextMenu();

  const nodes = mode === "creative" ? creativeNodes : proNodes;
  const edges = mode === "creative" ? creativeEdges : proEdges;
  const onNodesChange = mode === "creative" ? creativeOnNodesChange : proOnNodesChange;
  const onEdgesChange = mode === "creative" ? creativeOnEdgesChange : proOnEdgesChange;
  const onConnect = mode === "creative" ? creativeOnConnect : proOnConnect;
  const nodeTypes = mode === "creative" ? creativeNodeTypes : professionalNodeTypes;

  const handleViewportChange = useCallback(
    (vp: Viewport) => {
      setLocalViewport(vp);
      setViewport(vp);
    },
    [setViewport],
  );

  const handleDoubleClick = useCallback(
    (event: React.MouseEvent) => {
      if (!rfInstance) return;
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const screenX = event.clientX - rect.left;
      const screenY = event.clientY - rect.top;
      const worldPos = rfInstance.screenToFlowPosition({ x: screenX, y: screenY });
      openNodePicker({ x: event.clientX, y: event.clientY, worldX: worldPos.x, worldY: worldPos.y });
    },
    [rfInstance, openNodePicker],
  );

  const handleNodeClick = useCallback(
    (_: React.MouseEvent, node: any) => {
      setSelectedNodeId(node.id);
    },
    [setSelectedNodeId],
  );

  const handlePaneClick = useCallback(() => {
    setSelectedNodeId(null);
    closeNodePicker();
  }, [setSelectedNodeId, closeNodePicker]);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      if (!rfInstance) return;
      const kind = e.dataTransfer.getData("application/workflow-node-kind");
      if (!kind) return;
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const screenX = e.clientX - rect.left;
      const screenY = e.clientY - rect.top;
      const worldPos = rfInstance.screenToFlowPosition({ x: screenX, y: screenY });
      if (mode === "creative") {
        useCreativeStore.getState().addNode(kind as any, worldPos.x, worldPos.y);
      } else {
        useProfessionalStore.getState().addNode(kind as any, worldPos.x, worldPos.y);
      }
    },
    [rfInstance, mode],
  );

  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      if (!rfInstance) return;
      e.preventDefault();
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const screenX = e.clientX - rect.left;
      const screenY = e.clientY - rect.top;
      const worldPos = rfInstance.screenToFlowPosition({ x: screenX, y: screenY });
      openMenu({ x: e.clientX, y: e.clientY, worldX: worldPos.x, worldY: worldPos.y });
    },
    [rfInstance, openMenu],
  );

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Delete" || e.key === "Backspace") {
        if (selectedNodeId && !isEditableTarget(e.target)) {
          if (mode === "creative") useCreativeStore.getState().removeNode(selectedNodeId);
          else useProfessionalStore.getState().removeNode(selectedNodeId);
          setSelectedNodeId(null);
        }
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "z" && !e.shiftKey) {
        e.preventDefault();
        if (mode === "creative") useCreativeStore.getState().undo();
        else useProfessionalStore.getState().undo();
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "z" && e.shiftKey) {
        e.preventDefault();
        if (mode === "creative") useCreativeStore.getState().redo();
        else useProfessionalStore.getState().redo();
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "y") {
        e.preventDefault();
        if (mode === "creative") useCreativeStore.getState().redo();
        else useProfessionalStore.getState().redo();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [mode, selectedNodeId, setSelectedNodeId]);

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full overflow-hidden"
      style={{ background: "var(--theme-bg, #000)" }}
      onDoubleClick={handleDoubleClick}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
      onContextMenu={handleContextMenu}
    >
      {gridVisible && (
        <div
          className="absolute inset-0 pointer-events-none"
          style={{
            backgroundImage: "radial-gradient(circle, rgba(148,163,184,0.12) 1px, transparent 1px)",
            backgroundSize: "24px 24px",
          }}
        />
      )}

      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onInit={setRfInstance}
        onNodeClick={handleNodeClick}
        onPaneClick={handlePaneClick}
        onMove={(_, vp) => handleViewportChange(vp)}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        snapToGrid={snapToGrid}
        snapGrid={[24, 24]}
        fitView
        minZoom={0.2}
        maxZoom={3}
        proOptions={{ hideAttribution: true }}
        style={{ background: "transparent" }}
        className="studio-workflow-canvas"
      >
        <Background variant={BackgroundVariant.Dots} gap={24} size={1} color="transparent" />
      </ReactFlow>

      {nodes.length === 0 && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div
            className="flex flex-col items-center gap-2 px-5 py-3.5 rounded-2xl"
            style={{
              background: "rgba(255,255,255,0.04)",
              color: "rgba(255,255,255,0.72)",
              border: "1px dashed rgba(255,255,255,0.14)",
              backdropFilter: "blur(10px)",
            }}
          >
            <div className="flex items-center gap-2 text-sm font-semibold">
              <span style={{ color: "#3b82f6" }}></span>
              双击屏幕
              <span className="font-normal" style={{ opacity: 0.68 }}>
                添加节点
              </span>
            </div>
            <div className="text-xs" style={{ opacity: 0.62 }}>
              {mode === "creative" ? "添加文本、图片、视频等创作节点" : "添加 LLM、代码、HTTP 等工作流节点"}
            </div>
          </div>
        </div>
      )}

      <ModeSwitcher />
      <WorkflowBottomBar viewport={viewport} onViewportChange={handleViewportChange} rfInstance={rfInstance} />
      <MiniMap />
      <NodePicker />
      <ConfigPanel />
      <ContextMenu menu={menu} onClose={closeMenu} />
    </div>
  );
}

export function CanvasApp() {
  return (
    <div className="flex h-screen w-full flex-col overflow-hidden bg-white text-text-base">
      <CanvasTitleBar />
      <div className="min-h-0 flex-1">
        <ReactFlowProvider>
          <WorkflowCanvasInner />
        </ReactFlowProvider>
      </div>
    </div>
  );
}
