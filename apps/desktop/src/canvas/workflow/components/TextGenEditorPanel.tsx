import { useMemo } from "react";
import { ArrowUp, ChevronDown, Sparkles } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../../../components/shadcn/dropdown-menu";
import { Textarea } from "../../../components/shadcn/textarea";
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
  const models = [
    { value: "deepseek-chat", label: "DeepSeek Chat" },
    { value: "deepseek-reasoner", label: "DeepSeek Reasoner" },
  ];
  const selectedModel = data.model ?? "deepseek-chat";
  const selectedModelLabel = models.find((model) => model.value === selectedModel)?.label ?? "DeepSeek Chat";
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
      className="w-full overflow-hidden rounded-2xl border border-white/[0.12] bg-[#1c1d20]/95 p-1.5 shadow-[0_18px_48px_rgba(0,0,0,0.34)] backdrop-blur-2xl"
      onPointerDown={stopPanelGesture}
      onMouseDown={stopPanelGesture}
    >
      {isImageToPrompt && (
        <div className="flex items-center gap-2 px-2.5 pb-1 pt-2">
          {upstreamImage ? (
            <img
              src={upstreamImage}
              alt="上游图片"
              draggable={false}
              onDragStart={(event) => event.preventDefault()}
              className="pointer-events-none h-10 w-10 select-none rounded-lg object-cover ring-1 ring-white/10"
            />
          ) : (
            <div className="flex h-10 w-10 items-center justify-center rounded-lg bg-white/[0.05] text-white/35">
              <Sparkles className="h-4 w-4" />
            </div>
          )}
          <span className="text-[10px] text-white/40">{upstreamImage ? "已连接图片输入" : "等待图片输入"}</span>
        </div>
      )}

      <div className="px-2.5 pb-1.5 pt-2">
        <Textarea
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
            target.style.height = `${Math.min(Math.max(target.scrollHeight, 76), 140)}px`;
          }}
          className="!w-full !resize-none !rounded-none !border-0 !bg-transparent !px-0 !py-0 !text-[13px] !leading-relaxed !text-white/85 !shadow-none !outline-none !ring-0 focus:!border-0 focus:!ring-0 placeholder:!text-white/40"
          style={{ minHeight: 76, maxHeight: 140, scrollbarWidth: "none" }}
        />
      </div>

      <div className="flex items-center justify-between px-1.5 pb-1.5 pt-1">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="flex h-8 max-w-[190px] items-center gap-2 rounded-xl bg-white/[0.06] px-3 text-xs text-white/70 outline-none transition hover:bg-white/[0.1] focus-visible:ring-1 focus-visible:ring-white/20 data-[state=open]:bg-white/[0.1]"
            >
              <span className="truncate">{selectedModelLabel}</span>
              <ChevronDown className="h-3.5 w-3.5 shrink-0 text-white/40" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="start"
            sideOffset={6}
            className="!min-w-[168px] !rounded-xl !border !border-white/10 !bg-[rgba(28,29,32,0.96)] !p-1.5 !text-xs !shadow-[0_12px_32px_rgba(0,0,0,0.42)] backdrop-blur-2xl"
          >
            {models.map((model) => (
              <DropdownMenuItem
                key={model.value}
                onSelect={() => onUpdate({ model: model.value })}
                className="!rounded-lg !px-2.5 !py-2 !text-xs data-[highlighted]:!bg-white/10"
                style={{
                  color: model.value === selectedModel ? "rgba(248,248,248,0.95)" : "rgba(248,248,248,0.65)",
                }}
              >
                {model.label}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <button
          type="button"
          title="执行节点"
          disabled={!data.prompt?.trim()}
          onClick={() => void runWorkflow(nodeId)}
          className="flex h-8 w-8 items-center justify-center rounded-full bg-white/[0.08] text-white/65 transition hover:bg-white/[0.14] hover:text-white disabled:cursor-not-allowed disabled:opacity-30"
        >
          <ArrowUp className="h-4 w-4" />
        </button>
      </div>

      {data.output && (
        <div className="mx-1.5 mb-1.5 rounded-xl bg-black/[0.12] px-2.5 py-2.5 text-xs leading-relaxed text-white/65">
          {data.output}
        </div>
      )}
    </div>
  );
}
