import { useCallback, useEffect, useRef, useState } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  SelectionMode,
  type ReactFlowInstance,
  type Viewport,
  type FinalConnectionState,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useCanvasStore } from "./workflow/store/canvasStore";
import { useCreativeStore } from "./workflow/store/creativeStore";
import { useProfessionalStore } from "./workflow/store/professionalStore";
import { ModeSwitcher } from "./workflow/components/ModeSwitcher";
import { BottomBar as WorkflowBottomBar } from "./workflow/components/BottomBar";
import { MiniMap } from "./workflow/components/MiniMap";
import { NodePicker } from "./workflow/components/NodePicker";
import { ContextMenu, useContextMenu } from "./workflow/components/ContextMenu";
import { CanvasSettingsDialog } from "./workflow/components/CanvasSettingsDialog";
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
  const setPendingConnection = useCanvasStore((s) => s.setPendingConnection);
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
  const rfWrapperRef = useRef<HTMLDivElement>(null);
  const isSpaceHeldRef = useRef(false);
  const [isSpaceHeld, setIsSpaceHeld] = useState(false);
  const clipboardRef = useRef<{ mode: string; nodes: any[] } | null>(null);
  const [containerSize, setContainerSize] = useState({ width: 1200, height: 800 });
  const { menu, openMenu, close: closeMenu } = useContextMenu();

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      for (const entry of entries) {
        setContainerSize({ width: entry.contentRect.width, height: entry.contentRect.height });
      }
    });
    ro.observe(el);
    setContainerSize({ width: el.clientWidth, height: el.clientHeight });
    return () => ro.disconnect();
  }, []);

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

  const handleNodeDragStart = useCallback(() => {
    // pushHistory snapshots current state, so it must run before the drag mutates positions
    (mode === "creative" ? useCreativeStore : useProfessionalStore).getState().pushHistory();
  }, [mode]);

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

  const handleConnectEnd = useCallback(
    (event: MouseEvent | TouchEvent, state: FinalConnectionState) => {
      // Valid drops create the edge via onConnect; drops on any handle are deliberate, not a miss
      if (state.isValid || state.toHandle || !state.fromNode || !state.fromHandle) return;
      if (!rfInstance) return;
      const clientX = "changedTouches" in event ? event.changedTouches[0].clientX : event.clientX;
      const clientY = "changedTouches" in event ? event.changedTouches[0].clientY : event.clientY;
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const world = rfInstance.screenToFlowPosition({ x: clientX - rect.left, y: clientY - rect.top });
      setPendingConnection({ nodeId: state.fromNode.id, handleType: state.fromHandle.type });
      openNodePicker({ x: clientX, y: clientY, worldX: world.x, worldY: world.y });
    },
    [rfInstance, openNodePicker, setPendingConnection],
  );

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
      if (e.code === "Space" && !isEditableTarget(e.target)) {
        e.preventDefault();
        if (!isSpaceHeldRef.current) {
          isSpaceHeldRef.current = true;
          setIsSpaceHeld(true);
        }
        return;
      }

      if (e.key === "Escape") {
        onNodesChange(nodes.map((n) => ({ id: n.id, type: "select", selected: false })));
        setSelectedNodeId(null);
        closeNodePicker();
        closeMenu();
        return;
      }

      if (e.key === "Delete" || e.key === "Backspace") {
        if (selectedNodeId && !isEditableTarget(e.target)) {
          if (mode === "creative") useCreativeStore.getState().removeNode(selectedNodeId);
          else useProfessionalStore.getState().removeNode(selectedNodeId);
          setSelectedNodeId(null);
        }
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === "z" && !e.shiftKey) {
        e.preventDefault();
        if (mode === "creative") useCreativeStore.getState().undo();
        else useProfessionalStore.getState().undo();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "z" && e.shiftKey) {
        e.preventDefault();
        if (mode === "creative") useCreativeStore.getState().redo();
        else useProfessionalStore.getState().redo();
        return;
      }
      if ((e.ctrlKey || e.metaKey) && e.key === "y") {
        e.preventDefault();
        if (mode === "creative") useCreativeStore.getState().redo();
        else useProfessionalStore.getState().redo();
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === "a" && !isEditableTarget(e.target)) {
        e.preventDefault();
        const allIds = nodes.map((n) => n.id);
        if (mode === "creative") useCreativeStore.getState().setSelectedIds(allIds);
        else useProfessionalStore.getState().setSelectedIds(allIds);
        return;
      }

      if (e.key === "f" && !isEditableTarget(e.target) && !(e.ctrlKey || e.metaKey)) {
        rfInstance?.fitView({ padding: 0.2, duration: 300, maxZoom: 0.9 });
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === "c" && !isEditableTarget(e.target)) {
        const store = mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
        const selected = store.nodes.filter((n) => n.selected);
        if (selected.length > 0) {
          clipboardRef.current = { mode, nodes: selected.map((n) => ({ ...n, data: { ...n.data } })) };
        }
        return;
      }

      if ((e.ctrlKey || e.metaKey) && e.key === "v" && !isEditableTarget(e.target)) {
        if (!clipboardRef.current || clipboardRef.current.mode !== mode) return;
        const offset = 40;
        for (const cn of clipboardRef.current.nodes) {
          if (mode === "creative") {
            useCreativeStore.getState().addNodeAt(cn.data.kind, cn.position.x + offset, cn.position.y + offset, cn.data);
          } else {
            useProfessionalStore.getState().addNodeAt(cn.data.kind, cn.position.x + offset, cn.position.y + offset, cn.data);
          }
        }
        return;
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      if (e.code === "Space") {
        isSpaceHeldRef.current = false;
        setIsSpaceHeld(false);
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    window.addEventListener("keyup", handleKeyUp);
    return () => {
      window.removeEventListener("keydown", handleKeyDown);
      window.removeEventListener("keyup", handleKeyUp);
    };
  }, [mode, selectedNodeId, setSelectedNodeId, closeNodePicker, closeMenu, nodes, onNodesChange, rfInstance]);

  const handleWheel = useCallback(
    (e: WheelEvent) => {
      if (!rfInstance) return;
      // Floating layers (node toolbar / node picker / context menu) scroll natively; don't pan the canvas under them
      if (e.target instanceof HTMLElement && e.target.closest(".react-flow__node-toolbar, .wf-floating-layer")) return;
      e.preventDefault();

      const vp = rfInstance.getViewport();
      const zoomSensitivity = 0.001;
      const panSensitivity = 1;

      if (e.ctrlKey || e.metaKey) {
        // Ctrl+scroll: zoom
        const delta = -e.deltaY * zoomSensitivity;
        const newZoom = Math.min(Math.max(vp.zoom + delta * vp.zoom, 0.2), 3);
        // Zoom toward cursor position
        const rect = containerRef.current?.getBoundingClientRect();
        if (rect) {
          const mouseX = e.clientX - rect.left;
          const mouseY = e.clientY - rect.top;
          const flowPos = rfInstance.screenToFlowPosition({ x: mouseX, y: mouseY });
          const zoomRatio = newZoom / vp.zoom;
          const newX = flowPos.x - (flowPos.x - vp.x) * zoomRatio;
          const newY = flowPos.y - (flowPos.y - vp.y) * zoomRatio;
          rfInstance.setViewport({ x: newX, y: newY, zoom: newZoom });
        } else {
          rfInstance.setViewport({ ...vp, zoom: newZoom });
        }
      } else if (e.shiftKey) {
        // Shift+scroll: horizontal pan
        const dx = e.deltaY * panSensitivity;
        rfInstance.setViewport({ ...vp, x: vp.x - dx });
      } else {
        // Plain scroll: vertical pan
        const dy = e.deltaY * panSensitivity;
        rfInstance.setViewport({ ...vp, y: vp.y - dy });
      }
    },
    [rfInstance],
  );

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    el.addEventListener("wheel", handleWheel, { passive: false });
    return () => el.removeEventListener("wheel", handleWheel);
  }, [handleWheel]);

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full overflow-hidden"
      style={{ background: "#0a0a0a" }}
      onDoubleClick={handleDoubleClick}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
      onContextMenu={handleContextMenu}
    >
      {gridVisible && (
        <>
          <div
            className="absolute inset-0 pointer-events-none"
            style={{
              backgroundImage: "radial-gradient(circle, rgba(255,255,255,0.16) 1px, transparent 1px)",
              backgroundSize: "24px 24px",
            }}
          />
          <div
            className="absolute inset-0 pointer-events-none"
            style={{
              backgroundImage: "radial-gradient(circle at center, rgba(255,255,255,0.04) 0%, transparent 70%)",
            }}
          />
        </>
      )}

      <div ref={rfWrapperRef} className="absolute inset-0">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onConnectEnd={handleConnectEnd}
        onNodeDragStart={handleNodeDragStart}
        onInit={setRfInstance}
        onNodeClick={handleNodeClick}
        onPaneClick={handlePaneClick}
        onMove={(_, vp) => handleViewportChange(vp)}
        nodeTypes={nodeTypes}
        edgeTypes={edgeTypes}
        snapToGrid={snapToGrid}
        snapGrid={[24, 24]}
        fitView
        fitViewOptions={{ maxZoom: 0.9 }}
        minZoom={0.2}
        maxZoom={3}
        panOnDrag={[1, 2]}
        selectionOnDrag
        selectionMode={SelectionMode.Partial}
        nodesDraggable={!isSpaceHeld}
        zoomOnScroll={false}
        panOnScroll={false}
        proOptions={{ hideAttribution: true }}
        style={{ background: "transparent" }}
        className="studio-workflow-canvas"
      >
        <Background variant={BackgroundVariant.Dots} gap={24} size={1} color="transparent" />
      </ReactFlow>
      </div>

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
      <MiniMap containerWidth={containerSize.width} containerHeight={containerSize.height} />
      <NodePicker />
      <ContextMenu menu={menu} onClose={closeMenu} />
      <CanvasSettingsDialog />
    </div>
  );
}

export function CanvasApp() {
  return (
    <div className="h-screen w-full overflow-hidden text-text-base">
      <ReactFlowProvider>
        <WorkflowCanvasInner />
      </ReactFlowProvider>
    </div>
  );
}
