import { useCallback, useEffect, useState } from "react";
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

interface ContextMenuState {
  x: number;
  y: number;
  worldX: number;
  worldY: number;
}

export function useContextMenu() {
  const [menu, setMenu] = useState<ContextMenuState | null>(null);
  const close = useCallback(() => setMenu(null), []);
  return { menu, openMenu: setMenu, close };
}

interface Props {
  menu: ContextMenuState | null;
  onClose: () => void;
}

export function ContextMenu({ menu, onClose }: Props) {
  const mode = useCanvasStore((s) => s.mode);
  const addCreativeNode = useCreativeStore((s) => s.addNode);
  const addProfessionalNode = useProfessionalStore((s) => s.addNode);

  useEffect(() => {
    if (!menu) return;
    const handleEsc = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handleEsc);
    return () => window.removeEventListener("keydown", handleEsc);
  }, [menu, onClose]);

  if (!menu) return null;

  const handleSelect = (kind: CreativeNodeKind | ProfessionalNodeKind) => {
    if (mode === "creative") {
      addCreativeNode(kind as CreativeNodeKind, menu.worldX, menu.worldY);
    } else {
      addProfessionalNode(kind as ProfessionalNodeKind, menu.worldX, menu.worldY);
    }
    onClose();
  };

  const categories = mode === "creative" ? CREATIVE_NODE_CATEGORIES : PROFESSIONAL_NODE_CATEGORIES;

  return (
    <>
      <div className="fixed inset-0 z-[9998]" onClick={onClose} onContextMenu={(e) => { e.preventDefault(); onClose(); }} />
      <div
        className="fixed z-[9999] overflow-hidden rounded-xl"
        style={{
          left: menu.x,
          top: menu.y,
          width: 220,
          background: "rgba(30,30,35,0.9)",
          border: "1px solid rgba(255,255,255,0.1)",
          backdropFilter: "blur(40px)",
          boxShadow: "0 16px 48px rgba(0,0,0,0.45)",
          animation: "wfSlideIn 0.12s ease-out",
        }}
      >
        <div className="px-3 py-1.5" style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}>
          <span className="text-[10px] font-medium" style={{ color: "rgba(248,248,248,0.4)" }}>
            添加节点
          </span>
        </div>
        <div className="max-h-72 overflow-y-auto p-1">
          {categories.map((cat) => (
            <div key={cat.group} className="mb-0.5">
              <div className="px-2 py-0.5">
                <span className="text-[9px] font-semibold uppercase tracking-wider" style={{ color: "color" in cat ? cat.color : "rgba(248,248,248,0.3)" }}>
                  {cat.group}
                </span>
              </div>
              {cat.items.map((item) => (
                <button
                  key={item.kind}
                  onClick={() => handleSelect(item.kind)}
                  className="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left transition-colors duration-100 hover:bg-white/10"
                >
                  <FontAwesomeIcon
                    icon={["fas", item.icon] as IconProp}
                    style={{ fontSize: 11, color: "color" in cat ? cat.color : "rgba(248,248,248,0.6)", width: 14 }}
                  />
                  <span className="text-[11px]" style={{ color: "rgba(248,248,248,0.8)" }}>
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
