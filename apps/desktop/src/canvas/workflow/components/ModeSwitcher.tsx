import { useCanvasStore } from "../store/canvasStore";
import type { CanvasMode } from "../types";

const MODES: Array<{ key: CanvasMode; label: string }> = [
  { key: "creative", label: "创作" },
  { key: "professional", label: "专业" },
];

export function ModeSwitcher() {
  const mode = useCanvasStore((s) => s.mode);
  const setMode = useCanvasStore((s) => s.setMode);

  return (
    <div
      className="fixed left-6 top-4 z-[9997] flex items-center gap-0.5 rounded-xl p-1"
      style={{
        background: "rgba(76,80,82,0.55)",
        border: "1px solid rgba(255,255,255,0.08)",
        backdropFilter: "blur(40px)",
        boxShadow: "rgba(0,0,0,0.11) 0px 20px 84px, rgba(0,0,0,0.15) 0px 33px 139px",
      }}
    >
      {MODES.map((m) => (
        <button
          key={m.key}
          onClick={() => setMode(m.key)}
          className="rounded-lg px-3 py-1.5 text-xs font-medium transition-all duration-300"
          style={{
            color: mode === m.key ? "rgb(248,248,248)" : "rgba(248,248,248,0.45)",
            background: mode === m.key ? "rgba(255,255,255,0.15)" : "transparent",
          }}
        >
          {m.label}
        </button>
      ))}
    </div>
  );
}
