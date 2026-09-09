import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNode } from "../types";

const SHELL_STYLE: React.CSSProperties = {
  width: 280,
  height: 180,
  background: "rgba(30,30,35,0.45)",
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: 16,
  boxShadow: "0 18px 44px rgba(0,0,0,0.24)",
  backdropFilter: "blur(16px) saturate(1.18)",
  overflow: "hidden",
};

function projectNodes(nodes: WorkflowNode[], width: number, height: number) {
  if (nodes.length === 0) return [];
  const pad = 24;
  let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
  for (const n of nodes) {
    minX = Math.min(minX, n.position.x);
    minY = Math.min(minY, n.position.y);
    maxX = Math.max(maxX, n.position.x + 240);
    maxY = Math.max(maxY, n.position.y + 120);
  }
  const worldW = maxX - minX || 1;
  const worldH = maxY - minY || 1;
  const scale = Math.min((width - pad * 2) / worldW, (height - pad * 2) / worldH, 0.5);
  return nodes.map((n) => ({
    id: n.id,
    x: pad + (n.position.x - minX) * scale,
    y: pad + (n.position.y - minY) * scale,
    w: 240 * scale,
    h: 120 * scale,
    status: n.data.status,
  }));
}

export function MiniMap() {
  const mode = useCanvasStore((s) => s.mode);
  const creativeNodes = useCreativeStore((s) => s.nodes);
  const professionalNodes = useProfessionalStore((s) => s.nodes);
  const selectedNodeId = useCanvasStore((s) => s.selectedNodeId);

  const nodes = mode === "creative" ? creativeNodes : professionalNodes;
  const projected = projectNodes(nodes, 268, 168);

  const statusColor: Record<string, string> = {
    idle: "rgba(255,255,255,0.2)",
    running: "rgba(59,130,246,0.7)",
    completed: "rgba(34,197,94,0.7)",
    error: "rgba(239,68,68,0.7)",
  };

  return (
    <div className="absolute left-6 bottom-16 z-[80]" style={{ width: 280, height: 180 }}>
      <div style={SHELL_STYLE}>
        <svg width="100%" height="100%" viewBox="0 0 268 168">
          {projected.map((n) => (
            <rect
              key={n.id}
              x={n.x}
              y={n.y}
              width={Math.max(n.w, 4)}
              height={Math.max(n.h, 4)}
              rx={2}
              fill={n.id === selectedNodeId ? "rgba(139,124,247,0.6)" : statusColor[n.status] ?? statusColor.idle}
              stroke={n.id === selectedNodeId ? "rgba(139,124,247,0.9)" : "transparent"}
              strokeWidth={n.id === selectedNodeId ? 1.5 : 0}
            />
          ))}
        </svg>
      </div>
    </div>
  );
}
