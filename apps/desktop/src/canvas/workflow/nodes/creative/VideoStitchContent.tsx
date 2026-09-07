import type { CreativeNodeData } from "../../types";

interface Props {
  data: CreativeNodeData;
}

export function VideoStitchContent({ data }: Props) {
  const count = data.inputVideoUrls?.length ?? 0;
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
        {count > 0 ? `${count} 段视频` : "添加视频片段"}
      </span>
      <div className="flex gap-1">
        {[0, 1, 2].map((i) => (
          <div
            key={i}
            className="flex-1 flex items-center justify-center rounded"
            style={{
              height: 56,
              background: i < count ? "rgba(255,255,255,0.06)" : "rgba(255,255,255,0.02)",
              border: i < count ? "none" : "1px dashed rgba(255,255,255,0.06)",
            }}
          >
            {i < count ? (
              <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.5)" }}>#{i + 1}</span>
            ) : (
              <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.15)" }}>+</span>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
