import { useLayoutEffect, useRef } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { Connection } from "@xyflow/react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import {
  CREATIVE_NODE_CATEGORIES,
  PROFESSIONAL_NODE_CATEGORIES,
  type CreativeNodeKind,
  type ProfessionalNodeKind,
} from "../types";

export function NodePicker() {
  const mode = useCanvasStore((s) => s.mode);
  const nodePicker = useCanvasStore((s) => s.nodePicker);
  const pendingConnection = useCanvasStore((s) => s.pendingConnection);
  const closeNodePicker = useCanvasStore((s) => s.closeNodePicker);
  const addCreativeNode = useCreativeStore((s) => s.addNode);
  const addProfessionalNode = useProfessionalStore((s) => s.addNode);
  const panelRef = useRef<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    const el = panelRef.current;
    if (!el || !nodePicker) return;
    const { width, height } = el.getBoundingClientRect();
    const margin = 8;
    el.style.left = `${Math.max(margin, Math.min(nodePicker.x, window.innerWidth - width - margin))}px`;
    el.style.top = `${Math.max(margin, Math.min(nodePicker.y, window.innerHeight - height - margin))}px`;
  }, [nodePicker]);

  if (!nodePicker) return null;

  const handleSelect = (kind: CreativeNodeKind | ProfessionalNodeKind) => {
    let newNodeId: string;
    if (mode === "creative") {
      newNodeId = addCreativeNode(kind as CreativeNodeKind, nodePicker.worldX, nodePicker.worldY);
    } else {
      newNodeId = addProfessionalNode(kind as ProfessionalNodeKind, nodePicker.worldX, nodePicker.worldY);
    }
    if (pendingConnection) {
      const connection: Connection =
        pendingConnection.handleType === "source"
          ? { source: pendingConnection.nodeId, target: newNodeId, sourceHandle: null, targetHandle: null }
          : { source: newNodeId, target: pendingConnection.nodeId, sourceHandle: null, targetHandle: null };
      if (mode === "creative") useCreativeStore.getState().onConnect(connection);
      else useProfessionalStore.getState().onConnect(connection);
    }
    closeNodePicker();
  };

  const categories = mode === "creative" ? CREATIVE_NODE_CATEGORIES : PROFESSIONAL_NODE_CATEGORIES;

  return (
    <>
      <div className="fixed inset-0 z-[9998]" onClick={closeNodePicker} />
      <div
        ref={panelRef}
        className="fixed z-[9999] overflow-hidden rounded-2xl"
        style={{
          left: nodePicker.x,
          top: nodePicker.y,
          width: 240,
          background: "rgba(30,30,35,0.85)",
          border: "1px solid rgba(255,255,255,0.08)",
          backdropFilter: "blur(40px)",
          boxShadow: "0 24px 64px rgba(0,0,0,0.4)",
        }}
      >
        <div className="px-3 py-2" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
          <span className="text-xs font-medium" style={{ color: "rgba(248,248,248,0.45)" }}>
            添加节点
          </span>
        </div>
        <div className="max-h-80 overflow-y-auto p-1.5">
          {categories.map((cat) => (
            <div key={cat.group} className="mb-1">
              <div className="px-2 py-1">
                <span className="text-[10px] font-semibold uppercase tracking-wider" style={{ color: "color" in cat ? cat.color : "rgba(248,248,248,0.35)" }}>
                  {cat.group}
                </span>
              </div>
              {cat.items.map((item) => (
                <button
                  key={item.kind}
                  draggable
                  onDragStart={(e) => {
                    e.dataTransfer.setData("application/workflow-node-kind", item.kind);
                    e.dataTransfer.effectAllowed = "copy";
                  }}
                  onClick={() => handleSelect(item.kind)}
                  className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left transition-all duration-150 hover:bg-white/10"
                >
                  <FontAwesomeIcon
                    icon={["fas", item.icon] as IconProp}
                    style={{ fontSize: 12, color: "color" in cat ? cat.color : "rgba(248,248,248,0.7)", width: 16 }}
                  />
                  <span className="text-xs" style={{ color: "rgba(248,248,248,0.85)" }}>
                    {item.label}
                  </span>
                </button>
              ))}
            </div>
          ))}
        </div>
      </div>
    </>
  );
}
