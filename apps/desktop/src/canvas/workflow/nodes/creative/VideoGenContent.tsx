import type { CreativeNodeData } from "../../types";

interface Props {
  data: CreativeNodeData;
}

const SERVICE_COLORS: Record<string, string> = {
  sora: "rgba(16,185,129,0.9)",
  veo: "rgba(59,130,246,0.9)",
  kling: "rgba(245,158,11,0.9)",
};

export function VideoGenContent({ data }: Props) {
  const service = data.videoService ?? "sora";
  const serviceColor = SERVICE_COLORS[service] ?? SERVICE_COLORS.sora;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5">
        <span className="rounded px-1 py-0.5 text-[9px] font-medium uppercase" style={{ background: `${serviceColor}22`, color: serviceColor }}>
          {service}
        </span>
        {data.videoDuration && (
          <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
            {data.videoDuration}s
          </span>
        )}
      </div>
      <div
        className="flex items-center justify-center rounded-lg relative"
        style={{
          height: 100,
          background: data.videoUrl ? "transparent" : "rgba(255,255,255,0.03)",
          border: data.videoUrl ? "none" : "1px dashed rgba(255,255,255,0.08)",
          overflow: "hidden",
        }}
      >
        {data.videoUrl ? (
          <video src={data.videoUrl} className="h-full w-full object-cover rounded-lg" muted />
        ) : (
          <span className="text-xs" style={{ color: "rgba(248,248,248,0.25)" }}>
            输入提示词生成视频
          </span>
        )}
        {data.status === "running" && data.videoProgress != null && (
          <div className="absolute bottom-0 left-0 right-0 h-1 overflow-hidden rounded-b-lg">
            <div
              className="h-full transition-all duration-300"
              style={{
                width: `${data.videoProgress}%`,
                background: "linear-gradient(90deg, #3b82f6, #8b5cf6)",
              }}
            />
          </div>
        )}
      </div>
    </div>
  );
}
