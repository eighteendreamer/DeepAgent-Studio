import { HoverInfo } from "../../../components/ui/HoverInfo";
import {
  AlignHorizontalJustifyCenter,
  AlignHorizontalJustifyEnd,
  AlignHorizontalJustifyStart,
  AlignVerticalJustifyCenter,
  AlignVerticalJustifyEnd,
  AlignVerticalJustifyStart,
  Grid3x3,
  Magnet,
  Maximize2,
  Play,
  Redo2,
  Settings,
  Square,
  Undo2,
  ZoomIn,
  ZoomOut,
  LayoutGrid,
  type LucideIcon,
} from "lucide-react";
import { Button } from "../../../components/shadcn/button";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { cancelWorkflow, runWorkflow, resetAllStatus, isWorkflowRunning } from "../utils/workflowExecutor";
import { applyDagreLayout } from "../utils/layout";
import type { NodeAlignMode } from "../types";
import type { Viewport } from "@xyflow/react";
import type { ReactFlowInstance } from "@xyflow/react";
import { useState, useEffect } from "react";
import { CanvasToolbarDock } from "./CanvasToolbarDock";

interface Props {
  viewport: Viewport;
  onViewportChange: (vp: Viewport) => void;
  rfInstance: ReactFlowInstance | null;
}

const BTN_CLASS =
  "w-7 h-7 flex-shrink-0 rounded-lg bg-transparent p-0 text-white/70 transition-[background-color,color,transform] duration-150 hover:bg-white/10 hover:text-white active:scale-95 disabled:opacity-100";

const ICON_COLOR = "rgba(248,248,248,0.7)";
const ICON_ACTIVE = "rgb(248,248,248)";
const ICON_MUTED = "rgba(248,248,248,0.25)";
const DIVIDER_STYLE: React.CSSProperties = {
  width: 1,
  height: 16,
  background: "rgba(255,255,255,0.14)",
  flexShrink: 0,
};

const ALIGN_BUTTONS: Array<{ mode: NodeAlignMode; title: string; Icon: LucideIcon }> = [
  { mode: "left", title: "左对齐", Icon: AlignHorizontalJustifyStart },
  { mode: "center-x", title: "水平居中对齐", Icon: AlignHorizontalJustifyCenter },
  { mode: "right", title: "右对齐", Icon: AlignHorizontalJustifyEnd },
  { mode: "top", title: "顶端对齐", Icon: AlignVerticalJustifyStart },
  { mode: "center-y", title: "垂直居中对齐", Icon: AlignVerticalJustifyCenter },
  { mode: "bottom", title: "底端对齐", Icon: AlignVerticalJustifyEnd },
];

export function BottomBar({ viewport, onViewportChange, rfInstance }: Props) {
  const mode = useCanvasStore((s) => s.mode);
  const gridVisible = useCanvasStore((s) => s.gridVisible);
  const toggleGrid = useCanvasStore((s) => s.toggleGrid);
  const snapToGrid = useCanvasStore((s) => s.snapToGrid);
  const toggleSnap = useCanvasStore((s) => s.toggleSnap);

  const creativeUndo = useCreativeStore((s) => s.undo);
  const creativeRedo = useCreativeStore((s) => s.redo);
  const creativePast = useCreativeStore((s) => s.past);
  const creativeFuture = useCreativeStore((s) => s.future);
  const creativeNodeCount = useCreativeStore((s) => s.nodes.length);
  const creativeSelectedCount = useCreativeStore((s) => s.nodes.filter((n) => n.selected).length);
  const creativeAlignNodes = useCreativeStore((s) => s.alignNodes);

  const proUndo = useProfessionalStore((s) => s.undo);
  const proRedo = useProfessionalStore((s) => s.redo);
  const proPast = useProfessionalStore((s) => s.past);
  const proFuture = useProfessionalStore((s) => s.future);
  const proNodeCount = useProfessionalStore((s) => s.nodes.length);
  const proSelectedCount = useProfessionalStore((s) => s.nodes.filter((n) => n.selected).length);
  const proAlignNodes = useProfessionalStore((s) => s.alignNodes);

  const canUndo = mode === "creative" ? creativePast.length > 0 : proPast.length > 0;
  const canRedo = mode === "creative" ? creativeFuture.length > 0 : proFuture.length > 0;
  const nodeCount = mode === "creative" ? creativeNodeCount : proNodeCount;
  const selectedCount = mode === "creative" ? creativeSelectedCount : proSelectedCount;
  const canAlign = selectedCount >= 2;

  const handleAlign = (alignMode: NodeAlignMode) => {
    if (mode === "creative") creativeAlignNodes(alignMode);
    else proAlignNodes(alignMode);
  };

  const [running, setRunning] = useState(false);
  useEffect(() => {
    const interval = setInterval(() => setRunning(isWorkflowRunning()), 200);
    return () => clearInterval(interval);
  }, []);

  const handleUndo = () => (mode === "creative" ? creativeUndo() : proUndo());
  const handleRedo = () => (mode === "creative" ? creativeRedo() : proRedo());

  const handleRun = () => {
    if (running) {
      void cancelWorkflow();
    } else {
      resetAllStatus();
      void runWorkflow();
    }
  };

  const zoom = viewport.zoom;
  const percent = Math.round(zoom * 100);

  const handleZoomIn = () => {
    const next = Math.min(zoom + 0.1, 3);
    onViewportChange({ ...viewport, zoom: next });
    rfInstance?.setViewport({ ...viewport, zoom: next });
  };
  const handleZoomOut = () => {
    const next = Math.max(zoom - 0.1, 0.2);
    onViewportChange({ ...viewport, zoom: next });
    rfInstance?.setViewport({ ...viewport, zoom: next });
  };
  const handleResetZoom = () => {
    onViewportChange({ ...viewport, zoom: 1 });
    rfInstance?.setViewport({ ...viewport, zoom: 1 });
  };
  const handleFitScreen = () => {
    rfInstance?.fitView({ padding: 0.2, duration: 300, maxZoom: 0.9 });
  };

  const handleSlider = (e: React.ChangeEvent<HTMLInputElement>) => {
    const next = Number(e.target.value) / 100;
    onViewportChange({ ...viewport, zoom: next });
    rfInstance?.setViewport({ ...viewport, zoom: next });
  };

  const handleAutoLayout = () => {
    const store = mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
    const nodes = store.nodes;
    const edges = store.edges;
    if (nodes.length === 0) return;

    store.pushHistory();
    const laidOut = applyDagreLayout(nodes, edges, { direction: "TB", nodeSpacing: 80, rankSpacing: 120 });
    store.setNodes(laidOut);

    // Fit view after layout
    setTimeout(() => {
      rfInstance?.fitView({ padding: 0.2, duration: 300, maxZoom: 0.9 });
    }, 50);
  };

  return (
    <CanvasToolbarDock>
      {/* Grid / Snap */}
      <HoverInfo content="网格"><Button variant="ghost"
        aria-label="网格"
        className={BTN_CLASS}
        onClick={toggleGrid}

        style={{ background: gridVisible ? "rgba(255,255,255,0.15)" : undefined }}
      >
        <Grid3x3 size={14} strokeWidth={1.8} color={gridVisible ? ICON_ACTIVE : ICON_COLOR} />
      </Button></HoverInfo>
      <HoverInfo content="磁吸"><Button variant="ghost"
        aria-label="磁吸"
        className={BTN_CLASS}
        onClick={toggleSnap}

        style={{ background: snapToGrid ? "rgba(255,255,255,0.15)" : undefined }}
      >
        <Magnet size={14} strokeWidth={1.8} color={snapToGrid ? ICON_ACTIVE : ICON_COLOR} />
      </Button></HoverInfo>
      <HoverInfo content="撤销"><Button variant="ghost" aria-label="撤销" className={BTN_CLASS} onClick={handleUndo} disabled={!canUndo} >
        <Undo2 size={14} strokeWidth={1.8} color={canUndo ? ICON_COLOR : ICON_MUTED} />
      </Button></HoverInfo>
      <HoverInfo content="重做"><Button variant="ghost" aria-label="重做" className={BTN_CLASS} onClick={handleRedo} disabled={!canRedo} >
        <Redo2 size={14} strokeWidth={1.8} color={canRedo ? ICON_COLOR : ICON_MUTED} />
      </Button></HoverInfo>

      <div style={DIVIDER_STYLE} />

      {/* Align */}
      {ALIGN_BUTTONS.map(({ mode: alignMode, title, Icon }) => (
        <HoverInfo key={alignMode} content={canAlign ? title : "选择至少两个节点后可对齐"}><Button variant="ghost"
          aria-label={title}
          className={BTN_CLASS}
          onClick={() => handleAlign(alignMode)}
          disabled={!canAlign}

        >
          <Icon size={14} strokeWidth={1.8} color={canAlign ? ICON_COLOR : ICON_MUTED} />
        </Button></HoverInfo>
      ))}

      <div style={DIVIDER_STYLE} />

      {/* Auto layout */}
      <HoverInfo content={nodeCount > 0 ? "自动布局" : "画布为空"}><Button variant="ghost"
        aria-label="自动布局"
        className={BTN_CLASS}
        onClick={handleAutoLayout}
        disabled={nodeCount === 0}

      >
        <LayoutGrid size={14} strokeWidth={1.8} color={nodeCount > 0 ? ICON_COLOR : ICON_MUTED} />
      </Button></HoverInfo>

      <div style={DIVIDER_STYLE} />

      {/* Fit screen + Zoom */}
      <HoverInfo content="适应屏幕"><Button variant="ghost" aria-label="适应屏幕" className={BTN_CLASS} onClick={handleFitScreen} >
        <Maximize2 size={14} strokeWidth={1.8} color={ICON_COLOR} />
      </Button></HoverInfo>
      <HoverInfo content="缩小"><Button variant="ghost" aria-label="缩小" className={BTN_CLASS} onClick={handleZoomOut} >
        <ZoomOut size={14} strokeWidth={1.8} color={ICON_COLOR} />
      </Button></HoverInfo>
      <input
        type="range"
        aria-label="画布缩放比例"
        min={20}
        max={300}
        step={5}
        value={percent}
        onChange={handleSlider}
        className="w-28 cursor-pointer"
        style={{ accentColor: "rgb(248,248,248)" }}
      />
      <HoverInfo content="放大"><Button variant="ghost" aria-label="放大" className={BTN_CLASS} onClick={handleZoomIn} >
        <ZoomIn size={14} strokeWidth={1.8} color={ICON_COLOR} />
      </Button></HoverInfo>
      <HoverInfo content="重置缩放"><Button variant="ghost"
        aria-label="重置缩放"
        className="px-1.5 text-xs font-medium transition-colors duration-200"
        style={{ color: ICON_COLOR, minWidth: 36 }}
        onClick={handleResetZoom}

      >
        {percent}%
      </Button></HoverInfo>

      <div style={DIVIDER_STYLE} />

      {/* Node count */}
      <span className="px-1 text-xs whitespace-nowrap" style={{ color: "rgba(248,248,248,0.45)" }}>
        {selectedCount > 0 ? `${selectedCount}/${nodeCount}` : `${nodeCount} 节点`}
      </span>

      <div style={DIVIDER_STYLE} />

      {/* Run / Stop */}
      <Button variant="ghost"
        className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-[background-color,transform] duration-150 hover:bg-white/10 active:scale-95"
        style={{
          background: running ? "rgba(239,68,68,0.8)" : "rgba(59,130,246,0.8)",
          color: "rgb(248,248,248)",
        }}
        onClick={handleRun}
      >
        {running ? <Square size={10} fill="currentColor" strokeWidth={0} /> : <Play size={10} fill="currentColor" strokeWidth={0} />}
        {running ? "停止" : "运行"}
      </Button>

      <div style={DIVIDER_STYLE} />

      {/* Settings */}
      <HoverInfo content="设置"><Button variant="ghost" aria-label="设置" className={BTN_CLASS}  onClick={useCanvasStore.getState().openSettings}>
        <Settings size={14} strokeWidth={1.8} color={ICON_COLOR} />
      </Button></HoverInfo>
    </CanvasToolbarDock>
  );
}
