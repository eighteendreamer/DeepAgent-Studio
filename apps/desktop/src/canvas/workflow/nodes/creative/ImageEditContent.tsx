import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

interface Props {
  data: CreativeNodeData;
}

export function ImageEditContent({ data }: Props) {
  const src = useCanvasMediaSrc(data.imageUrl);
  return (
    <div className="flex flex-col gap-1.5">
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
