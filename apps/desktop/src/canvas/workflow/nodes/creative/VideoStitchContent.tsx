import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

export function VideoStitchContent({ data }: { data: CreativeNodeData }) {
  const src = useCanvasMediaSrc(data.videoUrl);
  return src ? (
    <video src={src} className="nodrag nopan nowheel block max-h-[260px] w-full rounded-lg" controls />
  ) : (
    <div className="flex h-14 items-center justify-center rounded-lg border border-dashed border-white/[0.08] bg-white/[0.03] text-xs text-white/30">
      {data.status === "running" ? "正在拼接…" : "运行后显示拼接结果"}
    </div>
  );
}
