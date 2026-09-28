import { HoverInfo } from "../../../components/ui/HoverInfo";
import { useState } from "react";
import { Eye, EyeOff, Maximize2, ZoomIn, ZoomOut } from "lucide-react";
import { useReactFlow } from "@xyflow/react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowEdge, WorkflowNode } from "../types";

const LENS_SIZE = 320;
const MAP_PADDING = 12;
const MAP_SCALE_MAX = 0.5;
const NODE_W = 240;
const NODE_H = 120;
const TICK_COUNT = 24;
const TICK_RING_GAP = 14;
const CONTROL_BAR_WIDTH = 136;
const CONTROL_BAR_HEIGHT = 34;
const NODE_POINTER_SIZE = 18;
const NODE_POINTER_RADIUS = LENS_SIZE / 2 + TICK_RING_GAP * 0.65;

// 沿左下对角线移动后的折中位置：相对原始 128px 只保留一半的位移。
// 圆心仍沿 x=y 对角线定位，同时避免小地图过度贴出左下边界。
const CIRCLE_CENTER_OFFSET = 88;
const CIRCLE_LEFT = CIRCLE_CENTER_OFFSET - LENS_SIZE / 2;
const CIRCLE_BOTTOM = CIRCLE_CENTER_OFFSET - LENS_SIZE / 2;
const TICK_RING_SIZE = LENS_SIZE + TICK_RING_GAP * 2;
const TICK_RING_LEFT = CIRCLE_LEFT - TICK_RING_GAP;
const TICK_RING_BOTTOM = CIRCLE_BOTTOM - TICK_RING_GAP;
const CIRCLE_CENTER_IN_WRAP_Y = LENS_SIZE - CIRCLE_CENTER_OFFSET;

const SHELL_STYLE: React.CSSProperties = {
  width: LENS_SIZE,
  height: LENS_SIZE,
  background: "rgba(30,30,35,0.45)",
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: "50%",
  boxShadow: "0 18px 44px rgba(0,0,0,0.24)",
  backdropFilter: "blur(16px) saturate(1.18)",
  overflow: "hidden",
};

interface Bounds {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
  width: number;
  height: number;
}

interface Projection {
  mapScale: number;
  originX: number;
  originY: number;
}

function finiteOr(value: number, fallback: number) {
  return Number.isFinite(value) ? value : fallback;
}

function computeWorldBounds(nodes: WorkflowNode[], viewportBounds: Bounds): Bounds {
  let { minX, minY, maxX, maxY } = viewportBounds;
  for (const node of nodes) {
    const nx = finiteOr(node.position.x, 0);
    const ny = finiteOr(node.position.y, 0);
    minX = Math.min(minX, nx);
    minY = Math.min(minY, ny);
    maxX = Math.max(maxX, nx + NODE_W);
    maxY = Math.max(maxY, ny + NODE_H);
  }
  const width = Math.max(1, maxX - minX);
  const height = Math.max(1, maxY - minY);
  return { minX, minY, maxX: minX + width, maxY: minY + height, width, height };
}

function computeProjection(
  worldBounds: Bounds,
  width: number,
  height: number,
): Projection {
  const availW = Math.max(1, width - MAP_PADDING * 2);
  const availH = Math.max(1, height - MAP_PADDING * 2);
  const fitted = Math.min(MAP_SCALE_MAX, availW / worldBounds.width, availH / worldBounds.height);
  const mapScale = Number.isFinite(fitted) && fitted > 0 ? fitted : MAP_SCALE_MAX;
  const originX = (width - worldBounds.width * mapScale) / 2 - worldBounds.minX * mapScale;
  const originY = (height - worldBounds.height * mapScale) / 2 - worldBounds.minY * mapScale;

  return {
    mapScale,
    originX,
    originY,
  };
}

function getNearestNodeToViewport(
  nodes: WorkflowNode[],
  viewportBounds: Bounds,
): WorkflowNode | undefined {
  const viewportCenterX = viewportBounds.minX + viewportBounds.width / 2;
  const viewportCenterY = viewportBounds.minY + viewportBounds.height / 2;
  let nearestNode: WorkflowNode | undefined;
  let nearestDistance = Number.POSITIVE_INFINITY;

  for (const node of nodes) {
    const nx = finiteOr(node.position.x, 0) + NODE_W / 2;
    const ny = finiteOr(node.position.y, 0) + NODE_H / 2;
    const distance = (nx - viewportCenterX) ** 2 + (ny - viewportCenterY) ** 2;
    if (distance < nearestDistance) {
      nearestDistance = distance;
      nearestNode = node;
    }
  }

  return nearestNode;
}

interface Props {
  containerWidth: number;
  containerHeight: number;
}

export function MiniMap({ containerWidth, containerHeight }: Props) {
  const { fitView, getViewport, setViewport: setFlowViewport } = useReactFlow<WorkflowNode, WorkflowEdge>();
  const [minimapVisible, setMinimapVisible] = useState(true);
  const mode = useCanvasStore((s) => s.mode);
  const viewport = useCanvasStore((s) => s.viewport);
  const creativeNodes = useCreativeStore((s) => s.nodes);
  const professionalNodes = useProfessionalStore((s) => s.nodes);
  const selectedNodeId = useCanvasStore((s) => s.selectedNodeId);
  const nodes = mode === "creative" ? creativeNodes : professionalNodes;

  const safeZoom = Math.max(viewport.zoom, 0.001);
  const viewportBounds: Bounds = {
    minX: -viewport.x / safeZoom,
    minY: -viewport.y / safeZoom,
    maxX: -viewport.x / safeZoom + containerWidth / safeZoom,
    maxY: -viewport.y / safeZoom + containerHeight / safeZoom,
    width: containerWidth / safeZoom,
    height: containerHeight / safeZoom,
  };

  const worldBounds = computeWorldBounds(nodes, viewportBounds);
  const projection = computeProjection(worldBounds, LENS_SIZE, LENS_SIZE);
  const pointerTargetNode = nodes.find((node) => node.id === selectedNodeId)
    ?? getNearestNodeToViewport(nodes, viewportBounds);
  const nodePointer = pointerTargetNode ? (() => {
    const nx = finiteOr(pointerTargetNode.position.x, 0);
    const ny = finiteOr(pointerTargetNode.position.y, 0);
    const targetX = projection.originX + (nx + NODE_W / 2) * projection.mapScale;
    const targetY = projection.originY + (ny + NODE_H / 2) * projection.mapScale;
    const dx = targetX - LENS_SIZE / 2;
    const dy = targetY - LENS_SIZE / 2;
    const angle = Math.abs(dx) < 0.001 && Math.abs(dy) < 0.001
      ? -Math.PI / 2
      : Math.atan2(dy, dx);

    return {
      left: CIRCLE_CENTER_OFFSET + Math.cos(angle) * NODE_POINTER_RADIUS - NODE_POINTER_SIZE / 2,
      top: CIRCLE_CENTER_IN_WRAP_Y + Math.sin(angle) * NODE_POINTER_RADIUS - NODE_POINTER_SIZE / 2,
      rotate: angle * 180 / Math.PI + 90,
    };
  })() : null;
  const statusColor: Record<string, string> = {
    idle: "rgba(255,255,255,0.2)",
    running: "rgba(59,130,246,0.7)",
    completed: "rgba(34,197,94,0.7)",
    error: "rgba(239,68,68,0.7)",
  };

  const changeZoom = (delta: number) => {
    const current = getViewport();
    const zoom = Math.min(3, Math.max(0.2, current.zoom + delta));
    void setFlowViewport({ ...current, zoom });
  };

  const handleFitView = () => {
    void fitView({ padding: 0.2, duration: 300, maxZoom: 0.9 });
  };

  return (
    <div
      className="absolute left-0 bottom-0 z-[80] pointer-events-none"
      style={{ width: LENS_SIZE, height: LENS_SIZE, overflow: "visible" }}
    >
      <div
        aria-hidden="true"
        style={{
          position: "absolute",
          left: TICK_RING_LEFT,
          bottom: TICK_RING_BOTTOM,
          width: TICK_RING_SIZE,
          height: TICK_RING_SIZE,
          opacity: minimapVisible ? 1 : 0,
          transition: "opacity 180ms ease",
          pointerEvents: "none",
        }}
      >
        {Array.from({ length: TICK_COUNT }, (_, index) => {
          const major = index % 6 === 0;
          return (
            <span
              key={index}
              style={{
                position: "absolute",
                left: "50%",
                top: 0,
                width: major ? 2 : 1,
                height: major ? 13 : 7,
                borderRadius: 999,
                background: major ? "rgba(255,255,255,0.62)" : "rgba(255,255,255,0.28)",
                transform: `translateX(-50%) rotate(${index * (360 / TICK_COUNT)}deg)`,
                transformOrigin: `50% ${TICK_RING_SIZE / 2}px`,
              }}
            />
          );
        })}
      </div>

      {nodePointer && (
        <div
          aria-hidden="true"
          style={{
            position: "absolute",
            left: nodePointer.left,
            top: nodePointer.top,
            zIndex: 2,
            width: NODE_POINTER_SIZE,
            height: NODE_POINTER_SIZE,
            opacity: minimapVisible ? 1 : 0,
            visibility: minimapVisible ? "visible" : "hidden",
            pointerEvents: "none",
            transform: `rotate(${nodePointer.rotate}deg)`,
            transition: "opacity 180ms ease, transform 180ms ease",
            filter: "drop-shadow(0 2px 5px rgba(0,0,0,0.38))",
          }}
        >
          <span
            style={{
              position: "absolute",
              inset: 3,
              display: "block",
              background: "rgba(248,248,248,0.9)",
              clipPath: "polygon(50% 0%, 90% 100%, 50% 78%, 10% 100%)",
            }}
          />
        </div>
      )}

      <div
        style={{
          ...SHELL_STYLE,
          position: "absolute",
          left: CIRCLE_LEFT,
          bottom: CIRCLE_BOTTOM,
          opacity: minimapVisible ? 1 : 0,
          visibility: minimapVisible ? "visible" : "hidden",
          pointerEvents: minimapVisible ? "auto" : "none",
          transition: "opacity 180ms ease",
        }}
        aria-label="画布小地图"
      >
        <svg width="100%" height="100%" viewBox={`0 0 ${LENS_SIZE} ${LENS_SIZE}`}>
          {minimapVisible && nodes.map((node) => {
            const nx = finiteOr(node.position.x, 0);
            const ny = finiteOr(node.position.y, 0);
            const px = projection.originX + nx * projection.mapScale;
            const py = projection.originY + ny * projection.mapScale;
            const pw = Math.max(NODE_W * projection.mapScale, 3);
            const ph = Math.max(NODE_H * projection.mapScale, 3);
            return (
              <rect
                key={node.id}
                x={px}
                y={py}
                width={pw}
                height={ph}
                rx={2}
                fill={node.id === selectedNodeId ? "rgba(139,124,247,0.6)" : statusColor[node.data.status ?? "idle"] ?? statusColor.idle}
                stroke={node.id === selectedNodeId ? "rgba(139,124,247,0.9)" : "transparent"}
                strokeWidth={node.id === selectedNodeId ? 1.5 : 0}
              />
            );
          })}
        </svg>
      </div>

      <div
        role="toolbar"
        aria-label="小地图控制"
        onPointerDown={(event) => event.stopPropagation()}
        onClick={(event) => event.stopPropagation()}
        style={{
          position: "absolute",
          // 圆可以继续贴出左下边界，但控制栏保持完整可见，避免左移后按钮被裁掉。
          left: Math.max(8, CIRCLE_CENTER_OFFSET - CONTROL_BAR_WIDTH / 2),
          bottom: Math.max(14, CIRCLE_BOTTOM + 20),
          width: CONTROL_BAR_WIDTH,
          height: CONTROL_BAR_HEIGHT,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-evenly",
          gap: 2,
          padding: "3px 5px",
          boxSizing: "border-box",
          border: "1px solid rgba(255,255,255,0.1)",
          borderRadius: 999,
          background: "rgba(36,39,45,0.82)",
          boxShadow: "0 10px 28px rgba(0,0,0,0.24)",
          backdropFilter: "blur(14px) saturate(1.2)",
          pointerEvents: "auto",
        }}
      >
        <HoverInfo content="缩小"><button type="button" onClick={() => changeZoom(-0.1)}  aria-label="缩小" style={CONTROL_BUTTON_STYLE}>
          <ZoomOut size={15} strokeWidth={1.8} />
        </button></HoverInfo>
        <HoverInfo content="适应屏幕"><button type="button" onClick={handleFitView}  aria-label="适应屏幕" style={CONTROL_BUTTON_STYLE}>
          <Maximize2 size={15} strokeWidth={1.8} />
        </button></HoverInfo>
        <HoverInfo content="放大"><button type="button" onClick={() => changeZoom(0.1)}  aria-label="放大" style={CONTROL_BUTTON_STYLE}>
          <ZoomIn size={15} strokeWidth={1.8} />
        </button></HoverInfo>
        <HoverInfo content={minimapVisible ? "隐藏小地图" : "显示小地图"}><button
          type="button"
          onClick={() => setMinimapVisible((visible) => !visible)}

          aria-label={minimapVisible ? "隐藏小地图" : "显示小地图"}
          style={CONTROL_BUTTON_STYLE}
        >
          {minimapVisible ? <Eye size={15} strokeWidth={1.8} /> : <EyeOff size={15} strokeWidth={1.8} />}
        </button></HoverInfo>
      </div>
    </div>
  );
}

const CONTROL_BUTTON_STYLE: React.CSSProperties = {
  width: 27,
  height: 27,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  padding: 0,
  border: 0,
  borderRadius: 8,
  color: "rgba(248,248,248,0.78)",
  background: "transparent",
  cursor: "pointer",
  transition: "background 150ms ease, color 150ms ease, transform 150ms ease",
};
