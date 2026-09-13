import { useMemo } from "react";
import { Play, Sparkles } from "lucide-react";
import type { CreativeNodeData } from "../types";
import { useCreativeStore } from "../store/creativeStore";
import { runWorkflow } from "../utils/workflowExecutor";

interface Props {
  nodeId: string;
  data: CreativeNodeData;
  onUpdate: (patch: Record<string, unknown>) => void;
}

function stopPanelGesture(event: React.SyntheticEvent) {
  event.stopPropagation();
}

export function TextGenEditorPanel({ nodeId, data, onUpdate }: Props) {
  const isImageToPrompt = data.creativeActionKey === "image-to-prompt";
  const upstreamImage = useCreativeStore((state) => {
    if (!isImageToPrompt) return "";
    const edge = state.edges.find((item) => item.target === nodeId);
    const source = edge ? state.nodes.find((node) => node.id === edge.source)?.data.imageUrl : undefined;
    return typeof source === "string" ? source : "";
  });
  const placeholder = useMemo(
    () => (isImageToPrompt ? "描述要从图片中反推出的提示词，也可以手动补充..." : "描述你想要生成的内容，并在下方调整生成参数..."),
    [isImageToPrompt],
  );

  return (
    <div
      className="w-full overflow-hidden rounded-xl border border-white/10 bg-[#202124]/95 shadow-[0_18px_48px_rgba(0,0,0,0.38)] backdrop-blur-2xl"
      onPointerDown={stopPanelGesture}
      onMouseDown={stopPanelGesture}
    >
      {isImageToPrompt && (
        <div className="flex items-center gap-2 px-3.5 pt-3">
          <span className="shrink-0 text-[10px] text-white/40">输入：</span>
          {upstreamImage ? (
            <img
              src={upstreamImage}
              alt="上游图片"
              draggable={false}
              onDragStart={(event) => event.preventDefault()}
              className="pointer-events-none h-11 w-11 select-none rounded-md object-cover"
            />
          ) : (
            <div className="flex h-11 w-11 items-center justify-center rounded-md bg-white/[0.06] text-white/35">
              <Sparkles className="h-4 w-4" />
            </div>
          )}
          <span className="text-[10px] text-white/40">{upstreamImage ? "已连接图片输入" : "等待图片输入"}</span>
        </div>
      )}

      <div className="px-3.5 pb-2 pt-3">
        <textarea
          value={data.prompt ?? ""}
          placeholder={placeholder}
          rows={2}
          onChange={(event) => onUpdate({ prompt: event.target.value })}
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              void runWorkflow(nodeId);
            }
          }}
          onInput={(event) => {
            const target = event.currentTarget;
            target.style.height = "auto";
            target.style.height = `${Math.min(Math.max(target.scrollHeight, 64), 120)}px`;
          }}
          className="w-full resize-none bg-transparent pr-7 text-[13px] leading-relaxed text-white/85 outline-none placeholder:text-white/30"
          style={{ minHeight: 64, maxHeight: 120, scrollbarWidth: "none" }}
        />
      </div>

      <div className="flex items-center justify-between border-t border-white/[0.07] px-3 pb-3 pt-2">
        <select
          value={data.model ?? "deepseek-chat"}
          onChange={(event) => onUpdate({ model: event.target.value })}
          className="h-8 max-w-[190px] cursor-pointer rounded-lg bg-white/[0.08] px-2.5 text-xs text-white/75 outline-none"
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
        <button
          type="button"
          title="执行节点"
          disabled={!data.prompt?.trim()}
          onClick={() => void runWorkflow(nodeId)}
          className="flex h-8 w-8 items-center justify-center rounded-lg bg-white/[0.08] text-white/65 transition hover:bg-white/[0.14] hover:text-white disabled:cursor-not-allowed disabled:opacity-30"
        >
          <Play className="h-3.5 w-3.5" />
        </button>
      </div>

      {data.output && (
        <div className="border-t border-white/[0.07] px-3.5 py-3 text-xs leading-relaxed text-white/65">
          {data.output}
        </div>
      )}
    </div>
  );
}
