import type { CreativeNodeData } from "../../types";

interface Props {
  data: CreativeNodeData;
}

export function ImageGenContent({ data }: Props) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5">
        {data.imageModel && (
          <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(236,72,153,0.2)", color: "rgba(236,72,153,0.9)" }}>
            {data.imageModel}
          </span>
        )}
        {data.aspectRatio && (
          <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
            {data.aspectRatio}
          </span>
        )}
      </div>
      <div
        className="flex items-center justify-center rounded-lg"
        style={{
          height: 100,
          background: data.imageUrl ? "transparent" : "rgba(255,255,255,0.03)",
          border: data.imageUrl ? "none" : "1px dashed rgba(255,255,255,0.08)",
          overflow: "hidden",
        }}
      >
        {data.imageUrl ? (
          <img src={data.imageUrl} alt="" className="h-full w-full object-cover rounded-lg" />
        ) : (
          <span className="text-xs" style={{ color: "rgba(248,248,248,0.25)" }}>
            输入提示词生成图片
          </span>
        )}
      </div>
    </div>
  );
}
