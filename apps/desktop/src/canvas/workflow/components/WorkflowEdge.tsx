import { memo, useCallback, useState } from "react";
import {
  BaseEdge,
  EdgeLabelRenderer,
  getSmoothStepPath,
  type EdgeProps,
} from "@xyflow/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import {
  CREATIVE_NODE_CATEGORIES,
  PROFESSIONAL_NODE_CATEGORIES,
  type CreativeNodeKind,
  type ProfessionalNodeKind,
} from "../types";

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
  const [showInsert, setShowInsert] = useState(false);

  const creativeNodes = useCreativeStore((s) => s.nodes);
  const proNodes = useProfessionalStore((s) => s.nodes);
  const nodes = mode === "creative" ? creativeNodes : proNodes;
  const sourceNode = nodes.find((n) => n.id === source);
  const sourceStatus = sourceNode ? (sourceNode.data as { status?: string }).status : undefined;
  const isRunning = sourceStatus === "running";

  const [edgePath, labelX, labelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
    borderRadius: 12,
  });

  const handleInsert = useCallback(
    (kind: CreativeNodeKind | ProfessionalNodeKind) => {
      if (mode === "creative") {
        useCreativeStore.getState().insertNodeBetween(id, kind as CreativeNodeKind);
      } else {
        useProfessionalStore.getState().insertNodeBetween(id, kind as ProfessionalNodeKind);
      }
      setShowInsert(false);
    },
    [id, mode],
  );

  const categories = mode === "creative" ? CREATIVE_NODE_CATEGORIES : PROFESSIONAL_NODE_CATEGORIES;

  return (
    <>
      <BaseEdge
        id={id}
        path={edgePath}
        className={isRunning ? "wf-edge-running-path" : undefined}
        style={{
          ...style,
          stroke: isRunning ? "rgba(59,130,246,0.7)" : "rgba(148,163,184,0.25)",
          strokeWidth: isRunning ? 2 : 1.5,
          strokeDasharray: isRunning ? "8 4" : undefined,
          animation: isRunning ? "wfEdgeFlow 0.6s linear infinite" : undefined,
          filter: isRunning ? "drop-shadow(0 0 6px rgba(59,130,246,0.3))" : undefined,
        }}
      />
      <EdgeLabelRenderer>
        <div
          style={{
            position: "absolute",
            left: labelX - 12,
            top: labelY - 12,
            zIndex: 10,
          }}
          className="pointer-events-auto"
        >
          <button
            className="flex items-center justify-center rounded-full transition-all duration-200 hover:scale-110"
            style={{
              width: 24,
              height: 24,
              background: "rgba(59,130,246,0.15)",
              border: "1px solid rgba(59,130,246,0.3)",
              color: "rgba(59,130,246,0.8)",
              fontSize: 14,
              lineHeight: 1,
            }}
            onClick={() => setShowInsert((v) => !v)}
          >
            +
          </button>

          {showInsert && (
            <>
              <div className="fixed inset-0 z-[9998]" onClick={() => setShowInsert(false)} />
              <div
                className="fixed z-[9999] overflow-hidden rounded-xl"
                style={{
                  left: labelX - 100,
                  top: labelY + 18,
                  width: 200,
                  background: "rgba(30,30,35,0.92)",
                  border: "1px solid rgba(255,255,255,0.1)",
                  backdropFilter: "blur(40px)",
                  boxShadow: "0 12px 40px rgba(0,0,0,0.4)",
                  animation: "wfSlideIn 0.12s ease-out",
                }}
              >
                <div className="px-2.5 py-1.5" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
                  <span className="text-[10px] font-medium" style={{ color: "rgba(248,248,248,0.4)" }}>
                    插入节点
                  </span>
                </div>
                <div className="max-h-56 overflow-y-auto p-1">
                  {categories.map((cat) => (
                    <div key={cat.group} className="mb-0.5">
                      <div className="px-2 py-0.5">
                        <span
                          className="text-[9px] font-semibold uppercase tracking-wider"
                          style={{ color: "color" in cat ? cat.color : "rgba(248,248,248,0.3)" }}
                        >
                          {cat.group}
                        </span>
                      </div>
                      {cat.items.map((item) => (
                        <button
                          key={item.kind}
                          onClick={() => handleInsert(item.kind)}
                          className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left transition-colors duration-100 hover:bg-white/10"
                        >
                          <FontAwesomeIcon
                            icon={["fas", item.icon] as IconProp}
                            style={{
                              fontSize: 10,
                              color: "color" in cat ? cat.color : "rgba(248,248,248,0.6)",
                              width: 12,
                            }}
                          />
                          <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.8)" }}>
                            {item.label}
                          </span>
                        </button>
                      ))}
                    </div>
                  ))}
                </div>
              </div>
            </>
          )}
        </div>
      </EdgeLabelRenderer>
    </>
  );
}

export const WorkflowEdge = memo(WorkflowEdgeInner);
