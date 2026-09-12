import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ReactFlow,
  ReactFlowProvider,
  Background,
  BackgroundVariant,
  SelectionMode,
  useStoreApi,
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
import { CropOverlay } from "./workflow/components/CropOverlay";
import { DrawingOverlay } from "./workflow/components/DrawingOverlay";
import { OutpaintOverlay } from "./workflow/components/OutpaintOverlay";
import { CreativeLibraryPanel } from "./workflow/components/CreativeLibraryPanel";
import { WorkflowNodeShell } from "./workflow/components/WorkflowNodeShell";
import { WorkflowEdge } from "./workflow/components/WorkflowEdge";
import { useWorkflowPersistence } from "./workflow/hooks/useWorkflowPersistence";
import { CREATIVE_NODE_CATEGORIES, PROFESSIONAL_NODE_CATEGORIES } from "./workflow/types";
import { isTauri } from "../api";

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
  const cropTarget = useCanvasStore((s) => s.cropTarget);
  const setCropTarget = useCanvasStore((s) => s.setCropTarget);
  const drawingTarget = useCanvasStore((s) => s.drawingTarget);
  const setDrawingTarget = useCanvasStore((s) => s.setDrawingTarget);
  const outpaintTarget = useCanvasStore((s) => s.outpaintTarget);
  const setOutpaintTarget = useCanvasStore((s) => s.setOutpaintTarget);
  const creativeLibraryOpen = useCanvasStore((s) => s.creativeLibraryOpen);
  const setCreativeLibraryOpen = useCanvasStore((s) => s.setCreativeLibraryOpen);

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
  const flowStore = useStoreApi();
  const isSpaceHeldRef = useRef(false);
  const [isSpaceHeld, setIsSpaceHeld] = useState(false);
  const clipboardRef = useRef<{ mode: string; nodes: any[] } | null>(null);
  const [containerSize, setContainerSize] = useState({ width: 1200, height: 800 });
  const [isExternalDragOver, setIsExternalDragOver] = useState(false);
  const { menu, openMenu, close: closeMenu } = useContextMenu();
  const isDesktop = isTauri();

  // WebView2 can end a captured pointer with pointercancel/lostpointercapture.
  // React Flow releases the DOM capture there, but older versions leave the
  // selection state active, which blocks every subsequent drag selection.
  // Keep this compatibility cleanup desktop-only; browser behavior stays on
  // React Flow's native path.
  useEffect(() => {
    if (!isDesktop) return;

    const finishCapturedPointer = (event: PointerEvent) => {
      window.setTimeout(() => {
        const pane = rfWrapperRef.current?.querySelector<HTMLElement>(".react-flow__pane");
        if (pane?.hasPointerCapture(event.pointerId)) {
          pane.releasePointerCapture(event.pointerId);
        }

        const state = flowStore.getState();
        if (state.userSelectionActive || state.userSelectionRect) {
          flowStore.setState({
            userSelectionActive: false,
            userSelectionRect: null,
            nodesSelectionActive: false,
          });
        }
      }, 0);
    };

    window.addEventListener("pointerup", finishCapturedPointer, true);
    window.addEventListener("pointercancel", finishCapturedPointer, true);
    window.addEventListener("lostpointercapture", finishCapturedPointer, true);
    return () => {
      window.removeEventListener("pointerup", finishCapturedPointer, true);
      window.removeEventListener("pointercancel", finishCapturedPointer, true);
      window.removeEventListener("lostpointercapture", finishCapturedPointer, true);
    };
  }, [flowStore, isDesktop]);

  const handleMediaFiles = useCallback(
    async (files: Array<{ name: string; url: string; kind: "image" | "video" }>, dropPoint?: { x: number; y: number }) => {
      if (files.length === 0 || !rfInstance) return;

      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;

      const baseX = dropPoint
        ? rfInstance.screenToFlowPosition({ x: dropPoint.x - rect.left, y: dropPoint.y - rect.top }).x
        : containerSize.width / 2;
      const baseY = dropPoint
        ? rfInstance.screenToFlowPosition({ x: dropPoint.x - rect.left, y: dropPoint.y - rect.top }).y
        : containerSize.height / 2;

      const cols = 3;
      const spacing = 280;
      const startOffsetX = -((Math.min(files.length, cols) - 1) * spacing) / 2;

      for (let i = 0; i < files.length; i++) {
        const row = Math.floor(i / cols);
        const col = i % cols;
        const f = files[i];
        const nodeKind = f.kind === "video" ? "video-gen" : "image-gen";
        const data = f.kind === "video" ? { label: f.name.replace(/\.[^.]+$/, ""), videoUrl: f.url } : { label: f.name.replace(/\.[^.]+$/, ""), imageUrl: f.url };
        useCreativeStore.getState().addNodeAt(nodeKind as any, baseX + startOffsetX + col * spacing, baseY + row * spacing, data);
      }
    },
    [rfInstance, containerSize],
  );

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = "copy";
  }, []);

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      const kind = e.dataTransfer.getData("application/workflow-node-kind");
      if (!kind || !rfInstance) return;
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

  // Tauri native drag-drop: OS files dragged onto the webview
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      const { getCurrentWebview } = await import("@tauri-apps/api/webview");
      const { convertFileSrc } = await import("@tauri-apps/api/core");
      if (disposed) return;

      unlisten = await getCurrentWebview().onDragDropEvent(async (event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setIsExternalDragOver(true);
          return;
        }
        if (event.payload.type === "leave") {
          setIsExternalDragOver(false);
          return;
        }
        if (event.payload.type === "drop") {
          setIsExternalDragOver(false);
          const paths = event.payload.paths;
          if (paths.length === 0) return;

          const imageExts = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg", ".avif"]);
          const videoExts = new Set([".mp4", ".webm", ".mov", ".avi", ".mkv", ".flv", ".wmv", ".m4v"]);
          const entries: Array<{ name: string; url: string; kind: "image" | "video" }> = [];
          for (const p of paths) {
            const ext = p.slice(p.lastIndexOf(".")).toLowerCase();
            const sepIdx = p.replace(/\\/g, "/").lastIndexOf("/");
            const name = p.slice(sepIdx + 1);
            if (imageExts.has(ext)) {
              entries.push({ name, url: convertFileSrc(p), kind: "image" });
            } else if (videoExts.has(ext)) {
              entries.push({ name, url: convertFileSrc(p), kind: "video" });
            }
          }
          if (entries.length > 0) {
            await handleMediaFiles(entries, { x: window.innerWidth / 2, y: window.innerHeight / 2 });
          }
        }
      });
    };

    void setup();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [handleMediaFiles]);

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
  const nodeTypes = useMemo(
    () => (mode === "creative" ? creativeNodeTypes : professionalNodeTypes),
    [mode],
  );

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

  const handleNodeDragStop = useCallback(
    (_: unknown, node: { id: string; position: { x: number; y: number } }) => {
      // 拖动期间 onNodesChange 已被 rAF 批处理；落点用 React Flow 给的最终（已 snap）位置再补一次，确保 store 拿到精确坐标
      onNodesChange([{ id: node.id, type: "position", position: node.position }]);
    },
    [onNodesChange],
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

  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (isEditableTarget(e.target)) return;
      const items = Array.from(e.clipboardData?.items ?? []);
      const imageFiles = items
        .filter((it) => it.kind === "file" && it.type.startsWith("image/"))
        .map((it) => it.getAsFile())
        .filter((f): f is File => f !== null);
      if (imageFiles.length === 0) return;

      const toEntries = (files: File[]): Promise<Array<{ name: string; url: string; kind: "image" }>> =>
        Promise.all(
          files.map(
            (f, i) =>
              new Promise<{ name: string; url: string; kind: "image" }>((resolve, reject) => {
                const reader = new FileReader();
                reader.onload = () => resolve({ name: `clipboard-${Date.now()}-${i}.png`, url: reader.result as string, kind: "image" });
                reader.onerror = reject;
                reader.readAsDataURL(f);
              }),
          ),
        );

      void toEntries(imageFiles).then((entries) => handleMediaFiles(entries));
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [handleMediaFiles]);

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
      {isExternalDragOver && (
        <div
          className="absolute inset-0 z-50 flex items-center justify-center pointer-events-none"
          style={{ background: "rgba(59,130,246,0.06)", border: "2px dashed rgba(59,130,246,0.5)" }}
        >
          <div
            className="px-5 py-3 rounded-2xl"
            style={{
              background: "rgba(30,30,35,0.9)",
              border: "1px solid rgba(59,130,246,0.3)",
              backdropFilter: "blur(10px)",
            }}
          >
            <span className="text-sm font-medium" style={{ color: "rgba(248,248,248,0.85)" }}>
              释放以添加图片到画布
            </span>
          </div>
        </div>
      )}
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
        onNodeDragStop={handleNodeDragStop}
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
        // 左键拖动交给框选，避免桌面 WebView 的原生 mousedown 被 pan 手势抢走。
        // 画布平移使用现有滚轮/Shift+滚轮逻辑。
        panOnDrag={isDesktop ? false : [1, 2]}
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
              {mode === "creative" ? "拖入图片/视频、Ctrl+V 粘贴，或双击添加创作节点" : "添加 LLM、代码、HTTP 等工作流节点"}
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
      {cropTarget && (
        <CropOverlay
          key={cropTarget.nodeId}
          imageUrl={cropTarget.imageUrl}
          itemName={cropTarget.name}
          initialRatio={cropTarget.ratio}
          onConfirm={(dataUrl) => {
            useCreativeStore.getState().updateNodeData(cropTarget.nodeId, { imageUrl: dataUrl });
            setCropTarget(null);
          }}
          onCancel={() => setCropTarget(null)}
        />
      )}
      {drawingTarget && (
        <DrawingOverlay
          imageUrl={drawingTarget.imageUrl}
          itemName={drawingTarget.name}
          mode={drawingTarget.mode}
          onConfirm={(dataUrl) => {
            useCreativeStore.getState().updateNodeData(drawingTarget.nodeId, { imageUrl: dataUrl });
            setDrawingTarget(null);
          }}
          onCancel={() => setDrawingTarget(null)}
        />
      )}
      {outpaintTarget && (
        <OutpaintOverlay
          imageUrl={outpaintTarget.imageUrl}
          itemName={outpaintTarget.name}
          onCancel={() => setOutpaintTarget(null)}
        />
      )}
      {creativeLibraryOpen && (
        <CreativeLibraryPanel
          onClose={() => setCreativeLibraryOpen(false)}
          onUse={(item) => {
            if (!selectedNodeId) return;
            const updates: Record<string, unknown> = {
              _creativeLabel: item.name,
              imagePrompt: item.prompt ?? "",
              status: "idle",
            };
            if (item.imageUrl) updates.imageInputUrls = [item.imageUrl];
            useCreativeStore.getState().updateNodeData(selectedNodeId, updates);
          }}
        />
      )}
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
