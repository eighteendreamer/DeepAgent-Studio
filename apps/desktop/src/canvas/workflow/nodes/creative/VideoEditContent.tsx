import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

export function VideoEditContent({ data }: { data: CreativeNodeData }) {
  const src = useCanvasMediaSrc(data.videoEditInputRefs?.[0]);
  return (
    <div className="overflow-hidden rounded-lg bg-black/30">
      {src ? (
        <video src={src} controls className="nodrag nowheel max-h-48 w-full object-contain" />
      ) : (
        <div className="px-3 py-6 text-center text-xs text-white/45">尚未导入视频素材</div>
      )}
      <div className="px-2.5 py-2 text-[10px] text-white/50">素材预览 · 编辑器待接通</div>
    </div>
  );
}
