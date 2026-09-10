import { memo } from "react";
import {
  BaseEdge,
  getBezierPath,
  type EdgeProps,
} from "@xyflow/react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";

function WorkflowEdgeInner({
  id,
  source,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  style,
}: EdgeProps) {
  const mode = useCanvasStore((s) => s.mode);

  const creativeNodes = useCreativeStore((s) => s.nodes);
  const proNodes = useProfessionalStore((s) => s.nodes);
  const nodes = mode === "creative" ? creativeNodes : proNodes;
  const sourceNode = nodes.find((n) => n.id === source);
  const sourceStatus = sourceNode ? (sourceNode.data as { status?: string }).status : undefined;
  const isRunning = sourceStatus === "running";

  const [edgePath] = getBezierPath({
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
  });

  return (
    <BaseEdge
      id={id}
      path={edgePath}
      className={isRunning ? "wf-edge-running-path" : undefined}
      style={{
        ...style,
        stroke: isRunning ? "rgba(59,130,246,0.7)" : "rgba(255,255,255,0.18)",
        strokeWidth: isRunning ? 2.4 : 1.8,
        strokeDasharray: isRunning ? "8 4" : undefined,
        animation: isRunning ? "wfEdgeFlow 0.6s linear infinite" : undefined,
        filter: isRunning ? "drop-shadow(0 0 6px rgba(59,130,246,0.3))" : undefined,
      }}
    />
  );
}

export const WorkflowEdge = memo(WorkflowEdgeInner);
