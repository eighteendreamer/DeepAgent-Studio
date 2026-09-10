import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  faRotateLeft,
  faRotateRight,
  faTableColumns,
  faMagnet,
  faMagnifyingGlassMinus,
  faMagnifyingGlassPlus,
  faMaximize,
  faPlay,
  faStop,
  faChevronUp,
} from "@fortawesome/free-solid-svg-icons";
import {
  HoverCard,
  HoverCardTrigger,
  HoverCardContent,
} from "../../../components/shadcn/hover-card";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { runWorkflow, stopWorkflow, resetAllStatus, isWorkflowRunning } from "../utils/workflowExecutor";
import type { Viewport } from "@xyflow/react";
import type { ReactFlowInstance } from "@xyflow/react";
import { useState, useEffect } from "react";

interface Props {
  viewport: Viewport;
  onViewportChange: (vp: Viewport) => void;
  rfInstance: ReactFlowInstance | null;
}

const BAR_STYLE: React.CSSProperties = {
  background: "rgba(76,80,82,0.55)",
  border: "1px solid rgba(255,255,255,0.08)",
  boxShadow: "rgba(0,0,0,0.11) 0px 20px 84px, rgba(0,0,0,0.15) 0px 33px 139px",
  backdropFilter: "blur(40px)",
  WebkitBackdropFilter: "blur(40px)",
};

const BTN_CLASS =
  "w-7 h-7 flex-shrink-0 rounded-lg flex items-center justify-center transition-all duration-300 active:scale-95";

const ICON_COLOR = "rgba(248,248,248,0.7)";
const ICON_ACTIVE = "rgb(248,248,248)";
const ICON_MUTED = "rgba(248,248,248,0.25)";
const DIVIDER_STYLE: React.CSSProperties = {
  width: 1,
  height: 16,
  background: "rgba(255,255,255,0.14)",
  flexShrink: 0,
};

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

  const proUndo = useProfessionalStore((s) => s.undo);
  const proRedo = useProfessionalStore((s) => s.redo);
  const proPast = useProfessionalStore((s) => s.past);
  const proFuture = useProfessionalStore((s) => s.future);
  const proNodeCount = useProfessionalStore((s) => s.nodes.length);

  const canUndo = mode === "creative" ? creativePast.length > 0 : proPast.length > 0;
  const canRedo = mode === "creative" ? creativeFuture.length > 0 : proFuture.length > 0;
  const nodeCount = mode === "creative" ? creativeNodeCount : proNodeCount;

  const [running, setRunning] = useState(false);
  const [toolsOpen, setToolsOpen] = useState(false);
  useEffect(() => {
    const interval = setInterval(() => setRunning(isWorkflowRunning()), 200);
    return () => clearInterval(interval);
  }, []);

  const handleUndo = () => (mode === "creative" ? creativeUndo() : proUndo());
  const handleRedo = () => (mode === "creative" ? creativeRedo() : proRedo());

  const handleRun = () => {
    if (running) {
      stopWorkflow();
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
    rfInstance?.fitView({ padding: 0.2, duration: 300 });
  };

  const handleSlider = (e: React.ChangeEvent<HTMLInputElement>) => {
    const next = Number(e.target.value) / 100;
    onViewportChange({ ...viewport, zoom: next });
    rfInstance?.setViewport({ ...viewport, zoom: next });
  };

  return (
    <div
      className={`absolute bottom-4 left-1/2 -translate-x-1/2 z-[9997]${toolsOpen ? " pointer-events-none" : ""}`}
    >
      <HoverCard open={toolsOpen} onOpenChange={setToolsOpen} openDelay={120} closeDelay={300}>
        <HoverCardTrigger asChild>
          <button
            className="flex items-center justify-center outline-none transition-all duration-300 hover:brightness-125 active:scale-95 data-[state=open]:pointer-events-none data-[state=open]:opacity-0"
            style={{ ...BAR_STYLE, width: 44, height: 28, borderRadius: 999, cursor: "pointer" }}
            title="工具栏"
          >
            <FontAwesomeIcon icon={faChevronUp} style={{ fontSize: 12, color: ICON_COLOR }} />
          </button>
        </HoverCardTrigger>
        <HoverCardContent
          side="top"
          align="center"
          sideOffset={-28}
          className="w-auto"
          style={{ ...BAR_STYLE, borderRadius: 12, padding: 0 }}
        >
          <div className="flex items-center gap-1 px-1.5" style={{ height: 40 }}>
      {/* Undo / Redo */}
      <button className={BTN_CLASS} onClick={handleUndo} disabled={!canUndo} title="撤销">
        <FontAwesomeIcon icon={faRotateLeft} style={{ fontSize: 14, color: canUndo ? ICON_COLOR : ICON_MUTED }} />
      </button>
      <button className={BTN_CLASS} onClick={handleRedo} disabled={!canRedo} title="重做">
        <FontAwesomeIcon icon={faRotateRight} style={{ fontSize: 14, color: canRedo ? ICON_COLOR : ICON_MUTED }} />
      </button>

      <div style={DIVIDER_STYLE} />

      {/* Grid / Snap */}
      <button
        className={BTN_CLASS}
        onClick={toggleGrid}
        title="网格"
        style={{ background: gridVisible ? "rgba(255,255,255,0.15)" : undefined }}
      >
        <FontAwesomeIcon
          icon={faTableColumns}
          style={{ fontSize: 14, color: gridVisible ? ICON_ACTIVE : ICON_COLOR }}
        />
      </button>
      <button
        className={BTN_CLASS}
        onClick={toggleSnap}
        title="磁吸"
        style={{ background: snapToGrid ? "rgba(255,255,255,0.15)" : undefined }}
      >
        <FontAwesomeIcon
          icon={faMagnet}
          style={{ fontSize: 14, color: snapToGrid ? ICON_ACTIVE : ICON_COLOR }}
        />
      </button>

      <div style={DIVIDER_STYLE} />

      {/* Zoom */}
      <button className={BTN_CLASS} onClick={handleZoomOut} title="缩小">
        <FontAwesomeIcon icon={faMagnifyingGlassMinus} style={{ fontSize: 14, color: ICON_COLOR }} />
      </button>
      <input
        type="range"
        min={20}
        max={300}
        step={5}
        value={percent}
        onChange={handleSlider}
        className="w-28 cursor-pointer"
        style={{ accentColor: "rgb(248,248,248)" }}
      />
      <button className={BTN_CLASS} onClick={handleZoomIn} title="放大">
        <FontAwesomeIcon icon={faMagnifyingGlassPlus} style={{ fontSize: 14, color: ICON_COLOR }} />
      </button>
      <button
        className="px-1.5 text-xs font-medium transition-colors duration-200"
        style={{ color: ICON_COLOR, minWidth: 36 }}
        onClick={handleResetZoom}
        title="重置缩放"
      >
        {percent}%
      </button>

      <div style={DIVIDER_STYLE} />

      {/* Fit screen */}
      <button className={BTN_CLASS} onClick={handleFitScreen} title="适应屏幕">
        <FontAwesomeIcon icon={faMaximize} style={{ fontSize: 14, color: ICON_COLOR }} />
      </button>

      {/* Node count */}
      <span className="px-1 text-xs whitespace-nowrap" style={{ color: "rgba(248,248,248,0.45)" }}>
        {nodeCount} 节点
      </span>

      <div style={DIVIDER_STYLE} />

      {/* Run / Stop */}
      <button
        className="flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-xs font-medium transition-all duration-300 active:scale-95"
        style={{
          background: running ? "rgba(239,68,68,0.8)" : "rgba(59,130,246,0.8)",
          color: "rgb(248,248,248)",
        }}
        onClick={handleRun}
      >
        <FontAwesomeIcon icon={running ? faStop : faPlay} style={{ fontSize: 10 }} />
        {running ? "停止" : "运行"}
      </button>
          </div>
        </HoverCardContent>
      </HoverCard>
    </div>
  );
}
