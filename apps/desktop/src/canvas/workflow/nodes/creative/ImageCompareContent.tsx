import type { CreativeNodeData } from "../../types";

interface Props {
  data: CreativeNodeData;
}

export function ImageCompareContent({ data }: Props) {
  return (
    <div className="flex gap-1">
      <div
        className="flex-1 flex items-center justify-center rounded-lg"
        style={{
          height: 80,
          background: data.leftImageUrl ? "transparent" : "rgba(255,255,255,0.03)",
          border: data.leftImageUrl ? "none" : "1px dashed rgba(255,255,255,0.08)",
          overflow: "hidden",
        }}
      >
        {data.leftImageUrl ? (
          <img src={data.leftImageUrl} alt="" className="h-full w-full object-cover rounded-lg" />
        ) : (
          <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.2)" }}>A</span>
        )}
      </div>
      <div
        className="flex items-center"
        style={{ color: "rgba(248,248,248,0.2)" }}
      >
        <span className="text-[10px]">vs</span>
      </div>
      <div
        className="flex-1 flex items-center justify-center rounded-lg"
        style={{
          height: 80,
          background: data.rightImageUrl ? "transparent" : "rgba(255,255,255,0.03)",
          border: data.rightImageUrl ? "none" : "1px dashed rgba(255,255,255,0.08)",
          overflow: "hidden",
        }}
      >
        {data.rightImageUrl ? (
          <img src={data.rightImageUrl} alt="" className="h-full w-full object-cover rounded-lg" />
        ) : (
          <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.2)" }}>B</span>
        )}
      </div>
    </div>
  );
}
