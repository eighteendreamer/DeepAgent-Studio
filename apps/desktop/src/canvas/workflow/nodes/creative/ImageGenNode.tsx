import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";
import { GeneratingOverlay } from "./GeneratingOverlay";

interface Props {
  id: string;
  data: CreativeNodeData;
}

export function ImageGenContent({ id, data }: Props) {
  const src = useCanvasMediaSrc(data.imageUrl);

  return (
    <div className="relative overflow-hidden rounded-2xl">
      {src ? (
        <img
          src={src}
          alt=""
          className="block max-h-[260px] w-full object-contain"
          draggable={false}
        />
      ) : (
        <div
          className="flex items-center justify-center"
          style={{
            height: 100,
            background: "rgba(255,255,255,0.03)",
            border: "1px dashed rgba(255,255,255,0.08)",
          }}
        >
          <span className="text-xs" style={{ color: "rgba(248,248,248,0.25)" }}>
            输入提示词生成图片
          </span>
        </div>
      )}
      {data.status === "running" && <GeneratingOverlay nodeId={id} label="图片生成中…" />}
    </div>
  );
}
