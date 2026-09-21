import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";
import { GeneratingOverlay } from "./GeneratingOverlay";

interface Props {
  id: string;
  data: CreativeNodeData;
}

export function VideoGenContent({ id, data }: Props) {
  const videoSrc = useCanvasMediaSrc(data.videoUrl);
  return (
    <div className="relative overflow-hidden rounded-2xl">
      {videoSrc ? (
        <video src={videoSrc} className="block max-h-[260px] w-full" muted />
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
            输入提示词生成视频
          </span>
        </div>
      )}
      {data.status === "running" && <GeneratingOverlay nodeId={id} label="视频生成中…" />}
      {data.status === "running" && data.videoProgress != null && (
        <div className="absolute bottom-0 left-0 right-0 h-1 overflow-hidden">
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
  );
}
