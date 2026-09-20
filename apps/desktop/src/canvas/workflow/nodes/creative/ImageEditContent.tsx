import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

interface Props {
  data: CreativeNodeData;
}

const EDIT_MODES: Record<string, { label: string; color: string }> = {
  crop: { label: "裁剪", color: "rgba(59,130,246,0.8)" },
  "remove-bg": { label: "去背景", color: "rgba(16,185,129,0.8)" },
  upscale: { label: "超分", color: "rgba(245,158,11,0.8)" },
  repaint: { label: "重绘", color: "rgba(139,92,246,0.8)" },
};

export function ImageEditContent({ data }: Props) {
  const mode = data.editMode ?? "crop";
  const src = useCanvasMediaSrc(data.imageUrl);
  const modeInfo = EDIT_MODES[mode] ?? EDIT_MODES.crop;
  return (
    <div className="flex flex-col gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium self-start" style={{ background: `${modeInfo.color}22`, color: modeInfo.color }}>
        {modeInfo.label}
      </span>
      <div
        className="flex items-center justify-center rounded-lg"
        style={{
          height: 80,
          background: data.imageUrl ? "transparent" : "rgba(255,255,255,0.03)",
          border: data.imageUrl ? "none" : "1px dashed rgba(255,255,255,0.08)",
          overflow: "hidden",
        }}
      >
        {src ? (
          <img src={src} alt="" className="h-full w-full object-cover rounded-lg" />
        ) : (
          <span className="text-xs" style={{ color: "rgba(248,248,248,0.25)" }}>
            选择图片进行编辑
          </span>
        )}
      </div>
    </div>
  );
}
