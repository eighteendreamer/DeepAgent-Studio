import { Music } from "lucide-react";
import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";
import { NodeTextOutput } from "../../components/NodeTextOutput";

/** 转写展示文本输出，合成展示音频输出；待转写文件留在配置面板。 */
export function AudioContent({ data }: { data: CreativeNodeData }) {
  const transcribed = data.audioOperation !== "speech_synthesize";
  const playable = useCanvasMediaSrc(transcribed ? undefined : data.audioUrl);
  if (transcribed) return <NodeTextOutput output={data.output} status={data.status} />;
  return playable ? (
    <div className="flex items-center gap-2 rounded-lg bg-white/[0.03] px-2 py-2">
      <Music className="h-3.5 w-3.5 shrink-0 text-white/35" />
      <audio controls src={playable} className="nodrag nopan nowheel h-8 w-full" />
    </div>
  ) : (
    <div className="flex h-14 items-center justify-center rounded-lg border border-dashed border-white/[0.08] bg-white/[0.03] text-[11px] text-white/30">
      {data.status === "running" ? "正在生成音频…" : "运行后显示音频"}
    </div>
  );
}
