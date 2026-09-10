import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNode } from "../types";

const SIZE = 220;
const PAD = 20;
const NODE_W = 240;
const NODE_H = 120;
// Estimated canvas container size for viewport calculation
const EST_CONTAINER_W = 1200;
const EST_CONTAINER_H = 800;

const SHELL_STYLE: React.CSSProperties = {
  width: SIZE,
  height: SIZE,
  background: "rgba(30,30,35,0.45)",
  border: "1px solid rgba(255,255,255,0.08)",
  boxShadow: "0 18px 44px rgba(0,0,0,0.24)",
  backdropFilter: "blur(16px) saturate(1.18)",
  overflow: "hidden",
  clipPath: "circle(100% at 0% 100%)",
};

interface ProjectedNode {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
  status?: string;
}

interface ViewportRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

function projectNodes(
  nodes: WorkflowNode[],
  viewport: { x: number; y: number; zoom: number },
): { nodes: ProjectedNode[]; viewport: ViewportRect | null; scale: number; minX: number; minY: number } {
  if (nodes.length === 0) return { nodes: [], viewport: null, scale: 1, minX: 0, minY: 0 };

  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const n of nodes) {
    minX = Math.min(minX, n.position.x);
    minY = Math.min(minY, n.position.y);
    maxX = Math.max(maxX, n.position.x + NODE_W);
    maxY = Math.max(maxY, n.position.y + NODE_H);
  }

  // Add padding around the bounding box
  const padX = (maxX - minX) * 0.15;
  const padY = (maxY - minY) * 0.15;
  minX -= padX;
  minY -= padY;
  maxX += padX;
  maxY += padY;

  const worldW = maxX - minX || 1;
  const worldH = maxY - minY || 1;
  const scale = Math.min((SIZE - PAD * 2) / worldW, (SIZE - PAD * 2) / worldH, 0.5);

  const projected = nodes.map((n) => ({
    id: n.id,
    x: PAD + (n.position.x - minX) * scale,
    y: PAD + (n.position.y - minY) * scale,
    w: NODE_W * scale,
    h: NODE_H * scale,
    status: n.data.status,
  }));

  // Viewport rect in projected coordinates
  const vpW = EST_CONTAINER_W / viewport.zoom;
  const vpH = EST_CONTAINER_H / viewport.zoom;
  const vpRect: ViewportRect = {
    x: PAD + (viewport.x - minX) * scale,
    y: PAD + (viewport.y - minY) * scale,
    w: vpW * scale,
    h: vpH * scale,
  };

  return { nodes: projected, viewport: vpRect, scale, minX, minY };
}

export function MiniMap() {
  const mode = useCanvasStore((s) => s.mode);
  const viewport = useCanvasStore((s) => s.viewport);
  const creativeNodes = useCreativeStore((s) => s.nodes);
  const professionalNodes = useProfessionalStore((s) => s.nodes);
  const selectedNodeId = useCanvasStore((s) => s.selectedNodeId);

  const nodes = mode === "creative" ? creativeNodes : professionalNodes;
  const { nodes: projected, viewport: vpRect } = projectNodes(nodes, viewport);

  const statusColor: Record<string, string> = {
    idle: "rgba(255,255,255,0.2)",
    running: "rgba(59,130,246,0.7)",
    completed: "rgba(34,197,94,0.7)",
    error: "rgba(239,68,68,0.7)",
  };

  return (
    <div className="absolute left-0 bottom-0 z-[80]" style={{ width: SIZE, height: SIZE }}>
      <div style={SHELL_STYLE}>
        <svg width="100%" height="100%" viewBox={`0 0 ${SIZE} ${SIZE}`}>
          {/* Node rectangles */}
          {projected.map((n) => (
            <rect
              key={n.id}
              x={n.x}
              y={n.y}
              width={Math.max(n.w, 4)}
              height={Math.max(n.h, 4)}
              rx={2}
              fill={n.id === selectedNodeId ? "rgba(139,124,247,0.6)" : statusColor[n.status ?? "idle"] ?? statusColor.idle}
              stroke={n.id === selectedNodeId ? "rgba(139,124,247,0.9)" : "transparent"}
              strokeWidth={n.id === selectedNodeId ? 1.5 : 0}
            />
          ))}
          {/* Viewport indicator */}
          {vpRect && (
            <rect
              x={vpRect.x}
              y={vpRect.y}
              width={vpRect.w}
              height={vpRect.h}
              rx={2}
              fill="rgba(139,124,247,0.08)"
              stroke="rgba(139,124,247,0.5)"
              strokeWidth={1.5}
            />
          )}
        </svg>
      </div>
    </div>
  );
}
