import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNode } from "../types";

const LENS_WIDTH = 268;
const LENS_HEIGHT = 168;
const MAP_PADDING = 12;
const MAP_SCALE_MAX = 0.5;
const NODE_W = 240;
const NODE_H = 120;

const SHELL_STYLE: React.CSSProperties = {
  width: LENS_WIDTH,
  height: LENS_HEIGHT,
  background: "rgba(30,30,35,0.45)",
  border: "1px solid rgba(255,255,255,0.08)",
  boxShadow: "0 18px 44px rgba(0,0,0,0.24)",
  backdropFilter: "blur(16px) saturate(1.18)",
  overflow: "hidden",
  clipPath: "circle(100% at 0% 100%)",
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
  viewportRect: { left: number; top: number; width: number; height: number };
}

function finiteOr(value: number, fallback: number) {
  return Number.isFinite(value) ? value : fallback;
}

function computeWorldBounds(
  nodes: WorkflowNode[],
  viewportBounds: Bounds,
): Bounds {
  let { minX, minY, maxX, maxY } = viewportBounds;
  for (const n of nodes) {
    const nx = finiteOr(n.position.x, 0);
    const ny = finiteOr(n.position.y, 0);
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
  viewportBounds: Bounds,
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
    viewportRect: {
      left: originX + viewportBounds.minX * mapScale,
      top: originY + viewportBounds.minY * mapScale,
      width: viewportBounds.width * mapScale,
      height: viewportBounds.height * mapScale,
    },
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
  const proj = computeProjection(worldBounds, viewportBounds, LENS_WIDTH, LENS_HEIGHT);

  const statusColor: Record<string, string> = {
    idle: "rgba(255,255,255,0.2)",
    running: "rgba(59,130,246,0.7)",
    completed: "rgba(34,197,94,0.7)",
    error: "rgba(239,68,68,0.7)",
  };

  return (
    <div className="absolute left-0 bottom-0 z-[80]" style={{ width: LENS_WIDTH + 20, height: LENS_HEIGHT + 20 }}>
      <div style={{ ...SHELL_STYLE, marginLeft: 6, marginTop: 6 }}>
        <svg width="100%" height="100%" viewBox={`0 0 ${LENS_WIDTH} ${LENS_HEIGHT}`}>
          {nodes.map((n) => {
            const nx = finiteOr(n.position.x, 0);
            const ny = finiteOr(n.position.y, 0);
            const px = proj.originX + nx * proj.mapScale;
            const py = proj.originY + ny * proj.mapScale;
            const pw = Math.max(NODE_W * proj.mapScale, 3);
            const ph = Math.max(NODE_H * proj.mapScale, 3);
            return (
              <rect
                key={n.id}
                x={px}
                y={py}
                width={pw}
                height={ph}
                rx={2}
                fill={n.id === selectedNodeId ? "rgba(139,124,247,0.6)" : statusColor[n.data.status ?? "idle"] ?? statusColor.idle}
                stroke={n.id === selectedNodeId ? "rgba(139,124,247,0.9)" : "transparent"}
                strokeWidth={n.id === selectedNodeId ? 1.5 : 0}
              />
            );
          })}
          <rect
            x={proj.viewportRect.left}
            y={proj.viewportRect.top}
            width={proj.viewportRect.width}
            height={proj.viewportRect.height}
            rx={2}
            fill="rgba(139,124,247,0.06)"
            stroke="rgba(139,124,247,0.45)"
            strokeWidth={1.5}
          />
        </svg>
      </div>
    </div>
  );
}
