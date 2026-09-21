import type { CreativeNodeData } from "../../types";

interface Props {
  data: CreativeNodeData;
}

export function ScriptGenContent({ data }: Props) {
  const preview = data.prompt || data.output;
  return (
    <div className="flex flex-col gap-1.5">
      <div
        className="rounded-lg px-2.5 py-2 text-xs leading-relaxed font-mono"
        style={{
          background: "rgba(255,255,255,0.03)",
          color: preview ? "rgba(248,248,248,0.7)" : "rgba(248,248,248,0.25)",
          minHeight: 48,
          maxHeight: 80,
          overflow: "hidden",
        }}
      >
        {preview || "生成视频脚本/分镜..."}
      </div>
    </div>
  );
}
