import { useEffect, useMemo, useRef } from "react";
import { Play, Sparkles } from "lucide-react";
import type { CreativeNodeData } from "../../types";
import { useCreativeStore } from "../../store/creativeStore";
import { runWorkflow } from "../../utils/workflowExecutor";

interface Props {
  id: string;
  data: CreativeNodeData;
}

function stopNodeGesture(event: React.SyntheticEvent) {
  event.stopPropagation();
}

export function TextGenContent({ id, data }: Props) {
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const isImageToPrompt = data.creativeActionKey === "image-to-prompt";
  const upstreamImage = useCreativeStore((state) => {
    if (!isImageToPrompt) return "";
    const edge = state.edges.find((item) => item.target === id);
    const source = edge ? state.nodes.find((node) => node.id === edge.source)?.data.imageUrl : undefined;
    return typeof source === "string" ? source : "";
  });
  const placeholder = useMemo(
    () => (isImageToPrompt ? "反推结果会显示在这里，也可以手动补充提示词..." : "输入内容，按 Enter 执行，Shift+Enter 换行"),
    [isImageToPrompt],
  );

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(Math.max(textarea.scrollHeight, 68), 132)}px`;
  }, [data.prompt]);

  const updatePrompt = (prompt: string) => {
    useCreativeStore.getState().updateNodeData(id, { prompt });
  };

  return (
    <div className="flex flex-col gap-2">
      {isImageToPrompt && (
        <div className="flex items-center gap-2 rounded-lg border border-white/10 bg-white/[0.025] px-2 py-1.5">
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
      <div className="rounded-lg border border-white/10 bg-white/[0.025] px-2.5 py-2 focus-within:border-white/20">
        <textarea
          ref={textareaRef}
          value={data.prompt ?? ""}
          placeholder={placeholder}
          rows={3}
          onPointerDown={stopNodeGesture}
          onMouseDown={stopNodeGesture}
          onChange={(event) => updatePrompt(event.target.value)}
          onInput={(event) => {
            const target = event.currentTarget;
            target.style.height = "auto";
            target.style.height = `${Math.min(Math.max(target.scrollHeight, 68), 132)}px`;
          }}
          onKeyDown={(event) => {
            event.stopPropagation();
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              void runWorkflow(id);
            }
          }}
          className="w-full resize-none bg-transparent text-xs leading-relaxed text-white/80 outline-none placeholder:text-white/25"
          style={{ minHeight: 68, maxHeight: 132, scrollbarWidth: "none" }}
        />
        <div className="mt-1 flex items-center justify-between border-t border-white/[0.06] pt-1.5">
          <select
            value={data.model ?? "deepseek-chat"}
            onPointerDown={stopNodeGesture}
            onMouseDown={stopNodeGesture}
            onChange={(event) => useCreativeStore.getState().updateNodeData(id, { model: event.target.value })}
            className="max-w-[150px] cursor-pointer truncate bg-transparent text-[9px] text-white/40 outline-none"
          >
            <option value="deepseek-chat">DeepSeek Chat</option>
            <option value="deepseek-reasoner">DeepSeek Reasoner</option>
          </select>
          <button
            type="button"
            title="执行节点"
            onPointerDown={stopNodeGesture}
            onMouseDown={stopNodeGesture}
            onClick={(event) => {
              stopNodeGesture(event);
              void runWorkflow(id);
            }}
            className="flex h-6 w-6 items-center justify-center rounded-md text-white/55 transition hover:bg-white/10 hover:text-white"
          >
            <Play className="h-3 w-3" />
          </button>
        </div>
      </div>
      {data.output && (
        <div className="rounded-lg border border-white/[0.08] bg-white/[0.02] px-2.5 py-2 text-[11px] leading-relaxed text-white/60">
          {data.output}
        </div>
      )}
    </div>
  );
}
