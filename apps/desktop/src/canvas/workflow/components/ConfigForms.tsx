import React, { useRef, useState } from "react";
import { ArrowUp, ChevronDown, Maximize2, Plus, X, Square } from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "../../../components/shadcn/dialog";
import { CanvasInput } from "./CanvasFields";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "../../../components/shadcn/dropdown-menu";
import type { CreativeNodeData, WorkflowNodeData } from "../types";
import { useCanvasSettingsStore } from "../store/canvasSettingsStore";
import { runWorkflow } from "../utils/workflowExecutor";

const TEXT_COLOR = "rgba(255,255,255,0.88)";

function autoGrow(e: React.FormEvent<HTMLTextAreaElement>, max = 100) {
  const t = e.currentTarget;
  t.style.height = "auto";
  t.style.height = `${Math.min(t.scrollHeight, max)}px`;
}

function PromptArea({
  value,
  placeholder,
  onChange,
  rows = 2,
}: {
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
  rows?: number;
}) {
  return (
    <textarea
      value={value}
      placeholder={placeholder}
      rows={rows}
      onChange={(e) => onChange(e.target.value)}
      onInput={(e) => autoGrow(e)}
      className="w-full resize-none bg-transparent outline-none"
      style={{
        color: TEXT_COLOR,
        fontSize: 13,
        lineHeight: 1.6,
        maxHeight: 100,
        scrollbarWidth: "none",
      }}
    />
  );
}

function ChipSelect({
  value,
  options,
  onChange,
  icon,
  itemIcons,
  footer,
  keepOpenOnSelectValues,
}: {
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (v: string) => void;
  icon?: React.ReactNode;
  itemIcons?: Record<string, React.ReactNode>;
  footer?: React.ReactNode;
  keepOpenOnSelectValues?: string[];
}) {
  // 取选中项的 label 作为 trigger 文案；找不到时回退原 value
  const currentLabel = options.find((o) => o.value === value)?.label ?? value;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="relative flex h-8 items-center gap-1 rounded-lg text-[12px] font-medium transition-colors hover:bg-white/12 focus:outline-none data-[state=open]:bg-white/12"
          style={{
            paddingLeft: icon ? 24 : 10,
            paddingRight: 8,
            background: "rgba(255,255,255,0.08)",
            color: "rgba(255,255,255,0.75)",
          }}
        >
          {icon && (
            <span
              className="pointer-events-none absolute left-2 top-1/2 flex -translate-y-1/2 items-center"
              style={{ color: "rgba(255,255,255,0.7)" }}
            >
              {icon}
            </span>
          )}
          <span className="whitespace-nowrap">{currentLabel}</span>
          <ChevronDown className="h-3 w-3 opacity-60" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="start"
        sideOffset={6}
        className="!min-w-0 !rounded-xl !p-1 !text-[12px] !shadow-[0_6px_24px_rgba(0,0,0,0.4)] !border !border-white/8 !bg-[rgba(24,24,27,0.92)] backdrop-blur-[40px]"
      >
        {options.map((o) => {
          const active = o.value === value;
          const itemIcon = itemIcons?.[o.value];
          return (
            <DropdownMenuItem
              key={o.value}
              onSelect={(event) => {
                onChange(o.value);
                if (keepOpenOnSelectValues?.includes(o.value)) {
                  event.preventDefault();
                }
              }}
              className="!rounded-lg !px-2.5 !py-1.5 !text-[12px] data-[highlighted]:!bg-white/10 flex items-center gap-2"
              style={{ color: active ? "#a78bfa" : "rgba(255,255,255,0.88)" }}
            >
              {itemIcon && (
                <span className="flex h-3.5 w-3.5 flex-shrink-0 items-center justify-center" style={{ color: active ? "#a78bfa" : "rgba(255,255,255,0.7)" }}>
                  {itemIcon}
                </span>
              )}
              <span>{o.label}</span>
            </DropdownMenuItem>
          );
        })}
        {footer}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function buildModelOptions(configuredModel: string, selectedModel: string) {
  return Array.from(
    new Map(
      [
        configuredModel ? { value: configuredModel, label: configuredModel } : null,
        selectedModel && selectedModel !== configuredModel
          ? { value: selectedModel, label: selectedModel }
          : null,
      ]
        .filter((item): item is { value: string; label: string } => item !== null)
        .map((item) => [item.value, item]),
    ).values(),
  );
}

function stopPanelGesture(event: React.SyntheticEvent) {
  event.stopPropagation();
}

function GlassRunButton({
  disabled,
  onClick,
  title = "执行节点",
}: {
  disabled?: boolean;
  onClick: () => void;
  title?: string;
}) {
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={onClick}
      className="relative flex h-7 w-7 items-center justify-center overflow-hidden rounded-[10px] bg-white/[0.12] text-white/75 shadow-[0_1px_2px_rgba(0,0,0,0.08),0_4px_10px_rgba(0,0,0,0.06)] transition hover:bg-white/[0.18] hover:text-white disabled:cursor-not-allowed disabled:opacity-40"
    >
      <span
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 rounded-[10px] opacity-40"
        style={{
          padding: "0.6px",
          background:
            "linear-gradient(135deg,rgba(255,255,255,0.95) 0%,rgba(255,255,255,0.18) 30%,rgba(255,255,255,0.18) 70%,rgba(255,255,255,0.95) 100%)",
          WebkitMask: "linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0)",
          WebkitMaskComposite: "xor",
          maskComposite: "exclude",
        }}
      />
      <ArrowUp className="h-4 w-4" />
    </button>
  );
}

function SegmentedChips<T extends string>({
  value,
  options,
  onChange,
  activeColor = "rgba(139,92,246,1)",
}: {
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (v: T) => void;
  activeColor?: string;
}) {
  return (
    <div className="flex h-8 items-center gap-0.5 rounded-lg px-1" style={{ background: "rgba(255,255,255,0.08)" }}>
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            onClick={() => onChange(o.value)}
            className="h-6 rounded-md px-2.5 text-[12px] font-medium transition-all duration-200"
            style={{
              background: active ? "rgba(255,255,255,0.16)" : "transparent",
              color: active ? activeColor : "rgba(255,255,255,0.55)",
            }}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

function OutputBlock({ text }: { text: string }) {
  return (
    <div
      className="rounded-lg px-3 py-2 text-xs leading-relaxed"
      style={{ background: "rgba(255,255,255,0.03)", color: "rgba(248,248,248,0.6)", maxHeight: 140, overflowY: "auto" }}
    >
      {text}
    </div>
  );
}

interface FormProps {
  nodeId: string;
  onUpdate: (patch: Record<string, unknown>) => void;
}

export function TextGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-center gap-1.5">
        <ChipSelect
          value={data.model ?? "deepseek-chat"}
          options={[
            { value: "deepseek-chat", label: "DeepSeek Chat" },
            { value: "deepseek-reasoner", label: "DeepSeek Reasoner" },
          ]}
          onChange={(model) => onUpdate({ model })}
        />
      </div>
      <PromptArea
        value={data.prompt ?? ""}
        placeholder="描述你想要生成的内容..."
        onChange={(prompt) => onUpdate({ prompt })}
      />
      {data.output && <OutputBlock text={data.output} />}
    </div>
  );
}

const ASPECT_RATIOS = [
  { value: "Auto", label: "Auto" },
  { value: "21:9", label: "21:9" },
  { value: "16:9", label: "16:9" },
  { value: "3:2", label: "3:2" },
  { value: "4:3", label: "4:3" },
  { value: "5:4", label: "5:4" },
  { value: "1:1", label: "1:1" },
  { value: "4:5", label: "4:5" },
  { value: "3:4", label: "3:4" },
  { value: "2:3", label: "2:3" },
  { value: "9:16", label: "9:16" },
  { value: "custom", label: "自定义" },
] as const;

const RESOLUTIONS = [
  { value: "1K", label: "1K" },
  { value: "2K", label: "2K" },
  { value: "4K", label: "4K" },
] as const;

const COUNTS = [1, 2, 3, 4] as const;

function RatioIcon({ ratio, size = 14 }: { ratio?: string; size?: number }) {
  if (!ratio || ratio === "Auto") {
    return <Square className="opacity-50" style={{ width: size, height: size }} strokeWidth={1.5} strokeDasharray="2 1.5" />;
  }
  if (ratio === "custom") {
    return (
      <div
        className="relative"
        style={{
          width: size,
          height: size,
          borderRadius: 2,
          border: "1.5px dashed rgba(255,255,255,0.7)",
          background: "rgba(255,255,255,0.10)",
        }}
      >
        <span
          className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 text-[7px] font-semibold leading-none"
          style={{ color: "rgba(255,255,255,0.85)" }}
        >
          W×H
        </span>
      </div>
    );
  }
  const parts = ratio.split(":").map(Number);
  if (parts.length !== 2 || parts[0] <= 0 || parts[1] <= 0) {
    return <Square className="opacity-50" style={{ width: size, height: size }} strokeWidth={1.5} />;
  }
  const aspect = parts[0] / parts[1];
  let w = size, h = size;
  if (aspect >= 1) { w = size; h = Math.max(5, Math.round(size / aspect)); }
  else { h = size; w = Math.max(5, Math.round(size * aspect)); }
  return (
    <div
      style={{
        width: w,
        height: h,
        borderRadius: 2,
        border: "1.5px solid rgba(255,255,255,0.85)",
        background: "rgba(255,255,255,0.18)",
      }}
    />
  );
}

const SUB_COLOR = "rgba(255,255,255,0.5)";
export function ImageGenForm({ nodeId, data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const [expanded, setExpanded] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const expandedTextareaRef = useRef<HTMLTextAreaElement>(null);
  const configuredModel = useCanvasSettingsStore((state) => state.scenarioModels.image.model.trim());

  const model = data.imageModel?.trim() || configuredModel;
  const modelOptions = buildModelOptions(configuredModel, model);
  const prompt = data.imagePrompt ?? "";
  const ratio = data.aspectRatio ?? "1:1";
  const resolution = data.resolution ?? "1K";
  const count = data.batchCount ?? 1;
  const inputUrls = data.imageInputUrls ?? [];
  const hasStoryboardTag = !!data._storyboardLabel;
  const hasCreativeTag = !hasStoryboardTag && !!data._creativeLabel;
  const MAX_INPUT_IMAGES = 2;
  const atMaxInput = inputUrls.length >= MAX_INPUT_IMAGES;

  const handleFile = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    if (atMaxInput) {
      e.target.value = "";
      return;
    }
    const reader = new FileReader();
    reader.onload = (ev) => {
      if (typeof ev.target?.result === "string") {
        onUpdate({ imageInputUrls: [...inputUrls, ev.target.result] });
      }
    };
    reader.readAsDataURL(file);
    e.target.value = "";
  };

  const removeInput = (idx: number) => {
    onUpdate({ imageInputUrls: inputUrls.filter((_, i) => i !== idx) });
  };

  const setModel = (next: string) => {
    onUpdate({ imageModel: next });
  };

  const setRatio = (next: string) => {
    const updates: Record<string, unknown> = { aspectRatio: next };
    if (next === "custom" && !data.customSize) {
      updates.customSize = "1024x1024";
    }
    onUpdate(updates);
  };

  // 每个比例的形状图标映射（下拉项里也要展示）
  const ratioItemIcons: Record<string, React.ReactNode> = React.useMemo(() => {
    const m: Record<string, React.ReactNode> = {};
    for (const r of ASPECT_RATIOS) {
      m[r.value] = <RatioIcon ratio={r.value} size={12} />;
    }
    return m;
  }, []);

  // 自定义宽高：本地表单态 + 持久化字符串 "WxH"
  const parseCustomSize = (s: string | undefined): { w: number; h: number } | null => {
    if (!s) return null;
    const m = s.match(/^(\d+)\s*[xX×]\s*(\d+)$/);
    if (!m) return null;
    const w = Number(m[1]);
    const h = Number(m[2]);
    if (!w || !h) return null;
    return { w, h };
  };
  const initCustom = parseCustomSize(data.customSize) ?? { w: 1024, h: 1024 };
  const [customW, setCustomW] = useState<number>(initCustom.w);
  const [customH, setCustomH] = useState<number>(initCustom.h);

  // customSize 由外部变化时（粘贴分镜、导入资产等）同步本地态
  React.useEffect(() => {
    const parsed = parseCustomSize(data.customSize);
    if (parsed && (parsed.w !== customW || parsed.h !== customH)) {
      setCustomW(parsed.w);
      setCustomH(parsed.h);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [data.customSize]);

  const commitCustomSize = (w: number, h: number) => {
    if (!w || !h) return;
    onUpdate({ customSize: `${w}x${h}` });
  };

  const isCustomRatio = ratio === "custom";
  const renderCustomSizeControls = () => (
    <>
      <DropdownMenuSeparator className="!my-1 !bg-white/[0.08]" />
      <div
        className="px-2.5 pb-1.5 pt-1"
        onClick={(event) => event.stopPropagation()}
        onPointerDown={(event) => event.stopPropagation()}
        onKeyDown={(event) => event.stopPropagation()}
      >
        <DropdownMenuLabel className="!px-0 !pb-1.5 !pt-0 !text-[10px] !font-medium !text-white/40">
          自定义尺寸
        </DropdownMenuLabel>
        <div className="flex items-center gap-1.5">
          <input
            aria-label="自定义宽度"
            type="number"
            min={64}
            max={4096}
            step={64}
            value={customW}
            onChange={(e) => {
              const v = Math.max(64, Math.min(4096, Number(e.target.value) || 64));
              setCustomW(v);
              commitCustomSize(v, customH);
            }}
            className="h-8 w-[76px] rounded-lg border border-white/[0.08] bg-white/[0.08] px-2 text-[12px] font-medium text-white/80 outline-none transition focus:border-white/20 focus:bg-white/[0.11] focus:ring-0"
          />
          <span className="text-[11px] text-white/35">×</span>
          <input
            aria-label="自定义高度"
            type="number"
            min={64}
            max={4096}
            step={64}
            value={customH}
            onChange={(e) => {
              const v = Math.max(64, Math.min(4096, Number(e.target.value) || 64));
              setCustomH(v);
              commitCustomSize(customW, v);
            }}
            className="h-8 w-[76px] rounded-lg border border-white/[0.08] bg-white/[0.08] px-2 text-[12px] font-medium text-white/80 outline-none transition focus:border-white/20 focus:bg-white/[0.11] focus:ring-0"
          />
        </div>
      </div>
    </>
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

  return (
    <>
      <div
        className="relative w-full overflow-hidden rounded-[12px] border border-white/[0.08] shadow-[0_28px_80px_rgba(0,0,0,0.06),0_11.7px_33.4px_rgba(0,0,0,0.04),0_6.3px_17.9px_rgba(0,0,0,0.04),0_3.5px_10px_rgba(0,0,0,0.03)] backdrop-blur-[20px]"
        style={{
          background: "linear-gradient(rgba(96,104,108,0.55) 0%, rgba(52,58,60,0.55) 100%)",
          WebkitBackdropFilter: "blur(20px)",
        }}
        onPointerDown={stopPanelGesture}
        onMouseDown={stopPanelGesture}
      >
        <button
          type="button"
          onClick={() => setExpanded(true)}
          className="absolute right-3 top-3 z-10 flex h-7 w-7 items-center justify-center rounded-lg text-white/70 transition-colors hover:bg-white/[0.08] hover:text-white focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-white/20"
          title="放大编辑"
        >
          <Maximize2 className="h-3.5 w-3.5" />
        </button>

        <div className="flex flex-wrap items-center gap-1.5 px-4 pb-0 pr-12 pt-3">
          <span className="shrink-0 text-[11px]" style={{ color: SUB_COLOR }}>
            输入图{inputUrls.length}/{MAX_INPUT_IMAGES}
          </span>
          {inputUrls.map((url, idx) => (
            <div
              key={`${idx}-${url.slice(0, 24)}`}
              className="group relative h-10 w-10 shrink-0 overflow-hidden rounded-lg transition-transform hover:scale-105"
              style={{ border: "1px solid rgba(255,255,255,0.15)" }}
            >
              <img src={url} alt={`输入${idx + 1}`} className="h-full w-full object-cover" draggable={false} />
              <button
                type="button"
                onClick={() => removeInput(idx)}
                className="absolute right-0 top-0 flex h-4 w-4 items-center justify-center rounded-bl-md opacity-0 transition-opacity group-hover:opacity-100"
                style={{ background: "rgba(0,0,0,0.6)" }}
                title="移除输入图"
              >
                <X className="h-2.5 w-2.5 text-white" />
              </button>
            </div>
          ))}
          <button
            type="button"
            onClick={() => {
              if (atMaxInput) return;
              fileInputRef.current?.click();
            }}
            disabled={atMaxInput}
            className="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg transition-transform hover:scale-105 disabled:cursor-not-allowed disabled:hover:scale-100"
            style={{
              border: atMaxInput ? "1.5px dashed rgba(255,255,255,0.10)" : "1.5px dashed rgba(255,255,255,0.22)",
              background: atMaxInput ? "rgba(255,255,255,0.02)" : "rgba(255,255,255,0.04)",
              opacity: atMaxInput ? 0.4 : 1,
            }}
            title={atMaxInput ? `已达上限 ${MAX_INPUT_IMAGES} 张` : "添加图片"}
          >
            <Plus className="h-3.5 w-3.5" style={{ color: "rgba(255,255,255,0.6)" }} />
          </button>
          <input ref={fileInputRef} type="file" accept="image/*" className="hidden" onChange={handleFile} />
        </div>

        {hasStoryboardTag && (
          <div className="flex items-center gap-1.5 px-4 pt-2">
            <span
              className="inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px] font-medium"
              style={{ background: "rgba(59,130,246,0.15)", color: "#3b82f6" }}
            >
              <span style={{ fontSize: 12 }}>🎬</span>
              分镜·{data._storyboardLabel}
              <button
                type="button"
                className="flex h-3.5 w-3.5 items-center justify-center rounded-full opacity-70 transition-opacity hover:opacity-100"
                style={{ background: "rgba(59,130,246,0.25)" }}
                title="移除分镜模板"
                onClick={() =>
                  onUpdate({
                    _storyboardLabel: undefined,
                    _storyboardKey: undefined,
                    imagePrompt: "",
                    status: "idle",
                  })
                }
              >
                <X className="h-2 w-2" />
              </button>
            </span>
          </div>
        )}

        {hasCreativeTag && (
          <div className="flex items-center gap-1.5 px-4 pt-2">
            <span
              className="inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 text-[11px] font-medium"
              style={{ background: "rgba(139,92,246,0.18)", color: "#8b5cf6" }}
            >
              <span style={{ fontSize: 12 }}>📚</span>
              模板·{data._creativeLabel}
              <button
                type="button"
                className="flex h-3.5 w-3.5 items-center justify-center rounded-full opacity-70 transition-opacity hover:opacity-100"
                style={{ background: "rgba(139,92,246,0.28)" }}
                title="移除模板"
                onClick={() =>
                  onUpdate({
                    _creativeLabel: undefined,
                    imagePrompt: "",
                    status: "idle",
                  })
                }
              >
                <X className="h-2 w-2" />
              </button>
            </span>
          </div>
        )}

        <div className="relative px-4 pb-2 pt-3">
          <textarea
            value={prompt}
            placeholder={hasStoryboardTag ? "分镜指令已就绪，点击发送开始生成…" : "描述你想要生成的内容，并在下方调整生成参数..."}
            rows={2}
            onChange={(event) => onUpdate({ imagePrompt: event.target.value })}
            onKeyDown={handleKeyDown}
            onInput={(event) => resizeTextarea(event.currentTarget)}
            className="block min-h-[76px] w-full resize-none border-0 bg-transparent p-0 pr-8 text-[13px] leading-relaxed text-white/[0.88] outline-none placeholder:text-white/40 focus:border-0 focus:outline-none focus:ring-0"
            style={{ maxHeight: 120, scrollbarWidth: "none" }}
          />
        </div>

        <div className="flex items-center justify-between px-3 pb-3 pt-1">
          <div className="flex min-w-0 flex-wrap items-center gap-1.5">
            <ChipSelect
              value={model}
              options={modelOptions.length > 0 ? modelOptions : [{ value: "", label: "选择模型" }]}
              onChange={setModel}
            />
            <ChipSelect
              value={ratio}
              options={ASPECT_RATIOS.map((r) => ({ value: r.value, label: r.label }))}
              onChange={setRatio}
              icon={<RatioIcon ratio={ratio} size={12} />}
              itemIcons={ratioItemIcons}
              keepOpenOnSelectValues={["custom"]}
              footer={isCustomRatio ? renderCustomSizeControls() : null}
            />
            <ChipSelect
              value={resolution}
              options={RESOLUTIONS.map((r) => ({ value: r.value, label: r.label }))}
              onChange={(resolution) => onUpdate({ resolution })}
            />
            <ChipSelect
              value={String(count)}
              options={COUNTS.map((n) => ({ value: String(n), label: `${n}x` }))}
              onChange={(v) => onUpdate({ batchCount: Number(v) })}
            />
          </div>
          <GlassRunButton disabled={!prompt.trim()} onClick={() => void runWorkflow(nodeId)} />
        </div>
      </div>

      <Dialog open={expanded} onOpenChange={setExpanded}>
        <DialogContent className="w-[min(720px,calc(100vw-32px))] max-w-none rounded-2xl border border-white/10 bg-[#1c1d20]/95 p-0 text-white shadow-[0_28px_80px_rgba(0,0,0,0.45)] backdrop-blur-2xl">
          <div className="flex items-center justify-between border-b border-white/[0.08] px-5 py-4">
            <DialogTitle className="text-sm font-medium text-white/85">编辑提示词</DialogTitle>
            <button
              type="button"
              onClick={() => setExpanded(false)}
              className="flex h-7 w-7 items-center justify-center rounded-lg text-white/50 transition hover:bg-white/[0.08] hover:text-white"
              title="关闭"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <div className="px-5 py-4">
            <textarea
              ref={expandedTextareaRef}
              autoFocus
              value={prompt}
              placeholder="描述你想要生成的内容，并在下方调整生成参数..."
              onChange={(e) => onUpdate({ imagePrompt: e.target.value })}
              onKeyDown={handleKeyDown}
              className="min-h-[220px] w-full resize-y border-0 bg-transparent text-sm leading-6 text-white/90 outline-none placeholder:text-white/40 focus:ring-0"
            />
          </div>
          <div className="flex items-center justify-between border-t border-white/[0.08] px-5 py-3">
            <div className="flex items-center gap-1.5">
              <ChipSelect
                value={model}
                options={modelOptions.length > 0 ? modelOptions : [{ value: "", label: "选择模型" }]}
                onChange={setModel}
              />
              <ChipSelect
                value={ratio}
                options={ASPECT_RATIOS.map((r) => ({ value: r.value, label: r.label }))}
                onChange={setRatio}
                icon={<RatioIcon ratio={ratio} size={12} />}
                itemIcons={ratioItemIcons}
                keepOpenOnSelectValues={["custom"]}
                footer={isCustomRatio ? renderCustomSizeControls() : null}
              />
              <ChipSelect
                value={resolution}
                options={RESOLUTIONS.map((r) => ({ value: r.value, label: r.label }))}
                onChange={(resolution) => onUpdate({ resolution })}
              />
              <ChipSelect
                value={String(count)}
                options={COUNTS.map((n) => ({ value: String(n), label: `${n}x` }))}
                onChange={(v) => onUpdate({ batchCount: Number(v) })}
              />
            </div>
            <button
              type="button"
              onClick={() => { setExpanded(false); void runWorkflow(nodeId); }}
              disabled={!prompt.trim()}
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

export function VideoGenForm({ nodeId, data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const [expanded, setExpanded] = useState(false);
  const configuredModel = useCanvasSettingsStore((state) => state.scenarioModels.video.model.trim());
  const selectedModel = data.videoModel?.trim() || configuredModel;
  const modelOptions = buildModelOptions(configuredModel, selectedModel);
  const prompt = data.videoPrompt ?? "";
  const duration = data.videoDuration ?? 5;

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
            value={prompt}
            placeholder="描述你想要生成的内容，并在下方调整生成参数..."
            rows={2}
            onChange={(event) => onUpdate({ videoPrompt: event.target.value })}
            onKeyDown={handleKeyDown}
            onInput={(event) => resizeTextarea(event.currentTarget)}
            className="block min-h-[76px] w-full resize-none border-0 bg-transparent p-0 pr-8 text-[13px] leading-relaxed text-white/[0.88] outline-none placeholder:text-white/40 focus:border-0 focus:outline-none focus:ring-0"
            style={{ maxHeight: 120, scrollbarWidth: "none" }}
          />
        </div>

        <div className="flex items-center justify-between px-3 pb-3 pt-1">
          <div className="flex min-w-0 items-center gap-1.5">
            <ChipSelect
              value={selectedModel}
              options={modelOptions.length > 0 ? modelOptions : [{ value: "", label: "选择模型" }]}
              onChange={(videoModel) => onUpdate({ videoModel })}
            />
            <label
              className="flex h-8 items-center gap-1 rounded-lg bg-white/[0.08] px-2.5 text-xs text-white/70"
              title="视频时长"
            >
              <input
                type="number"
                min={1}
                max={60}
                value={duration}
                onChange={(event) => onUpdate({ videoDuration: Number(event.target.value) })}
                onKeyDown={(event) => event.stopPropagation()}
                className="w-8 border-0 bg-transparent p-0 text-xs font-medium text-white/80 outline-none [appearance:textfield] focus:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
              />
              <span className="text-white/40">秒</span>
            </label>
          </div>
          <GlassRunButton disabled={!prompt.trim()} onClick={() => void runWorkflow(nodeId)} />
        </div>
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
              value={prompt}
              onChange={(event) => onUpdate({ videoPrompt: event.target.value })}
              onKeyDown={handleKeyDown}
              className="min-h-[220px] w-full resize-y border-0 bg-transparent text-sm leading-6 text-white/90 outline-none placeholder:text-white/40 focus:ring-0"
              placeholder="描述你想要生成的内容，并在下方调整生成参数..."
            />
          </div>
          <div className="flex items-center justify-between border-t border-white/[0.08] px-5 py-3">
            <div className="flex items-center gap-1.5">
              <ChipSelect
                value={selectedModel}
                options={modelOptions.length > 0 ? modelOptions : [{ value: "", label: "选择模型" }]}
                onChange={(videoModel) => onUpdate({ videoModel })}
              />
              <label className="flex h-8 items-center gap-1 rounded-lg bg-white/[0.08] px-2.5 text-xs text-white/70">
                <input
                  type="number"
                  min={1}
                  max={60}
                  value={duration}
                  onChange={(event) => onUpdate({ videoDuration: Number(event.target.value) })}
                  onKeyDown={(event) => event.stopPropagation()}
                  className="w-8 border-0 bg-transparent p-0 text-xs font-medium text-white/80 outline-none [appearance:textfield] focus:ring-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
                />
                <span className="text-white/40">秒</span>
              </label>
            </div>
            <button
              type="button"
              onClick={() => { setExpanded(false); void runWorkflow(nodeId); }}
              disabled={!prompt.trim()}
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

export function ScriptGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex items-center gap-1.5">
        <ChipSelect
          value={data.model ?? "deepseek-chat"}
          options={[
            { value: "deepseek-chat", label: "DeepSeek Chat" },
            { value: "deepseek-reasoner", label: "DeepSeek Reasoner" },
          ]}
          onChange={(model) => onUpdate({ model })}
        />
      </div>
      <PromptArea
        value={data.prompt ?? ""}
        placeholder="描述视频主题和要求..."
        onChange={(prompt) => onUpdate({ prompt })}
      />
    </div>
  );
}

export function ImageEditForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <SegmentedChips
        value={data.editMode ?? "crop"}
        options={[
          { value: "crop", label: "裁剪" },
          { value: "remove-bg", label: "去背景" },
          { value: "upscale", label: "超分辨率" },
          { value: "repaint", label: "局部重绘" },
        ]}
        onChange={(editMode) => onUpdate({ editMode })}
      />
    </div>
  );
}

export function ImageCompareForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2">
      <CanvasInput
        placeholder="图片 A 地址..."
        value={data.leftImageUrl ?? ""}
        onChange={(e) => onUpdate({ leftImageUrl: e.target.value })}
      />
      <CanvasInput
        placeholder="图片 B 地址..."
        value={data.rightImageUrl ?? ""}
        onChange={(e) => onUpdate({ rightImageUrl: e.target.value })}
      />
    </div>
  );
}

export function VideoStitchForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const urls = data.inputVideoUrls ?? [];
  return (
    <div className="flex flex-col gap-1.5">
      {urls.map((url, i) => (
        <div key={i} className="flex items-center gap-1.5">
          <span className="shrink-0 text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
            #{i + 1}
          </span>
          <CanvasInput
            style={{ flex: 1 }}
            value={url}
            onChange={(e) => {
              const next = [...urls];
              next[i] = e.target.value;
              onUpdate({ inputVideoUrls: next });
            }}
          />
          <button
            className="shrink-0 rounded px-1.5 py-0.5 text-[10px] transition-colors hover:bg-white/10"
            style={{ color: "rgba(239,68,68,0.7)" }}
            onClick={() => {
              const next = urls.filter((_, j) => j !== i);
              onUpdate({ inputVideoUrls: next });
            }}
          >
            ✕
          </button>
        </div>
      ))}
      <button
        className="self-start rounded-lg px-2.5 py-1 text-[11px] transition-all duration-200 hover:bg-white/10"
        style={{ color: "rgba(59,130,246,0.8)", border: "1px dashed rgba(59,130,246,0.3)" }}
        onClick={() => onUpdate({ inputVideoUrls: [...urls, ""] })}
      >
        + 添加片段
      </button>
    </div>
  );
}

export function GenericConfigForm({ data }: { data: WorkflowNodeData }) {
  return (
    <div
      className="flex flex-col items-center justify-center rounded-xl py-8"
      style={{
        background: "rgba(255,255,255,0.02)",
        border: "1px dashed rgba(255,255,255,0.08)",
      }}
    >
      <span className="text-xs" style={{ color: "rgba(248,248,248,0.3)" }}>
        {data.label} 配置
      </span>
      <span className="mt-1 text-[10px]" style={{ color: "rgba(248,248,248,0.2)" }}>
        此节点类型暂无额外配置
      </span>
    </div>
  );
}
