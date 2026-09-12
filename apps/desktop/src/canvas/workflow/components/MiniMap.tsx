import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNode } from "../types";

const LENS_SIZE = 360;
const MAP_PADDING = 12;
const MAP_SCALE_MAX = 0.5;
const NODE_W = 240;
const NODE_H = 120;

// 圆心从左下角直角点沿 x=y 方向向右上偏移，避免只露出一个贴边的四分之一圆。
const CIRCLE_CENTER_OFFSET = 128;
const CIRCLE_LEFT = CIRCLE_CENTER_OFFSET - LENS_SIZE / 2;
const CIRCLE_BOTTOM = CIRCLE_CENTER_OFFSET - LENS_SIZE / 2;

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

interface Props {
  containerWidth: number;
  containerHeight: number;
}

export function MiniMap({ containerWidth, containerHeight }: Props) {
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
  const statusColor: Record<string, string> = {
    idle: "rgba(255,255,255,0.2)",
    running: "rgba(59,130,246,0.7)",
    completed: "rgba(34,197,94,0.7)",
    error: "rgba(239,68,68,0.7)",
  };

  return (
    <div
      className="absolute left-0 bottom-0 z-[80] pointer-events-none"
      style={{ width: LENS_SIZE, height: LENS_SIZE, overflow: "visible" }}
    >
      <div
        style={{
          ...SHELL_STYLE,
          position: "absolute",
          left: CIRCLE_LEFT,
          bottom: CIRCLE_BOTTOM,
          pointerEvents: "auto",
        }}
        aria-label="画布小地图"
      >
        <svg width="100%" height="100%" viewBox={`0 0 ${LENS_SIZE} ${LENS_SIZE}`}>
          {nodes.map((node) => {
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
    </div>
  );
}
