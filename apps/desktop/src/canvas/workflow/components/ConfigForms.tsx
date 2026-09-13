import React, { useRef, useState } from "react";
import { ChevronDown, Maximize2, Plus, X, Square } from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "../../../components/shadcn/dialog";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
} from "../../../components/shadcn/dropdown-menu";
import type { CreativeNodeData, ProfessionalNodeData, WorkflowNodeData } from "../types";
import { useCanvasSettingsStore } from "../store/canvasSettingsStore";

// —— Penguin-Magic 图二设计语言：玻璃 chip 参数行 + 无边框提示词区 ——
const CHIP_STYLE: React.CSSProperties = {
  height: 32,
  padding: "0 10px",
  borderRadius: 8,
  border: "none",
  background: "rgba(255,255,255,0.08)",
  color: "rgba(255,255,255,0.75)",
  fontSize: 12,
  fontWeight: 500,
  outline: "none",
  cursor: "pointer",
  appearance: "none" as const,
};

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
}: {
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (v: string) => void;
  icon?: React.ReactNode;
  itemIcons?: Record<string, React.ReactNode>;
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
              onSelect={() => onChange(o.value)}
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
const TEXT_COLOR_88 = "rgba(255,255,255,0.88)";

export function ImageGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const [expanded, setExpanded] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const expandedTextareaRef = useRef<HTMLTextAreaElement>(null);
  const configuredModel = useCanvasSettingsStore((state) => state.scenarioModels.image.model.trim());

  const model = data.imageModel?.trim() || configuredModel;
  const modelOptions = buildModelOptions(configuredModel, model);
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

  return (
    <div className="relative flex flex-col gap-2">
      {/* 1. 输入图行（PM：始终显示 + 占位；上限 2 张，满后禁用 + 按钮） */}
      <div className="flex items-center gap-1.5 flex-wrap">
        <span className="text-[11px] flex-shrink-0" style={{ color: SUB_COLOR }}>
          输入图{inputUrls.length}/{MAX_INPUT_IMAGES}
        </span>
        {inputUrls.map((url, idx) => (
          <div
            key={`${idx}-${url.slice(0, 24)}`}
            className="group relative h-10 w-10 flex-shrink-0 overflow-hidden rounded-lg transition-transform hover:scale-105"
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
          className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-lg transition-transform hover:scale-105 disabled:cursor-not-allowed disabled:hover:scale-100"
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

      {/* 2. 分镜大师标签（PM：蓝色圆角 + 一键移除） */}
      {hasStoryboardTag && (
        <div className="flex items-center gap-1.5">
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

      {/* 3. 创意库模板标签 */}
      {hasCreativeTag && (
        <div className="flex items-center gap-1.5">
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

      {/* 4. prompt（右上角留 pr-7 给放大编辑按钮让位，避免文字顶到） */}
      <div className="pr-7">
        <PromptArea
          value={data.imagePrompt ?? ""}
          placeholder={hasStoryboardTag ? "分镜指令已就绪，点击发送开始生成…" : "描述你想要生成的内容..."}
          onChange={(imagePrompt) => onUpdate({ imagePrompt })}
          rows={hasStoryboardTag ? 1 : 2}
        />
      </div>

      {/* 4b. 放大编辑按钮（PM：panel 右上角，悬浮于根容器） */}
      <button
        type="button"
        onClick={() => setExpanded(true)}
        className="absolute right-1.5 top-1.5 z-10 flex h-6 w-6 items-center justify-center rounded-md transition-colors hover:bg-white/10"
        style={{ color: "rgba(255,255,255,0.55)" }}
        title="放大编辑"
      >
        <Maximize2 className="h-3 w-3" />
      </button>

      {/* 5. 参数行：模型 / 比例 / 分辨率 / 质量(gpt) / 数量 */}
      <div className="flex flex-wrap items-center gap-1.5">
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

      {/* 5b. 自定义比例 W × H（仅 ratio=custom 时展示） */}
      {isCustomRatio && (
        <div className="flex items-center gap-1.5">
          <span className="text-[11px] flex-shrink-0" style={{ color: SUB_COLOR }}>尺寸</span>
          <input
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
            className="w-16 rounded-md px-2 py-1 text-[12px] outline-none"
            style={{
              background: "rgba(255,255,255,0.08)",
              border: "1px solid rgba(255,255,255,0.10)",
              color: "rgba(255,255,255,0.88)",
            }}
          />
          <span className="text-[11px]" style={{ color: "rgba(255,255,255,0.4)" }}>×</span>
          <input
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
            className="w-16 rounded-md px-2 py-1 text-[12px] outline-none"
            style={{
              background: "rgba(255,255,255,0.08)",
              border: "1px solid rgba(255,255,255,0.10)",
              color: "rgba(255,255,255,0.88)",
            }}
          />
        </div>
      )}

      {/* 6. 满屏编辑 Dialog（PM：TextEditorModal 风格 + Enter 提交） */}
      <Dialog open={expanded} onOpenChange={setExpanded}>
        <DialogContent className="!max-w-[640px]">
          <div className="mb-3 flex items-center justify-between">
            <DialogTitle className="text-sm font-medium" style={{ color: TEXT_COLOR_88 }}>
              提示词编辑
            </DialogTitle>
            <button
              type="button"
              onClick={() => setExpanded(false)}
              className="flex h-7 w-7 items-center justify-center rounded-md hover:bg-white/10"
              style={{ color: "rgba(255,255,255,0.6)" }}
              title="关闭"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
          <textarea
            ref={expandedTextareaRef}
            autoFocus
            value={data.imagePrompt ?? ""}
            placeholder="描述你想要生成的内容..."
            onChange={(e) => onUpdate({ imagePrompt: e.target.value })}
            onInput={(e) => {
              const t = e.currentTarget;
              t.style.height = "auto";
              t.style.height = `${Math.min(t.scrollHeight, 360)}px`;
            }}
            className="w-full resize-none rounded-lg p-3 outline-none"
            style={{
              minHeight: 220,
              color: TEXT_COLOR_88,
              fontSize: 14,
              lineHeight: 1.7,
              background: "rgba(255,255,255,0.04)",
              border: "1px solid rgba(255,255,255,0.08)",
              scrollbarWidth: "thin",
            }}
          />
        </DialogContent>
      </Dialog>
    </div>
  );
}

export function VideoGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const configuredModel = useCanvasSettingsStore((state) => state.scenarioModels.video.model.trim());
  const selectedModel = data.videoModel?.trim() || configuredModel;
  const modelOptions = buildModelOptions(configuredModel, selectedModel);

  return (
    <div className="flex flex-col gap-2.5">
      <ChipSelect
        value={selectedModel}
        options={modelOptions.length > 0 ? modelOptions : [{ value: "", label: "选择模型" }]}
        onChange={(videoModel) => onUpdate({ videoModel })}
      />
      <PromptArea
        value={data.videoPrompt ?? ""}
        placeholder="描述你想要生成的内容..."
        onChange={(videoPrompt) => onUpdate({ videoPrompt })}
      />
      <div className="flex items-center gap-1.5">
        <input
          type="number"
          min={1}
          max={60}
          style={{ ...CHIP_STYLE, width: 72, cursor: "text" }}
          value={data.videoDuration ?? 5}
          onChange={(e) => onUpdate({ videoDuration: Number(e.target.value) })}
        />
        <span className="text-[11px]" style={{ color: "rgba(255,255,255,0.4)" }}>
          秒
        </span>
      </div>
    </div>
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
