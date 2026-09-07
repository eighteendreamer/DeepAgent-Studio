import type { CreativeNodeData, ProfessionalNodeData, WorkflowNodeData } from "../types";

const INPUT_STYLE: React.CSSProperties = {
  width: "100%",
  padding: "6px 10px",
  borderRadius: 8,
  border: "1px solid rgba(255,255,255,0.1)",
  background: "rgba(255,255,255,0.05)",
  color: "rgba(248,248,248,0.85)",
  fontSize: 12,
  outline: "none",
};

const TEXTAREA_STYLE: React.CSSProperties = {
  ...INPUT_STYLE,
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
  ...INPUT_STYLE,
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

interface FormProps {
  nodeId: string;
  onUpdate: (patch: Record<string, unknown>) => void;
}

export function TextGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={data.model ?? "deepseek-chat"}
          onChange={(e) => onUpdate({ model: e.target.value })}
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
      </Field>
      <Field label="提示词">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="输入提示词..."
          value={data.prompt ?? ""}
          onChange={(e) => onUpdate({ prompt: e.target.value })}
        />
      </Field>
      {data.output && (
        <Field label="输出">
          <div className="rounded-lg px-2.5 py-2 text-xs" style={{ background: "rgba(255,255,255,0.03)", color: "rgba(248,248,248,0.6)", minHeight: 48 }}>
            {data.output}
          </div>
        </Field>
      )}
    </div>
  );
}

export function ImageGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={data.imageModel ?? ""}
          onChange={(e) => onUpdate({ imageModel: e.target.value })}
        >
          <option value="">选择图片模型</option>
          <option value="dall-e-3">DALL-E 3</option>
          <option value="stable-diffusion-xl">Stable Diffusion XL</option>
          <option value="midjourney-v6">Midjourney V6</option>
        </select>
      </Field>
      <Field label="提示词">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="描述要生成的图片..."
          value={data.imagePrompt ?? ""}
          onChange={(e) => onUpdate({ imagePrompt: e.target.value })}
        />
      </Field>
      <div className="flex gap-2">
        <div className="flex-1">
          <Field label="比例">
            <select
              style={SELECT_STYLE}
              value={data.aspectRatio ?? "1:1"}
              onChange={(e) => onUpdate({ aspectRatio: e.target.value })}
            >
              <option value="1:1">1:1</option>
              <option value="16:9">16:9</option>
              <option value="9:16">9:16</option>
              <option value="4:3">4:3</option>
              <option value="3:4">3:4</option>
            </select>
          </Field>
        </div>
        <div className="flex-1">
          <Field label="分辨率">
            <select
              style={SELECT_STYLE}
              value={data.resolution ?? "1024x1024"}
              onChange={(e) => onUpdate({ resolution: e.target.value })}
            >
              <option value="512x512">512×512</option>
              <option value="1024x1024">1024×1024</option>
              <option value="1792x1024">1792×1024</option>
            </select>
          </Field>
        </div>
      </div>
    </div>
  );
}

export function VideoGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="服务">
        <div className="flex gap-1.5">
          {(["sora", "veo", "kling"] as const).map((svc) => (
            <button
              key={svc}
              className="flex-1 rounded-lg py-1.5 text-[11px] font-medium transition-all duration-200"
              style={{
                background: data.videoService === svc ? "rgba(59,130,246,0.25)" : "rgba(255,255,255,0.05)",
                color: data.videoService === svc ? "rgba(59,130,246,1)" : "rgba(248,248,248,0.5)",
                border: `1px solid ${data.videoService === svc ? "rgba(59,130,246,0.4)" : "rgba(255,255,255,0.08)"}`,
              }}
              onClick={() => onUpdate({ videoService: svc })}
            >
              {svc.charAt(0).toUpperCase() + svc.slice(1)}
            </button>
          ))}
        </div>
      </Field>
      <Field label="提示词">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="描述要生成的视频..."
          value={data.videoPrompt ?? ""}
          onChange={(e) => onUpdate({ videoPrompt: e.target.value })}
        />
      </Field>
      <Field label="时长 (秒)">
        <input
          type="number"
          style={INPUT_STYLE}
          min={1}
          max={60}
          value={data.videoDuration ?? 5}
          onChange={(e) => onUpdate({ videoDuration: Number(e.target.value) })}
        />
      </Field>
    </div>
  );
}

export function ScriptGenForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="模型">
        <select
          style={SELECT_STYLE}
          value={data.model ?? "deepseek-chat"}
          onChange={(e) => onUpdate({ model: e.target.value })}
        >
          <option value="deepseek-chat">DeepSeek Chat</option>
          <option value="deepseek-reasoner">DeepSeek Reasoner</option>
        </select>
      </Field>
      <Field label="脚本要求">
        <textarea
          style={TEXTAREA_STYLE}
          placeholder="描述视频主题和要求..."
          value={data.prompt ?? ""}
          onChange={(e) => onUpdate({ prompt: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function ImageEditForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const modes: Array<{ value: string; label: string }> = [
    { value: "crop", label: "裁剪" },
    { value: "remove-bg", label: "去背景" },
    { value: "upscale", label: "超分辨率" },
    { value: "repaint", label: "局部重绘" },
  ];
  return (
    <div className="flex flex-col gap-3">
      <Field label="编辑模式">
        <div className="grid grid-cols-2 gap-1.5">
          {modes.map((m) => (
            <button
              key={m.value}
              className="rounded-lg py-1.5 text-[11px] font-medium transition-all duration-200"
              style={{
                background: data.editMode === m.value ? "rgba(139,92,246,0.2)" : "rgba(255,255,255,0.05)",
                color: data.editMode === m.value ? "rgba(139,92,246,1)" : "rgba(248,248,248,0.5)",
                border: `1px solid ${data.editMode === m.value ? "rgba(139,92,246,0.35)" : "rgba(255,255,255,0.08)"}`,
              }}
              onClick={() => onUpdate({ editMode: m.value })}
            >
              {m.label}
            </button>
          ))}
        </div>
      </Field>
    </div>
  );
}

export function ImageCompareForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  return (
    <div className="flex flex-col gap-3">
      <Field label="图片 A URL">
        <input
          style={INPUT_STYLE}
          placeholder="输入图片 A 地址..."
          value={data.leftImageUrl ?? ""}
          onChange={(e) => onUpdate({ leftImageUrl: e.target.value })}
        />
      </Field>
      <Field label="图片 B URL">
        <input
          style={INPUT_STYLE}
          placeholder="输入图片 B 地址..."
          value={data.rightImageUrl ?? ""}
          onChange={(e) => onUpdate({ rightImageUrl: e.target.value })}
        />
      </Field>
    </div>
  );
}

export function VideoStitchForm({ data, onUpdate }: { data: CreativeNodeData } & FormProps) {
  const urls = data.inputVideoUrls ?? [];
  return (
    <div className="flex flex-col gap-3">
      <Field label={`视频片段 (${urls.length})`}>
        <div className="flex flex-col gap-1.5">
          {urls.map((url, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <span className="text-[10px] shrink-0" style={{ color: "rgba(248,248,248,0.35)" }}>#{i + 1}</span>
              <input
                style={{ ...INPUT_STYLE, flex: 1 }}
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
      </Field>
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
              <span className="text-[10px] flex-1 truncate" style={{ color: "rgba(248,248,248,0.6)" }}>{tool}</span>
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
