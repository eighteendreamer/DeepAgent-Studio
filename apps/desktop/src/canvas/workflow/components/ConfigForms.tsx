import { ChevronDown } from "lucide-react";
import type { CreativeNodeData, ProfessionalNodeData, WorkflowNodeData } from "../types";

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
}: {
  value: string;
  placeholder: string;
  onChange: (v: string) => void;
}) {
  return (
    <textarea
      value={value}
      placeholder={placeholder}
      rows={2}
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
}: {
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (v: string) => void;
}) {
  return (
    <div className="relative">
      <select style={{ ...CHIP_STYLE, paddingRight: 26 }} value={value} onChange={(e) => onChange(e.target.value)}>
        {options.map((o) => (
          <option key={o.value} value={o.value} style={{ background: "#1c1c1f" }}>
            {o.label}
          </option>
        ))}
      </select>
      <ChevronDown
        className="pointer-events-none absolute right-2 top-1/2 h-3 w-3 -translate-y-1/2"
        style={{ color: "rgba(255,255,255,0.5)" }}
      />
    </div>
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

export function ImageGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <PromptArea
        value={data.imagePrompt ?? ""}
        placeholder="描述你想要生成的内容..."
        onChange={(imagePrompt) => onUpdate({ imagePrompt })}
      />
      <div className="flex items-center gap-1.5">
        <ChipSelect
          value={data.imageModel ?? ""}
          options={[
            { value: "", label: "模型" },
            { value: "dall-e-3", label: "DALL-E 3" },
            { value: "stable-diffusion-xl", label: "SD XL" },
            { value: "midjourney-v6", label: "Midjourney V6" },
          ]}
          onChange={(imageModel) => onUpdate({ imageModel })}
        />
        <ChipSelect
          value={data.aspectRatio ?? "1:1"}
          options={[
            { value: "1:1", label: "1:1" },
            { value: "16:9", label: "16:9" },
            { value: "9:16", label: "9:16" },
            { value: "4:3", label: "4:3" },
            { value: "3:4", label: "3:4" },
          ]}
          onChange={(aspectRatio) => onUpdate({ aspectRatio })}
        />
        <ChipSelect
          value={data.resolution ?? "1024x1024"}
          options={[
            { value: "512x512", label: "512" },
            { value: "1024x1024", label: "1K" },
            { value: "1792x1024", label: "2K" },
          ]}
          onChange={(resolution) => onUpdate({ resolution })}
        />
      </div>
    </div>
  );
}

export function VideoGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-2.5">
      <SegmentedChips
        value={data.videoService ?? "sora"}
        options={[
          { value: "sora", label: "Sora" },
          { value: "veo", label: "Veo" },
          { value: "kling", label: "Kling" },
        ]}
        onChange={(videoService) => onUpdate({ videoService })}
        activeColor="rgba(59,130,246,1)"
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
