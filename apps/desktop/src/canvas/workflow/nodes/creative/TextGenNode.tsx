import { useCreativeStore } from "../../store/creativeStore";
import { Sparkles } from "lucide-react";
import type { CreativeNodeData } from "../../types";

interface Props {
  id: string;
  data: CreativeNodeData;
}

export function TextGenContent({ id, data }: Props) {
  const isImageToPrompt = data.creativeActionKey === "image-to-prompt";
  const upstreamImage = useCreativeStore((state) => {
    if (!isImageToPrompt) return "";
    const edge = state.edges.find((item) => item.target === id);
    const source = edge ? state.nodes.find((node) => node.id === edge.source)?.data.imageUrl : undefined;
    return typeof source === "string" ? source : "";
  });
  return (
    <div className="flex flex-col gap-1.5">
      {isImageToPrompt && (
        <div className="flex items-center gap-2 rounded-lg bg-white/[0.025] px-2 py-1.5">
          {upstreamImage ? (
            <img
              src={upstreamImage}
              alt="待反推图片"
              draggable={false}
              onDragStart={(event) => event.preventDefault()}
              className="pointer-events-none h-9 w-9 select-none rounded object-cover"
            />
          ) : (
            <div className="flex h-9 w-9 items-center justify-center rounded bg-white/[0.05] text-white/35">
              <Sparkles className="h-3.5 w-3.5" />
            </div>
          )}
          <span className="truncate text-[10px] text-white/45">{upstreamImage ? "已连接图片输入" : "等待图片输入"}</span>
        </div>
      )}
      {data.prompt ? (
        <div className="rounded-lg bg-white/[0.025] px-2.5 py-2 text-xs leading-relaxed text-white/65" style={{ maxHeight: 72, overflow: "hidden" }}>
          {data.prompt}
        </div>
      ) : (
        <div className="px-2.5 py-2 text-[11px] text-white/30">点击节点后在下方编辑</div>
      )}
      {data.output && (
        <div className="rounded-lg border border-white/[0.08] bg-white/[0.02] px-2.5 py-2 text-[11px] leading-relaxed text-white/60">
          {data.output}
        </div>
      )}
    </div>
  );
}
