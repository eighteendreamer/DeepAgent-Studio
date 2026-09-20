import { useMemo, useRef, useState } from "react";
import { ArrowUp, ChevronDown, Maximize2, Sparkles, X } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../../../components/shadcn/dropdown-menu";
import {
  Dialog,
  DialogContent,
  DialogTitle,
} from "../../../components/shadcn/dialog";
import type { CreativeNodeData } from "../types";
import { useCreativeStore } from "../store/creativeStore";
import { runWorkflow } from "../utils/workflowExecutor";
import { useScenarioModelOptions } from "../store/canvasSettingsStore";
import { useCanvasMediaSrc } from "../utils/canvasMedia";
import { PromptRestoreButton } from "./PromptRestoreButton";

interface Props {
  nodeId: string;
  data: CreativeNodeData;
  onUpdate: (patch: Record<string, unknown>) => void;
}

function stopPanelGesture(event: React.SyntheticEvent) {
  event.stopPropagation();
}

export function TextGenEditorPanel({ nodeId, data, onUpdate }: Props) {
  const [expanded, setExpanded] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const isImageToPrompt = data.creativeActionKey === "image-to-prompt";
  const models = useScenarioModelOptions("text", data.model);
  const selectedModel = data.model ?? models[0]?.value ?? "";
  const selectedModelLabel =
    models.find((model) => model.value === selectedModel)?.label ?? "未配置模型";
  const upstreamImage = useCreativeStore((state) => {
    if (!isImageToPrompt) return "";
    const edge = state.edges.find((item) => item.target === nodeId);
    const source = edge ? state.nodes.find((node) => node.id === edge.source)?.data.imageUrl : undefined;
    return typeof source === "string" ? source : "";
  });
  const upstreamSrc = useCanvasMediaSrc(upstreamImage);
  const placeholder = useMemo(
    () => (isImageToPrompt ? "描述要从图片中反推出的提示词，也可以手动补充..." : "描述你想要生成的内容，并在下方调整生成参数..."),
    [isImageToPrompt],
  );

  const resizeTextarea = (target: HTMLTextAreaElement) => {
    target.style.height = "auto";
    target.style.height = `${Math.min(Math.max(target.scrollHeight, 76), 120)}px`;
  };

  const handleKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    event.stopPropagation();
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      void runWorkflow(nodeId);
    }
  };

  const handleChange = (value: string) => {
    onUpdate({ prompt: value });
  };

  return (
    <>
    <div
      className="w-full overflow-hidden rounded-[12px] border border-white/[0.08] shadow-[0_28px_80px_rgba(0,0,0,0.06),0_11.7px_33.4px_rgba(0,0,0,0.04),0_6.3px_17.9px_rgba(0,0,0,0.04),0_3.5px_10px_rgba(0,0,0,0.03)] backdrop-blur-[20px]"
      style={{
        background: "linear-gradient(rgba(96,104,108,0.55) 0%, rgba(52,58,60,0.55) 100%)",
        WebkitBackdropFilter: "blur(20px)",
      }}
      onPointerDown={stopPanelGesture}
      onMouseDown={stopPanelGesture}
    >
      {isImageToPrompt && (
        <div className="flex items-center gap-2 px-4 pb-0 pt-3">
          {upstreamSrc ? (
            <img
              src={upstreamSrc}
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

      <div className="relative px-4 pb-2 pt-3">
        <button
          type="button"
          title="放大编辑"
          onClick={() => setExpanded(true)}
          className="absolute right-3 top-3 z-10 flex h-7 w-7 items-center justify-center rounded-lg text-white/70 transition-colors hover:bg-white/[0.08] hover:text-white focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-white/20"
        >
          <Maximize2 className="h-3.5 w-3.5" />
        </button>
        <textarea
          ref={textareaRef}
          value={data.prompt ?? ""}
          placeholder={placeholder}
          rows={2}
          onChange={(event) => handleChange(event.target.value)}
          onKeyDown={handleKeyDown}
          onInput={(event) => resizeTextarea(event.currentTarget)}
          className="block min-h-[76px] w-full resize-none border-0 bg-transparent p-0 pr-8 text-[13px] leading-relaxed text-white/[0.88] outline-none placeholder:text-white/40 focus:border-0 focus:outline-none focus:ring-0"
          style={{ maxHeight: 120, scrollbarWidth: "none" }}
        />
        <PromptRestoreButton kind={data.kind} value={data.prompt ?? ""} onUpdate={onUpdate} />
      </div>

      <div className="flex items-center justify-between px-3 pb-3 pt-1">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              title="选择模型"
              className="flex h-8 max-w-[260px] items-center gap-2 rounded-lg bg-white/[0.08] px-3 text-xs text-white/75 outline-none transition hover:bg-white/[0.12] focus-visible:ring-1 focus-visible:ring-white/20 data-[state=open]:bg-white/[0.12]"
            >
              <span className="truncate">{selectedModelLabel}</span>
              <ChevronDown className="h-3.5 w-3.5 shrink-0 text-white/40" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="start"
            sideOffset={6}
            className="!min-w-[168px] !rounded-xl !border !border-white/10 !bg-[rgba(28,29,32,0.96)] !p-1.5 !text-xs !shadow-[0_18px_48px_rgba(0,0,0,0.34)] backdrop-blur-2xl"
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
          className="relative flex h-7 w-7 items-center justify-center overflow-hidden rounded-[10px] bg-white/[0.12] text-white/75 shadow-[0_1px_2px_rgba(0,0,0,0.08),0_4px_10px_rgba(0,0,0,0.06)] transition hover:bg-white/[0.18] hover:text-white disabled:cursor-not-allowed disabled:opacity-40"
        >
          <span
            aria-hidden="true"
            className="pointer-events-none absolute inset-0 rounded-[10px] opacity-40"
            style={{
              padding: "0.6px",
              background: "linear-gradient(135deg,rgba(255,255,255,0.95) 0%,rgba(255,255,255,0.18) 30%,rgba(255,255,255,0.18) 70%,rgba(255,255,255,0.95) 100%)",
              WebkitMask: "linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0)",
              WebkitMaskComposite: "xor",
              maskComposite: "exclude",
            }}
          />
          <ArrowUp className="h-4 w-4" />
        </button>
      </div>

      {data.output && (
        <div className="mx-4 mb-3 border-t border-white/[0.08] px-0 pt-3 text-xs leading-relaxed text-white/65">
          {data.output}
        </div>
      )}
    </div>
    <Dialog open={expanded} onOpenChange={setExpanded}>
      <DialogContent className="w-[min(720px,calc(100vw-32px))] max-w-none rounded-2xl border border-white/10 bg-[#1c1d20]/95 p-0 text-white shadow-[0_28px_80px_rgba(0,0,0,0.45)] backdrop-blur-2xl">
        <div className="flex items-center justify-between border-b border-white/[0.08] px-5 py-4">
          <DialogTitle className="text-sm font-medium text-white/85">编辑提示词</DialogTitle>
          <button
            type="button"
            title="关闭"
            onClick={() => setExpanded(false)}
            className="flex h-7 w-7 items-center justify-center rounded-lg text-white/50 transition hover:bg-white/[0.08] hover:text-white"
          >
            <X className="h-4 w-4" />
          </button>
        </div>
        <div className="px-5 py-4">
          <textarea
            autoFocus
            value={data.prompt ?? ""}
            onChange={(event) => handleChange(event.target.value)}
            onKeyDown={handleKeyDown}
            className="min-h-[220px] w-full resize-y border-0 bg-transparent text-sm leading-6 text-white/90 outline-none placeholder:text-white/40 focus:ring-0"
            placeholder={placeholder}
          />
        </div>
        <div className="flex justify-end border-t border-white/[0.08] px-5 py-3">
          <button
            type="button"
            onClick={() => { setExpanded(false); void runWorkflow(nodeId); }}
            disabled={!data.prompt?.trim()}
            className="flex h-8 items-center gap-2 rounded-lg bg-white/[0.1] px-3 text-xs text-white/80 transition hover:bg-white/[0.16] disabled:cursor-not-allowed disabled:opacity-40"
          >
            <ArrowUp className="h-3.5 w-3.5" />
            执行节点
          </button>
        </div>
      </DialogContent>
    </Dialog>
    </>
  );
}
