import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

export function VideoEditContent({ data }: { data: CreativeNodeData }) {
  const hasFreshExport = data.videoEditStatus === "exported" && Boolean(data.videoEditOutputRef);
  const src = useCanvasMediaSrc(hasFreshExport ? data.videoEditOutputRef : data.videoEditInputRefs?.[0]);
  return (
    <div className="overflow-hidden rounded-lg bg-black/30">
      {src ? (
        <video src={src} controls className="nodrag nowheel max-h-48 w-full object-contain" />
      ) : (
        <div className="px-3 py-6 text-center text-xs text-white/45">尚未导入视频素材</div>
      )}
      <div className="px-2.5 py-2 text-[10px] text-white/50">
        {hasFreshExport ? "编辑导出 · 原始素材保持不变" : data.videoEditInputRefs?.length ? "源素材预览 · 尚未导出" : "尚未导入视频素材"}
      </div>
    </div>
  );
}
