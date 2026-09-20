import { AudioLines, Music } from "lucide-react";
import type { CreativeNodeData } from "../../types";
import { useCanvasMediaSrc } from "../../utils/canvasMedia";

interface Props {
  data: CreativeNodeData;
}

/**
 * 音频节点的卡片内容。
 *
 * 合成结果只是一个 `artifact://` 引用，播放地址由内核解析出真实文件后再交给
 * asset 协议，节点里永远不留音频字节。
 */
export function AudioContent({ data }: Props) {
  const playable = useCanvasMediaSrc(data.audioUrl ?? data.audioReference);
  const transcribed = data.audioOperation !== "speech_synthesize";
  const model = data.audioModel?.split("::").pop()?.trim();

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-1.5 text-[9px]">
        <span className="font-medium" style={{ color: "rgba(248,248,248,0.55)" }}>
          {transcribed ? "语音转文字" : "文字转语音"}
        </span>
        {model && <span style={{ color: "rgba(248,248,248,0.35)" }}>{model}</span>}
        {data.audioFormat && !transcribed && (
          <span style={{ color: "rgba(248,248,248,0.35)" }}>{data.audioFormat.toUpperCase()}</span>
        )}
      </div>

      {playable ? (
        <div
          className="flex items-center gap-2 rounded-lg px-2 py-2"
          style={{ background: "rgba(255,255,255,0.03)" }}
        >
          {transcribed ? (
            <AudioLines className="h-3.5 w-3.5 shrink-0" style={{ color: "rgba(248,248,248,0.35)" }} />
          ) : (
            <Music className="h-3.5 w-3.5 shrink-0" style={{ color: "rgba(248,248,248,0.35)" }} />
          )}
          <audio controls src={playable} className="h-8 w-full" />
        </div>
      ) : (
        <div
          className="flex items-center justify-center rounded-lg px-2 text-center"
          style={{
            height: 56,
            background: "rgba(255,255,255,0.03)",
            border: "1px dashed rgba(255,255,255,0.08)",
          }}
        >
          <span className="text-[11px]" style={{ color: "rgba(248,248,248,0.28)" }}>
            {transcribed ? "选择要转写的音频文件" : "输入文本并选择音色"}
          </span>
        </div>
      )}

      {transcribed && typeof data.output === "string" && data.output && (
        <p
          className="line-clamp-3 text-[11px] leading-relaxed"
          style={{ color: "rgba(248,248,248,0.6)" }}
          title={data.output}
        >
          {data.output}
        </p>
      )}
    </div>
  );
}
