import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

export function VideoEditContent({ data }: { data: CreativeNodeData }) {
  const reference = data.videoEditOutputRef ?? data.videoEditInputRefs?.[0] ?? data.videoUrl;
  const src = useCanvasMediaSrc(reference);
  if (!src) return <div className="rounded-lg border border-dashed border-white/15 px-3 py-6 text-center text-xs text-white/45">导入或连接视频后开始编辑</div>;
  return <div className="overflow-hidden rounded-lg bg-black/30"><video src={src} controls className="max-h-48 w-full object-contain" /><div className="flex justify-between px-2.5 py-2 text-[10px] text-white/50"><span>{data.videoEditStatus === "exported" ? "已导出编辑结果" : "编辑草稿"}</span><span className="font-mono">{data.videoEditProjectId?.slice(0, 8)}</span></div></div>;
}
