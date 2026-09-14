import React, { useRef, useState } from "react";
import { ArrowUp, ChevronDown, Maximize2, Plus, X, Square } from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "../../../components/shadcn/dialog";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
} from "../../../components/shadcn/dropdown-menu";
import type { CreativeNodeData, ProfessionalNodeData, WorkflowNodeData } from "../types";
import { useCanvasSettingsStore } from "../store/canvasSettingsStore";
import { runWorkflow } from "../utils/workflowExecutor";

const MINIMAL_INPUT_STYLE: React.CSSProperties = {
  width: "100%",
  padding: "7px 10px",
  borderRadius: 8,
  border: "1px solid rgba(255,255,255,0.06)",
  background: "rgba(255,255,255,0.04)",
  color: "rgba(248,248,248,0.85)",
  fontSize: 12,
  outline: "none",
};

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
      <input
        style={MINIMAL_INPUT_STYLE}
        placeholder="图片 A 地址..."
        value={data.leftImageUrl ?? ""}
        onChange={(e) => onUpdate({ leftImageUrl: e.target.value })}
      />
      <input
        style={MINIMAL_INPUT_STYLE}
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
          <input
            style={{ ...MINIMAL_INPUT_STYLE, flex: 1 }}
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

const INPUT_STYLE: React.CSSProperties = MINIMAL_INPUT_STYLE;

const TEXTAREA_STYLE: React.CSSProperties = {
  ...MINIMAL_INPUT_STYLE,
  minHeight: 72,
  resize: "vertical",
  fontFamily: "inherit",
};

const LABEL_STYLE: React.CSSProperties = {
  fontSize: 11,
  fontWeight: 500,
  color: "rgba(248,248,248,0.55)",
  marginBottom: 4,
  display: "block",
};

const SELECT_STYLE: React.CSSProperties = {
  ...MINIMAL_INPUT_STYLE,
  appearance: "none" as const,
  cursor: "pointer",
};

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <label style={LABEL_STYLE}>{label}</label>
      {children}
    </div>
  );
}

export function LLMForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={data.llmModel ?? "deepseek-chat"}
          onChange={(e) => onUpdate({ llmModel: e.target.value })}
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
      </Field>
      <Field label="System Prompt">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="系统提示词..."
          value={data.llmSystemPrompt ?? ""}
          onChange={(e) => onUpdate({ llmSystemPrompt: e.target.value })}
        />
      </Field>
      <Field label="User Prompt">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="用户提示词..."
          value={data.llmPrompt ?? ""}
          onChange={(e) => onUpdate({ llmPrompt: e.target.value })}
        />
      </Field>
      <div className="flex gap-2">
        <div className="flex-1">
          <Field label="Temperature">
            <input
              type="number"
              style={INPUT_STYLE}
              min={0}
              max={2}
              step={0.1}
              value={data.llmTemperature ?? 0.7}
              onChange={(e) => onUpdate({ llmTemperature: Number(e.target.value) })}
            />
          </Field>
        </div>
        <div className="flex-1">
          <Field label="Max Tokens">
            <input
              type="number"
              style={INPUT_STYLE}
              min={1}
              max={128000}
              value={data.llmMaxTokens ?? 4096}
              onChange={(e) => onUpdate({ llmMaxTokens: Number(e.target.value) })}
            />
          </Field>
        </div>
      </div>
    </div>
  );
}

export function CodeForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="语言">
        <div className="flex gap-1.5">
          {(["javascript", "python"] as const).map((lang) => (
            <button
              key={lang}
              className="flex-1 rounded-lg py-1.5 text-[11px] font-medium transition-all duration-200"
              style={{
                background: data.codeLanguage === lang ? "rgba(16,185,129,0.2)" : "rgba(255,255,255,0.05)",
                color: data.codeLanguage === lang ? "rgba(16,185,129,1)" : "rgba(248,248,248,0.5)",
                border: `1px solid ${data.codeLanguage === lang ? "rgba(16,185,129,0.35)" : "rgba(255,255,255,0.08)"}`,
              }}
              onClick={() => onUpdate({ codeLanguage: lang })}
            >
              {lang === "python" ? "Python" : "JavaScript"}
            </button>
          ))}
        </div>
      </Field>
      <Field label="代码">
        <textarea
          style={{ ...TEXTAREA_STYLE, fontFamily: "ui-monospace, monospace", minHeight: 120 }}
          placeholder="输入代码..."
          value={data.codeScript ?? ""}
          onChange={(e) => onUpdate({ codeScript: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function HttpRequestForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <div className="flex gap-2">
        <div style={{ width: 100 }}>
          <Field label="方法">
            <select
              style={SELECT_STYLE}
              value={data.httpMethod ?? "GET"}
              onChange={(e) => onUpdate({ httpMethod: e.target.value })}
            >
              <option value="GET">GET</option>
              <option value="POST">POST</option>
              <option value="PUT">PUT</option>
              <option value="DELETE">DELETE</option>
            </select>
          </Field>
        </div>
        <div className="flex-1">
          <Field label="URL">
            <input
              style={INPUT_STYLE}
              placeholder="https://api.example.com/..."
              value={data.httpUrl ?? ""}
              onChange={(e) => onUpdate({ httpUrl: e.target.value })}
            />
          </Field>
        </div>
      </div>
      <Field label="Headers (JSON)">
        <textarea
          style={{ ...TEXTAREA_STYLE, fontFamily: "ui-monospace, monospace", minHeight: 56 }}
          placeholder='{"Content-Type": "application/json"}'
          value={data.httpHeaders ? JSON.stringify(data.httpHeaders, null, 2) : ""}
          onChange={(e) => {
            try {
              onUpdate({ httpHeaders: JSON.parse(e.target.value) });
            } catch {
              // keep raw text until valid JSON
            }
          }}
        />
      </Field>
      <Field label="Body">
        <textarea
          style={{ ...TEXTAREA_STYLE, fontFamily: "ui-monospace, monospace" }}
          placeholder="请求体..."
          value={data.httpBody ?? ""}
          onChange={(e) => onUpdate({ httpBody: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function AgentForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="策略">
        <div className="flex gap-1.5">
          {([
            { value: "function-call" as const, label: "Function Call" },
            { value: "react" as const, label: "ReAct" },
          ]).map((s) => (
            <button
              key={s.value}
              className="flex-1 rounded-lg py-1.5 text-[11px] font-medium transition-all duration-200"
              style={{
                background: data.agentStrategy === s.value ? "rgba(139,92,246,0.2)" : "rgba(255,255,255,0.05)",
                color: data.agentStrategy === s.value ? "rgba(139,92,246,1)" : "rgba(248,248,248,0.5)",
                border: `1px solid ${data.agentStrategy === s.value ? "rgba(139,92,246,0.35)" : "rgba(255,255,255,0.08)"}`,
              }}
              onClick={() => onUpdate({ agentStrategy: s.value })}
            >
              {s.label}
            </button>
          ))}
        </div>
      </Field>
      <Field label={`工具 (${(data.agentTools ?? []).length})`}>
        <div className="flex flex-col gap-1">
          {(data.agentTools ?? []).map((tool, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <span className="flex-1 truncate text-[10px]" style={{ color: "rgba(248,248,248,0.6)" }}>{tool}</span>
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = (data.agentTools ?? []).filter((_, j) => j !== i);
                  onUpdate({ agentTools: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(139,92,246,0.8)", border: "1px dashed rgba(139,92,246,0.3)" }}
            onClick={() => onUpdate({ agentTools: [...(data.agentTools ?? []), "new-tool"] })}
          >
            + 添加工具
          </button>
        </div>
      </Field>
    </div>
  );
}

export function StartForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const vars = data.inputVariables ?? [];
  return (
    <div className="flex flex-col gap-3">
      <Field label={`输入变量 (${vars.length})`}>
        <div className="flex flex-col gap-1.5">
          {vars.map((v, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="变量名"
                value={v.name}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ inputVariables: next });
                }}
              />
              <select
                style={{ ...SELECT_STYLE, width: 80 }}
                value={v.type}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], type: e.target.value };
                  onUpdate({ inputVariables: next });
                }}
              >
                <option value="string">文本</option>
                <option value="number">数字</option>
                <option value="boolean">布尔</option>
              </select>
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = vars.filter((_, j) => j !== i);
                  onUpdate({ inputVariables: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(59,130,246,0.8)", border: "1px dashed rgba(59,130,246,0.3)" }}
            onClick={() =>
              onUpdate({ inputVariables: [...vars, { name: "", type: "string", required: true }] })
            }
          >
            + 添加变量
          </button>
        </div>
      </Field>
    </div>
  );
}

export function KnowledgeForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="知识库 ID">
        <input
          style={INPUT_STYLE}
          placeholder="选择知识库..."
          value={data.knowledgeBaseId ?? ""}
          onChange={(e) => onUpdate({ knowledgeBaseId: e.target.value })}
        />
      </Field>
      <Field label="Top K">
        <input
          type="number"
          style={INPUT_STYLE}
          min={1}
          max={20}
          value={data.knowledgeTopK ?? 3}
          onChange={(e) => onUpdate({ knowledgeTopK: Number(e.target.value) })}
        />
      </Field>
    </div>
  );
}

export function EndForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const vars = (data.outputVariables ?? []) as Array<{ name: string; type: string; value: string }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label={`输出变量 (${vars.length})`}>
        <div className="flex flex-col gap-1.5">
          {vars.map((v, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="变量名"
                value={v.name}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ outputVariables: next });
                }}
              />
              <select
                style={{ ...SELECT_STYLE, width: 80 }}
                value={v.type}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], type: e.target.value };
                  onUpdate({ outputVariables: next });
                }}
              >
                <option value="string">文本</option>
                <option value="number">数字</option>
                <option value="object">对象</option>
                <option value="array">数组</option>
              </select>
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = vars.filter((_, j) => j !== i);
                  onUpdate({ outputVariables: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(59,130,246,0.8)", border: "1px dashed rgba(59,130,246,0.3)" }}
            onClick={() =>
              onUpdate({ outputVariables: [...vars, { name: "", type: "string", value: "" }] })
            }
          >
            + 添加输出变量
          </button>
        </div>
      </Field>
    </div>
  );
}

const CONDITION_OPERATORS = [
  { value: "is", label: "等于" },
  { value: "is-not", label: "不等于" },
  { value: "contains", label: "包含" },
  { value: "not-contains", label: "不包含" },
  { value: "starts-with", label: "开头是" },
  { value: "ends-with", label: "结尾是" },
  { value: "empty", label: "为空" },
  { value: "not-empty", label: "不为空" },
  { value: "gt", label: "大于" },
  { value: "gte", label: "大于等于" },
  { value: "lt", label: "小于" },
  { value: "lte", label: "小于等于" },
];

export function IfElseForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const conditions = (data.conditions ?? []) as unknown as Array<{
    id: string;
    logic: "and" | "or";
    items: Array<{ variable: string; operator: string; value: string }>;
  }>;
  return (
    <div className="flex flex-col gap-3">
      {conditions.map((group, gi) => (
        <div
          key={group.id}
          className="flex flex-col gap-2 rounded-lg p-2.5"
          style={{ background: "rgba(255,255,255,0.03)", border: "1px solid rgba(255,255,255,0.06)" }}
        >
          <div className="flex items-center justify-between">
            <span className="text-[10px] font-medium" style={{ color: "rgba(248,248,248,0.45)" }}>
              {gi === 0 ? "IF" : "ELIF"}
            </span>
            <div className="flex items-center gap-1">
              {(["and", "or"] as const).map((l) => (
                <button
                  key={l}
                  className="rounded px-1.5 py-0.5 text-[10px] font-medium transition-colors"
                  style={{
                    background: group.logic === l ? "rgba(59,130,246,0.2)" : "transparent",
                    color: group.logic === l ? "rgba(59,130,246,1)" : "rgba(248,248,248,0.4)",
                  }}
                  onClick={() => {
                    const next = [...conditions];
                    next[gi] = { ...group, logic: l };
                    onUpdate({ conditions: next });
                  }}
                >
                  {l.toUpperCase()}
                </button>
              ))}
            </div>
          </div>
          {group.items.map((item, ii) => (
            <div key={ii} className="flex items-center gap-1">
              <input
                style={{ ...INPUT_STYLE, flex: 2 }}
                placeholder="变量"
                value={item.variable}
                onChange={(e) => {
                  const next = [...conditions];
                  const items = [...group.items];
                  items[ii] = { ...items[ii], variable: e.target.value };
                  next[gi] = { ...group, items };
                  onUpdate({ conditions: next });
                }}
              />
              <select
                style={{ ...SELECT_STYLE, flex: 1.5 }}
                value={item.operator}
                onChange={(e) => {
                  const next = [...conditions];
                  const items = [...group.items];
                  items[ii] = { ...items[ii], operator: e.target.value };
                  next[gi] = { ...group, items };
                  onUpdate({ conditions: next });
                }}
              >
                {CONDITION_OPERATORS.map((op) => (
                  <option key={op.value} value={op.value}>{op.label}</option>
                ))}
              </select>
              <input
                style={{ ...INPUT_STYLE, flex: 2 }}
                placeholder="值"
                value={item.value}
                onChange={(e) => {
                  const next = [...conditions];
                  const items = [...group.items];
                  items[ii] = { ...items[ii], value: e.target.value };
                  next[gi] = { ...group, items };
                  onUpdate({ conditions: next });
                }}
              />
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = [...conditions];
                  const items = group.items.filter((_, j) => j !== ii);
                  next[gi] = { ...group, items };
                  onUpdate({ conditions: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded px-2 py-0.5 text-[10px] hover:bg-white/10"
            style={{ color: "rgba(59,130,246,0.7)" }}
            onClick={() => {
              const next = [...conditions];
              next[gi] = {
                ...group,
                items: [...group.items, { variable: "", operator: "is", value: "" }],
              };
              onUpdate({ conditions: next });
            }}
          >
            + 条件
          </button>
        </div>
      ))}
      <button
        className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
        style={{ color: "rgba(59,130,246,0.8)", border: "1px dashed rgba(59,130,246,0.3)" }}
        onClick={() => {
          const next = [
            ...conditions,
            {
              id: conditions.length === 0 ? "if" : `elif-${Date.now()}`,
              logic: "and" as const,
              items: [{ variable: "", operator: "is", value: "" }],
            },
          ];
          onUpdate({ conditions: next });
        }}
      >
        + 添加分支
      </button>
    </div>
  );
}

export function IterationForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="输入数组变量">
        <input
          style={INPUT_STYLE}
          placeholder="引用上游数组变量..."
          value={(data.inputVariable as string) ?? ""}
          onChange={(e) => onUpdate({ inputVariable: e.target.value })}
        />
      </Field>
      <Field label="输出变量">
        <input
          style={INPUT_STYLE}
          placeholder="迭代输出变量名..."
          value={(data.outputVariable as string) ?? ""}
          onChange={(e) => onUpdate({ outputVariable: e.target.value })}
        />
      </Field>
      <div className="flex items-center justify-between">
        <span style={{ ...LABEL_STYLE, marginBottom: 0 }}>并行执行</span>
        <button
          className="relative h-5 w-9 rounded-full transition-colors"
          style={{
            background: data.parallel ? "rgba(16,185,129,0.6)" : "rgba(255,255,255,0.12)",
          }}
          onClick={() => onUpdate({ parallel: !data.parallel })}
        >
          <span
            className="absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all"
            style={{ left: data.parallel ? 18 : 2 }}
          />
        </button>
      </div>
      {!!data.parallel && (
        <Field label="最大并发数">
          <input
            type="number"
            style={INPUT_STYLE}
            min={1}
            max={50}
            value={(data.maxConcurrency as number) ?? 1}
            onChange={(e) => onUpdate({ maxConcurrency: Number(e.target.value) })}
          />
        </Field>
      )}
      <Field label="错误处理">
        <select
          style={SELECT_STYLE}
          value={(data.errorHandling as string) ?? "terminate"}
          onChange={(e) => onUpdate({ errorHandling: e.target.value })}
        >
          <option value="terminate">终止</option>
          <option value="continue">跳过并继续</option>
          <option value="remove">移除异常输出</option>
        </select>
      </Field>
      <div className="flex items-center justify-between">
        <span style={{ ...LABEL_STYLE, marginBottom: 0 }}>扁平化输出</span>
        <button
          className="relative h-5 w-9 rounded-full transition-colors"
          style={{
            background: data.flatten ? "rgba(16,185,129,0.6)" : "rgba(255,255,255,0.12)",
          }}
          onClick={() => onUpdate({ flatten: !data.flatten })}
        >
          <span
            className="absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all"
            style={{ left: data.flatten ? 18 : 2 }}
          />
        </button>
      </div>
    </div>
  );
}

export function QuestionClassifierForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const classes = (data.classifierClasses ?? []) as Array<{ name: string; description: string }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={(data.classifierModel as string) ?? "deepseek-chat"}
          onChange={(e) => onUpdate({ classifierModel: e.target.value })}
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
      </Field>
      <Field label="输入查询">
        <input
          style={INPUT_STYLE}
          placeholder="引用输入变量..."
          value={(data.classifierInput as string) ?? ""}
          onChange={(e) => onUpdate({ classifierInput: e.target.value })}
        />
      </Field>
      <Field label={`分类 (${classes.length})`}>
        <div className="flex flex-col gap-1.5">
          {classes.map((c, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="分类名称"
                value={c.name}
                onChange={(e) => {
                  const next = [...classes];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ classifierClasses: next });
                }}
              />
              <input
                style={{ ...INPUT_STYLE, flex: 2 }}
                placeholder="分类描述（可选）"
                value={c.description}
                onChange={(e) => {
                  const next = [...classes];
                  next[i] = { ...next[i], description: e.target.value };
                  onUpdate({ classifierClasses: next });
                }}
              />
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = classes.filter((_, j) => j !== i);
                  onUpdate({ classifierClasses: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(139,92,246,0.8)", border: "1px dashed rgba(139,92,246,0.3)" }}
            onClick={() =>
              onUpdate({ classifierClasses: [...classes, { name: "", description: "" }] })
            }
          >
            + 添加分类
          </button>
        </div>
      </Field>
      <Field label="分类指令">
        <textarea
          style={{ ...TEXTAREA_STYLE, minHeight: 56 }}
          placeholder="给模型的额外分类指令..."
          value={(data.classifierInstruction as string) ?? ""}
          onChange={(e) => onUpdate({ classifierInstruction: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function ParameterExtractorForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const params = (data.extractorParams ?? []) as Array<{
    name: string;
    type: string;
    description: string;
    required: boolean;
  }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={(data.extractorModel as string) ?? "deepseek-chat"}
          onChange={(e) => onUpdate({ extractorModel: e.target.value })}
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
      </Field>
      <Field label="输入变量">
        <input
          style={INPUT_STYLE}
          placeholder="引用输入变量..."
          value={(data.extractorInput as string) ?? ""}
          onChange={(e) => onUpdate({ extractorInput: e.target.value })}
        />
      </Field>
      <Field label={`参数 (${params.length})`}>
        <div className="flex flex-col gap-1.5">
          {params.map((p, i) => (
            <div key={i} className="flex items-center gap-1">
              <input
                style={{ ...INPUT_STYLE, flex: 1.5 }}
                placeholder="参数名"
                value={p.name}
                onChange={(e) => {
                  const next = [...params];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ extractorParams: next });
                }}
              />
              <select
                style={{ ...SELECT_STYLE, width: 70 }}
                value={p.type}
                onChange={(e) => {
                  const next = [...params];
                  next[i] = { ...next[i], type: e.target.value };
                  onUpdate({ extractorParams: next });
                }}
              >
                <option value="string">文本</option>
                <option value="number">数字</option>
                <option value="boolean">布尔</option>
                <option value="array">数组</option>
                <option value="object">对象</option>
              </select>
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = params.filter((_, j) => j !== i);
                  onUpdate({ extractorParams: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(139,92,246,0.8)", border: "1px dashed rgba(139,92,246,0.3)" }}
            onClick={() =>
              onUpdate({
                extractorParams: [...params, { name: "", type: "string", description: "", required: true }],
              })
            }
          >
            + 添加参数
          </button>
        </div>
      </Field>
      <Field label="提取指令">
        <textarea
          style={{ ...TEXTAREA_STYLE, minHeight: 56 }}
          placeholder="给模型的额外提取指令..."
          value={(data.extractorInstruction as string) ?? ""}
          onChange={(e) => onUpdate({ extractorInstruction: e.target.value })}
        />
      </Field>
      <div className="flex items-center justify-between">
        <span style={{ ...LABEL_STYLE, marginBottom: 0 }}>推理模式</span>
        <button
          className="relative h-5 w-9 rounded-full transition-colors"
          style={{
            background: data.reasoningMode ? "rgba(16,185,129,0.6)" : "rgba(255,255,255,0.12)",
          }}
          onClick={() => onUpdate({ reasoningMode: !data.reasoningMode })}
        >
          <span
            className="absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all"
            style={{ left: data.reasoningMode ? 18 : 2 }}
          />
        </button>
      </div>
    </div>
  );
}

export function TemplateTransformForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const vars = (data.templateInputVariables ?? []) as Array<{ name: string; value: string }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label={`输入变量 (${vars.length})`}>
        <div className="flex flex-col gap-1.5">
          {vars.map((v, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="变量名"
                value={v.name}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ templateInputVariables: next });
                }}
              />
              <input
                style={{ ...INPUT_STYLE, flex: 2 }}
                placeholder="变量值或引用"
                value={v.value}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], value: e.target.value };
                  onUpdate({ templateInputVariables: next });
                }}
              />
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = vars.filter((_, j) => j !== i);
                  onUpdate({ templateInputVariables: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(16,185,129,0.8)", border: "1px dashed rgba(16,185,129,0.3)" }}
            onClick={() =>
              onUpdate({ templateInputVariables: [...vars, { name: "", value: "" }] })
            }
          >
            + 添加变量
          </button>
        </div>
      </Field>
      <Field label="Jinja 模板">
        <textarea
          style={{ ...TEXTAREA_STYLE, fontFamily: "ui-monospace, monospace", minHeight: 100 }}
          placeholder="Hello {{ name }}, welcome to {{ company }}!"
          value={(data.templateScript as string) ?? ""}
          onChange={(e) => onUpdate({ templateScript: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function VariableAggregatorForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const vars = (data.aggregatorVariables ?? []) as string[];
  return (
    <div className="flex flex-col gap-3">
      <Field label="输出类型">
        <select
          style={SELECT_STYLE}
          value={(data.aggregatorOutputType as string) ?? "string"}
          onChange={(e) => onUpdate({ aggregatorOutputType: e.target.value })}
        >
          <option value="string">文本</option>
          <option value="number">数字</option>
          <option value="array">数组</option>
          <option value="object">对象</option>
        </select>
      </Field>
      <Field label={`聚合变量 (${vars.length})`}>
        <div className="flex flex-col gap-1.5">
          {vars.map((v, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="引用变量..."
                value={v}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = e.target.value;
                  onUpdate({ aggregatorVariables: next });
                }}
              />
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = vars.filter((_, j) => j !== i);
                  onUpdate({ aggregatorVariables: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(16,185,129,0.8)", border: "1px dashed rgba(16,185,129,0.3)" }}
            onClick={() => onUpdate({ aggregatorVariables: [...vars, ""] })}
          >
            + 添加变量
          </button>
        </div>
      </Field>
    </div>
  );
}

export function HumanInputForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const fields = (data.humanInputFields ?? []) as Array<{
    name: string;
    type: string;
    label: string;
    required: boolean;
  }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label="提示内容">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="向用户展示的审批说明..."
          value={(data.humanInputPrompt as string) ?? ""}
          onChange={(e) => onUpdate({ humanInputPrompt: e.target.value })}
        />
      </Field>
      <Field label={`表单字段 (${fields.length})`}>
        <div className="flex flex-col gap-1.5">
          {fields.map((f, i) => (
            <div key={i} className="flex items-center gap-1">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="字段名"
                value={f.name}
                onChange={(e) => {
                  const next = [...fields];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ humanInputFields: next });
                }}
              />
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="显示标签"
                value={f.label}
                onChange={(e) => {
                  const next = [...fields];
                  next[i] = { ...next[i], label: e.target.value };
                  onUpdate({ humanInputFields: next });
                }}
              />
              <select
                style={{ ...SELECT_STYLE, width: 70 }}
                value={f.type}
                onChange={(e) => {
                  const next = [...fields];
                  next[i] = { ...next[i], type: e.target.value };
                  onUpdate({ humanInputFields: next });
                }}
              >
                <option value="string">文本</option>
                <option value="number">数字</option>
                <option value="boolean">布尔</option>
                <option value="file">文件</option>
              </select>
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = fields.filter((_, j) => j !== i);
                  onUpdate({ humanInputFields: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(245,158,11,0.8)", border: "1px dashed rgba(245,158,11,0.3)" }}
            onClick={() =>
              onUpdate({
                humanInputFields: [...fields, { name: "", type: "string", label: "", required: true }],
              })
            }
          >
            + 添加字段
          </button>
        </div>
      </Field>
    </div>
  );
}

export function AnswerForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const vars = (data.answerVariables ?? []) as Array<{ name: string; value: string }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label="回答模板">
        <textarea
          style={{ ...TEXTAREA_STYLE, minHeight: 80 }}
          placeholder="使用 {{变量名}} 引用上游变量..."
          value={(data.answerTemplate as string) ?? ""}
          onChange={(e) => onUpdate({ answerTemplate: e.target.value })}
        />
      </Field>
      <Field label={`模板变量 (${vars.length})`}>
        <div className="flex flex-col gap-1.5">
          {vars.map((v, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
                placeholder="变量名"
                value={v.name}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ answerVariables: next });
                }}
              />
              <input
                style={{ ...INPUT_STYLE, flex: 2 }}
                placeholder="引用值"
                value={v.value}
                onChange={(e) => {
                  const next = [...vars];
                  next[i] = { ...next[i], value: e.target.value };
                  onUpdate({ answerVariables: next });
                }}
              />
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = vars.filter((_, j) => j !== i);
                  onUpdate({ answerVariables: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(59,130,246,0.8)", border: "1px dashed rgba(59,130,246,0.3)" }}
            onClick={() => onUpdate({ answerVariables: [...vars, { name: "", value: "" }] })}
          >
            + 添加变量
          </button>
        </div>
      </Field>
    </div>
  );
}

export function LoopForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="循环变量">
        <input
          style={INPUT_STYLE}
          placeholder="引用循环变量..."
          value={(data.loopVariable as string) ?? ""}
          onChange={(e) => onUpdate({ loopVariable: e.target.value })}
        />
      </Field>
      <Field label="终止条件">
        <input
          style={INPUT_STYLE}
          placeholder="满足条件时退出循环..."
          value={(data.loopCondition as string) ?? ""}
          onChange={(e) => onUpdate({ loopCondition: e.target.value })}
        />
      </Field>
      <Field label="最大循环次数">
        <input
          type="number"
          style={INPUT_STYLE}
          min={1}
          max={10000}
          value={(data.loopMaxIterations as number) ?? 100}
          onChange={(e) => onUpdate({ loopMaxIterations: Number(e.target.value) })}
        />
      </Field>
      <Field label="错误处理">
        <select
          style={SELECT_STYLE}
          value={(data.errorHandling as string) ?? "terminate"}
          onChange={(e) => onUpdate({ errorHandling: e.target.value })}
        >
          <option value="terminate">终止</option>
          <option value="continue">跳过并继续</option>
        </select>
      </Field>
    </div>
  );
}

export function IterationStartForm(_props: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div
      className="flex flex-col items-center justify-center rounded-xl py-6"
      style={{ background: "rgba(255,255,255,0.02)", border: "1px dashed rgba(255,255,255,0.08)" }}
    >
      <span className="text-[11px]" style={{ color: "rgba(248,248,248,0.35)" }}>
        迭代开始节点自动接收父迭代的当前项
      </span>
    </div>
  );
}

export function LoopStartForm(_props: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div
      className="flex flex-col items-center justify-center rounded-xl py-6"
      style={{ background: "rgba(255,255,255,0.02)", border: "1px dashed rgba(255,255,255,0.08)" }}
    >
      <span className="text-[11px]" style={{ color: "rgba(248,248,248,0.35)" }}>
        循环开始节点自动继承父循环上下文
      </span>
    </div>
  );
}

export function LoopEndForm(_props: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div
      className="flex flex-col items-center justify-center rounded-xl py-6"
      style={{ background: "rgba(255,255,255,0.02)", border: "1px dashed rgba(255,255,255,0.08)" }}
    >
      <span className="text-[11px]" style={{ color: "rgba(248,248,248,0.35)" }}>
        循环结束节点标记循环体终止位置
      </span>
    </div>
  );
}

export function AgentV2Form({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  const outputs = (data.agentV2Outputs ?? []) as Array<{ name: string; type: string; description: string }>;
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={(data.agentV2Model as string) ?? "deepseek-chat"}
          onChange={(e) => onUpdate({ agentV2Model: e.target.value })}
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
      </Field>
      <Field label="任务描述">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="描述 Agent 需要完成的任务..."
          value={(data.agentV2Task as string) ?? ""}
          onChange={(e) => onUpdate({ agentV2Task: e.target.value })}
        />
      </Field>
      <Field label={`声明输出 (${outputs.length})`}>
        <div className="flex flex-col gap-1.5">
          {outputs.map((o, i) => (
            <div key={i} className="flex items-center gap-1">
              <input
                style={{ ...INPUT_STYLE, flex: 1.2 }}
                placeholder="输出名"
                value={o.name}
                onChange={(e) => {
                  const next = [...outputs];
                  next[i] = { ...next[i], name: e.target.value };
                  onUpdate({ agentV2Outputs: next });
                }}
              />
              <select
                style={{ ...SELECT_STYLE, width: 70 }}
                value={o.type}
                onChange={(e) => {
                  const next = [...outputs];
                  next[i] = { ...next[i], type: e.target.value };
                  onUpdate({ agentV2Outputs: next });
                }}
              >
                <option value="string">文本</option>
                <option value="number">数字</option>
                <option value="object">对象</option>
                <option value="array">数组</option>
              </select>
              <button
                className="shrink-0 rounded px-1 py-0.5 text-[10px] hover:bg-white/10"
                style={{ color: "rgba(239,68,68,0.7)" }}
                onClick={() => {
                  const next = outputs.filter((_, j) => j !== i);
                  onUpdate({ agentV2Outputs: next });
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            className="self-start rounded-lg px-2.5 py-1 text-[11px] hover:bg-white/10"
            style={{ color: "rgba(139,92,246,0.8)", border: "1px dashed rgba(139,92,246,0.3)" }}
            onClick={() =>
              onUpdate({ agentV2Outputs: [...outputs, { name: "", type: "string", description: "" }] })
            }
          >
            + 声明输出
          </button>
        </div>
      </Field>
      <div className="flex items-center justify-between">
        <span style={{ ...LABEL_STYLE, marginBottom: 0 }}>记忆</span>
        <button
          className="relative h-5 w-9 rounded-full transition-colors"
          style={{
            background: data.agentV2Memory ? "rgba(16,185,129,0.6)" : "rgba(255,255,255,0.12)",
          }}
          onClick={() => onUpdate({ agentV2Memory: !data.agentV2Memory })}
        >
          <span
            className="absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all"
            style={{ left: data.agentV2Memory ? 18 : 2 }}
          />
        </button>
      </div>
    </div>
  );
}

export function DocumentExtractorForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="文件变量">
        <input
          style={INPUT_STYLE}
          placeholder="引用上游文件变量..."
          value={(data.docExtractorFileVariable as string) ?? ""}
          onChange={(e) => onUpdate({ docExtractorFileVariable: e.target.value })}
        />
      </Field>
      <div className="flex items-center justify-between">
        <span style={{ ...LABEL_STYLE, marginBottom: 0 }}>输入为文件数组</span>
        <button
          className="relative h-5 w-9 rounded-full transition-colors"
          style={{
            background: data.docExtractorIsArray ? "rgba(16,185,129,0.6)" : "rgba(255,255,255,0.12)",
          }}
          onClick={() => onUpdate({ docExtractorIsArray: !data.docExtractorIsArray })}
        >
          <span
            className="absolute top-0.5 h-4 w-4 rounded-full bg-white shadow transition-all"
            style={{ left: data.docExtractorIsArray ? 18 : 2 }}
          />
        </button>
      </div>
    </div>
  );
}

export function VariableAssignerForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="目标变量">
        <input
          style={INPUT_STYLE}
          placeholder="要赋值的变量名..."
          value={(data.assignerTarget as string) ?? ""}
          onChange={(e) => onUpdate({ assignerTarget: e.target.value })}
        />
      </Field>
      <Field label="写入模式">
        <select
          style={SELECT_STYLE}
          value={(data.assignerMode as string) ?? "set"}
          onChange={(e) => onUpdate({ assignerMode: e.target.value })}
        >
          <option value="set">赋值 (set)</option>
          <option value="increment">自增 (increment)</option>
          <option value="decrement">自减 (decrement)</option>
          <option value="multiply">乘以 (multiply)</option>
          <option value="divide">除以 (divide)</option>
          <option value="clear">清空 (clear)</option>
          <option value="remove-first">移除首项 (remove first)</option>
          <option value="remove-last">移除末项 (remove last)</option>
        </select>
      </Field>
      <Field label="写入值">
        <input
          style={INPUT_STYLE}
          placeholder="引用变量或输入值..."
          value={(data.assignerValue as string) ?? ""}
          onChange={(e) => onUpdate({ assignerValue: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function ListOperatorForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="输入列表">
        <input
          style={INPUT_STYLE}
          placeholder="引用上游列表变量..."
          value={(data.listOperatorInput as string) ?? ""}
          onChange={(e) => onUpdate({ listOperatorInput: e.target.value })}
        />
      </Field>
      <Field label="操作">
        <select
          style={SELECT_STYLE}
          value={(data.listOperatorAction as string) ?? "filter"}
          onChange={(e) => onUpdate({ listOperatorAction: e.target.value })}
        >
          <option value="filter">过滤</option>
          <option value="map">映射提取</option>
          <option value="sort">排序</option>
          <option value="limit">截取</option>
        </select>
      </Field>
      <Field label="过滤/映射条件">
        <input
          style={INPUT_STYLE}
          placeholder="条件表达式..."
          value={(data.listOperatorCondition as string) ?? ""}
          onChange={(e) => onUpdate({ listOperatorCondition: e.target.value })}
        />
      </Field>
      <Field label="提取字段">
        <input
          style={INPUT_STYLE}
          placeholder="映射时提取的字段名..."
          value={(data.listOperatorExtractField as string) ?? ""}
          onChange={(e) => onUpdate({ listOperatorExtractField: e.target.value })}
        />
      </Field>
      <Field label="排序依据">
        <input
          style={INPUT_STYLE}
          placeholder="排序字段..."
          value={(data.listOperatorOrderBy as string) ?? ""}
          onChange={(e) => onUpdate({ listOperatorOrderBy: e.target.value })}
        />
      </Field>
      <Field label="数量限制">
        <input
          type="number"
          style={INPUT_STYLE}
          min={0}
          value={(data.listOperatorLimit as number) ?? 0}
          onChange={(e) => onUpdate({ listOperatorLimit: Number(e.target.value) })}
        />
      </Field>
    </div>
  );
}

export function TriggerScheduleForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <Field label="频率">
        <select
          style={INPUT_STYLE}
          value={(data.scheduleFrequency as string) ?? "daily"}
          onChange={(e) => onUpdate({ scheduleFrequency: e.target.value })}
        >
          <option value="minutely">每分钟</option>
          <option value="hourly">每小时</option>
          <option value="daily">每天</option>
          <option value="weekly">每周</option>
          <option value="monthly">每月</option>
          <option value="custom">自定义 Cron</option>
        </select>
      </Field>
      {(data.scheduleFrequency as string) !== "custom" && (
        <Field label="触发时间">
          <input
            type="time"
            style={INPUT_STYLE}
            value={(data.scheduleTime as string) ?? "09:00"}
            onChange={(e) => onUpdate({ scheduleTime: e.target.value })}
          />
        </Field>
      )}
      {(data.scheduleFrequency as string) === "weekly" && (
        <Field label="星期">
          <input
            type="text"
            style={INPUT_STYLE}
            placeholder="例: 1,3,5（周一=1）"
            value={((data.scheduleDayOfWeek as string[]) ?? []).join(",")}
            onChange={(e) => onUpdate({ scheduleDayOfWeek: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })}
          />
        </Field>
      )}
      {(data.scheduleFrequency as string) === "custom" && (
        <Field label="Cron 表达式">
          <input
            type="text"
            style={INPUT_STYLE}
            placeholder="0 9 * * 1-5"
            value={(data.scheduleCron as string) ?? ""}
            onChange={(e) => onUpdate({ scheduleCron: e.target.value })}
          />
        </Field>
      )}
      <Field label="时区">
        <input
          type="text"
          style={INPUT_STYLE}
          value={(data.scheduleTimezone as string) ?? "Asia/Shanghai"}
          onChange={(e) => onUpdate({ scheduleTimezone: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function TriggerWebhookForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <Field label="请求方法">
        <select
          style={INPUT_STYLE}
          value={(data.webhookMethod as string) ?? "POST"}
          onChange={(e) => onUpdate({ webhookMethod: e.target.value })}
        >
          <option value="GET">GET</option>
          <option value="POST">POST</option>
          <option value="PUT">PUT</option>
        </select>
      </Field>
      <Field label="路径">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="/webhook/my-endpoint"
          value={(data.webhookPath as string) ?? ""}
          onChange={(e) => onUpdate({ webhookPath: e.target.value })}
        />
      </Field>
      <Field label="鉴权方式">
        <select
          style={INPUT_STYLE}
          value={(data.webhookAuthType as string) ?? "none"}
          onChange={(e) => onUpdate({ webhookAuthType: e.target.value })}
        >
          <option value="none">无</option>
          <option value="bearer">Bearer Token</option>
          <option value="hmac">HMAC 签名</option>
          <option value="basic">Basic Auth</option>
        </select>
      </Field>
      {(data.webhookAuthType as string) !== "none" && (
        <Field label="密钥">
          <input
            type="password"
            style={INPUT_STYLE}
            placeholder="输入鉴权密钥"
            value={(data.webhookAuthSecret as string) ?? ""}
            onChange={(e) => onUpdate({ webhookAuthSecret: e.target.value })}
          />
        </Field>
      )}
      <label className="flex items-center gap-2 text-xs" style={{ color: "rgba(248,248,248,0.7)" }}>
        <input
          type="checkbox"
          checked={!!data.webhookAsync}
          onChange={(e) => onUpdate({ webhookAsync: e.target.checked })}
        />
        异步模式（立即返回 202）
      </label>
    </div>
  );
}

export function TriggerPluginForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <Field label="插件 Provider">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="选择或输入插件名称"
          value={(data.pluginProvider as string) ?? ""}
          onChange={(e) => onUpdate({ pluginProvider: e.target.value })}
        />
      </Field>
      <Field label="事件">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="插件事件名称"
          value={(data.pluginEvent as string) ?? ""}
          onChange={(e) => onUpdate({ pluginEvent: e.target.value })}
        />
      </Field>
      <Field label="凭据 ID">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="关联的凭据标识"
          value={(data.pluginCredentialId as string) ?? ""}
          onChange={(e) => onUpdate({ pluginCredentialId: e.target.value })}
        />
      </Field>
      <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.3)" }}>
        插件参数和输出 Schema 由插件声明，待后端接入后动态加载
      </span>
    </div>
  );
}

export function DatasourceForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <Field label="数据源类型">
        <select
          style={INPUT_STYLE}
          value={(data.datasourceType as string) ?? "api"}
          onChange={(e) => onUpdate({ datasourceType: e.target.value })}
        >
          <option value="api">API</option>
          <option value="database">数据库</option>
          <option value="storage">对象存储</option>
          <option value="plugin">插件</option>
        </select>
      </Field>
      <Field label="插件 / 连接器">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="数据源插件名称"
          value={(data.datasourcePlugin as string) ?? ""}
          onChange={(e) => onUpdate({ datasourcePlugin: e.target.value })}
        />
      </Field>
      <Field label="凭据 ID">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="关联的凭据标识"
          value={(data.datasourceCredentialId as string) ?? ""}
          onChange={(e) => onUpdate({ datasourceCredentialId: e.target.value })}
        />
      </Field>
      <Field label="文件扩展名过滤">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="例: .pdf,.docx,.txt"
          value={((data.datasourceExtensions as string[]) ?? []).join(",")}
          onChange={(e) => onUpdate({ datasourceExtensions: e.target.value.split(",").map((s) => s.trim()).filter(Boolean) })}
        />
      </Field>
      <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.3)" }}>
        具体参数由数据源插件声明，待后端接入后动态加载
      </span>
    </div>
  );
}

export function KnowledgeIndexForm({ data, onUpdate }: { data: ProfessionalNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <Field label="输入源变量">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="引用上游变量名"
          value={(data.indexSourceVariable as string) ?? ""}
          onChange={(e) => onUpdate({ indexSourceVariable: e.target.value })}
        />
      </Field>
      <div className="flex gap-2">
        <Field label="分块大小">
          <input
            type="number"
            style={INPUT_STYLE}
            min={100}
            max={4000}
            value={(data.indexChunkSize as number) ?? 500}
            onChange={(e) => onUpdate({ indexChunkSize: Number(e.target.value) })}
          />
        </Field>
        <Field label="重叠大小">
          <input
            type="number"
            style={INPUT_STYLE}
            min={0}
            max={1000}
            value={(data.indexChunkOverlap as number) ?? 50}
            onChange={(e) => onUpdate({ indexChunkOverlap: Number(e.target.value) })}
          />
        </Field>
      </div>
      <Field label="Embedding 模型">
        <input
          type="text"
          style={INPUT_STYLE}
          placeholder="例如 text-embedding-3-small"
          value={(data.indexEmbeddingModel as string) ?? ""}
          onChange={(e) => onUpdate({ indexEmbeddingModel: e.target.value })}
        />
      </Field>
      <Field label="检索模式">
        <select
          style={INPUT_STYLE}
          value={(data.indexRetrievalMode as string) ?? "semantic"}
          onChange={(e) => onUpdate({ indexRetrievalMode: e.target.value })}
        >
          <option value="semantic">语义检索</option>
          <option value="keyword">关键词检索</option>
          <option value="hybrid">混合检索</option>
        </select>
      </Field>
      <Field label="关键词数量">
        <input
          type="number"
          style={INPUT_STYLE}
          min={0}
          max={20}
          value={(data.indexKeywords as number) ?? 0}
          onChange={(e) => onUpdate({ indexKeywords: Number(e.target.value) })}
        />
      </Field>
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
