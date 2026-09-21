import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

interface Props {
  data: CreativeNodeData;
}

export function ImageGenContent({ data }: Props) {
  const model = data.imageModel?.trim();
  const src = useCanvasMediaSrc(data.imageUrl);

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5">
        {model && (
          <span className="truncate text-[9px] font-medium" style={{ color: "rgba(248,248,248,0.55)" }}>
            {model}
          </span>
        )}
        {data.aspectRatio && (
          <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
            {data.aspectRatio}
          </span>
        )}
      </div>
      {src ? (
        <img
          src={src}
          alt=""
          className="block max-h-[260px] w-full rounded-lg object-contain"
          draggable={false}
        />
      ) : (
        <div
          className="flex items-center justify-center rounded-lg"
          style={{
            height: 100,
            background: "rgba(255,255,255,0.03)",
            border: "1px dashed rgba(255,255,255,0.08)",
            overflow: "hidden",
          }}
        >
          <span className="text-xs" style={{ color: "rgba(248,248,248,0.25)" }}>
            输入提示词生成图片
          </span>
        </div>
      )}
    </div>
  );
}
