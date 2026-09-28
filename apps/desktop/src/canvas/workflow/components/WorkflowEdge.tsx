import { memo, useState } from "react";
import {
  getBezierPath,
  type EdgeProps,
  useReactFlow,
} from "@xyflow/react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { HoverInfo } from "../../../components/ui/HoverInfo";

const MAGIC_BEAM_GRADIENT_START = "#ffaa40";
const MAGIC_BEAM_GRADIENT_STOP = "#9c40ff";

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
  const [hovered, setHovered] = useState(false);
  const { deleteElements } = useReactFlow();
  const mode = useCanvasStore((s) => s.mode);

  const creativeNodes = useCreativeStore((s) => s.nodes);
  const proNodes = useProfessionalStore((s) => s.nodes);
  const nodes = mode === "creative" ? creativeNodes : proNodes;
  const sourceNode = nodes.find((n) => n.id === source);
  const sourceStatus = sourceNode ? (sourceNode.data as { status?: string }).status : undefined;
  const isRunning = sourceStatus === "running";

  const [edgePath, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
  });

  const handleCut = () => {
    deleteElements({ edges: [{ id }] });
  };

  const beamGradientId = `wf-beam-${id}`;
  const beamWidth = 120;
  const beamStartX = sourceX;
  const beamEndX = targetX;

  return (
    <>
      {/* 不可见宽线用于 hover/click 检测 */}
      <path
        d={edgePath}
        fill="none"
        stroke="transparent"
        strokeWidth={22}
        className="pointer-events-auto cursor-pointer"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      />
      {/* 可见连线 */}
      <path
        d={edgePath}
        fill="none"
        stroke={hovered ? "rgba(255,255,255,0.48)" : "rgba(255,255,255,0.18)"}
        strokeWidth={hovered ? 2.2 : 1.8}
        strokeLinecap="round"
        style={{
          pointerEvents: "none",
          transition: "stroke 0.18s ease, stroke-width 0.18s ease, filter 0.18s ease",
          filter: hovered ? "drop-shadow(0 0 5px rgba(255,255,255,0.22))" : "none",
          ...style,
        }}
      />
      {/* 运行时光束 */}
      {isRunning && (
        <>
          <defs>
            <linearGradient
              id={beamGradientId}
              gradientUnits="userSpaceOnUse"
              x1={beamStartX}
              x2={beamStartX + beamWidth}
              y1={sourceY}
              y2={targetY}
            >
              <stop offset="0%" stopColor={MAGIC_BEAM_GRADIENT_START} stopOpacity="0" />
              <stop offset="32%" stopColor={MAGIC_BEAM_GRADIENT_START} stopOpacity="0.95" />
              <stop offset="68%" stopColor={MAGIC_BEAM_GRADIENT_STOP} stopOpacity="0.95" />
              <stop offset="100%" stopColor={MAGIC_BEAM_GRADIENT_STOP} stopOpacity="0" />
              <animate
                attributeName="x1"
                values={`${beamStartX};${beamEndX - beamWidth}`}
                dur="5s"
                repeatCount="indefinite"
              />
              <animate
                attributeName="x2"
                values={`${beamStartX + beamWidth};${beamEndX}`}
                dur="5s"
                repeatCount="indefinite"
              />
            </linearGradient>
          </defs>
          <path
            d={edgePath}
            fill="none"
            stroke={`url(#${beamGradientId})`}
            strokeWidth={3}
            strokeLinecap="round"
            style={{
              pointerEvents: "none",
              filter: "drop-shadow(0 0 10px rgba(156,64,255,0.38)) drop-shadow(0 0 5px rgba(255,170,64,0.34))",
            }}
          />
        </>
      )}
      {/* 剪切按钮 */}
      {hovered && !isRunning && (
        <HoverInfo content="断开连线" side="top"><g
          transform={`translate(${labelX} ${labelY})`}
          style={{ cursor: "pointer", pointerEvents: "auto" }}
          onClick={(e) => {
            e.stopPropagation();
            handleCut();
          }}
          onMouseEnter={() => setHovered(true)}
          onMouseLeave={() => setHovered(false)}
        >
          <circle r="13" fill="rgba(239,68,68,0.12)" />
          <circle
            r="10"
            fill="rgba(15,23,42,0.95)"
            stroke="#ef4444"
            strokeWidth="1.5"
          />
          <g stroke="#ef4444" strokeWidth="1.5" strokeLinecap="round" fill="none">
            <circle cx="-2.5" cy="-2.5" r="1.6" />
            <circle cx="-2.5" cy="2.5" r="1.6" />
            <line x1="-1" y1="-1" x2="4.5" y2="4" />
            <line x1="-1" y1="1" x2="4.5" y2="-4" />
          </g>
        </g></HoverInfo>
      )}
    </>
  );
}

export const WorkflowEdge = memo(WorkflowEdgeInner);
