import { useState } from "react";
import { Plus, Pencil, Trash2 } from "lucide-react";
import { Button } from "../../../../components/shadcn/button";
import { Input } from "../../../../components/shadcn/input";
import { Label } from "../../../../components/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../../../../components/shadcn/select";
import {
  useCanvasSettingsStore,
  type ModelProtocol,
  type ScenarioKind,
} from "../../store/canvasSettingsStore";
import { TEXT_PRIMARY, TEXT_SECONDARY, TEXT_MUTED, BORDER_COLOR, CARD_BG, INPUT_BG, ACCENT } from "../CanvasSettingsDialog";

const PROTOCOLS: { value: ModelProtocol; label: string }[] = [
  { value: "openai", label: "OpenAI 兼容" },
  { value: "deepseek", label: "DeepSeek" },
  { value: "anthropic", label: "Anthropic" },
  { value: "custom", label: "自定义" },
];

const SCENARIOS: { key: ScenarioKind; label: string; desc: string }[] = [
  { key: "text", label: "文本", desc: "对话、代码生成等文本任务" },
  { key: "image", label: "生图", desc: "图片生成任务" },
  { key: "video", label: "视频", desc: "视频生成任务" },
];

interface ProviderFormState {
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  apiKey: string;
}

function emptyForm(): ProviderFormState {
  return { name: "", protocol: "openai", baseUrl: "", apiKey: "" };
}

export function ModelSettingsTab() {
  const providers = useCanvasSettingsStore((s) => s.providers);
  const scenarioModels = useCanvasSettingsStore((s) => s.scenarioModels);
  const addProvider = useCanvasSettingsStore((s) => s.addProvider);
  const updateProvider = useCanvasSettingsStore((s) => s.updateProvider);
  const removeProvider = useCanvasSettingsStore((s) => s.removeProvider);
  const setScenarioBinding = useCanvasSettingsStore((s) => s.setScenarioBinding);

  const [editingId, setEditingId] = useState<string | null>(null);
  const [form, setForm] = useState<ProviderFormState>(emptyForm());
  const [isAdding, setIsAdding] = useState(false);

  const handleAdd = () => {
    setIsAdding(true);
    setEditingId(null);
    setForm(emptyForm());
  };

  const handleEdit = (id: string) => {
    const p = providers.find((x) => x.id === id);
    if (!p) return;
    setEditingId(id);
    setIsAdding(false);
    setForm({ name: p.name, protocol: p.protocol, baseUrl: p.baseUrl, apiKey: p.apiKey });
  };

  const handleSave = () => {
    if (!form.name.trim()) return;
    if (editingId) {
      updateProvider(editingId, { ...form });
      setEditingId(null);
    } else {
      addProvider(form);
      setIsAdding(false);
    }
    setForm(emptyForm());
  };

  const handleCancel = () => {
    setEditingId(null);
    setIsAdding(false);
    setForm(emptyForm());
  };

  const showForm = isAdding || editingId !== null;

  return (
    <>
      <h2 className="text-lg font-semibold mb-1" style={{ color: TEXT_PRIMARY }}>
        模型
      </h2>
      <p className="text-[12px] mb-6" style={{ color: TEXT_MUTED }}>
        添加模型供应商，然后为文本、生图、视频三种场景分配对应的模型。
      </p>

      {/* Provider list */}
      <div className="mb-6">
        <div className="flex items-center justify-between mb-3">
          <h3 className="text-[13px] font-semibold" style={{ color: TEXT_PRIMARY }}>
            供应商
          </h3>
          {!showForm && (
            <Button
              variant="outline"
              size="sm"
              onClick={handleAdd}
              className="h-7 text-[12px]"
              style={{
                borderColor: BORDER_COLOR,
                color: TEXT_SECONDARY,
                background: "transparent",
              }}
            >
              <Plus size={13} className="mr-1" />
              添加
            </Button>
          )}
        </div>

        {providers.length === 0 && !showForm && (
          <div
            className="rounded-lg border border-dashed px-4 py-6 text-center text-[12px]"
            style={{ borderColor: BORDER_COLOR, color: TEXT_MUTED }}
          >
            暂无供应商，点击上方"添加"按钮配置
          </div>
        )}

        <div className="space-y-2">
          {providers.map((p) => (
            <div
              key={p.id}
              className="flex items-center justify-between rounded-lg border px-3 py-2"
              style={{ borderColor: BORDER_COLOR, background: CARD_BG }}
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="text-[13px] font-medium" style={{ color: TEXT_PRIMARY }}>
                    {p.name}
                  </span>
                  <span
                    className="rounded px-1.5 py-0.5 text-[10px]"
                    style={{ background: INPUT_BG, color: TEXT_SECONDARY }}
                  >
                    {PROTOCOLS.find((x) => x.value === p.protocol)?.label ?? p.protocol}
                  </span>
                </div>
                <div className="truncate text-[11px]" style={{ color: TEXT_MUTED }}>
                  {p.baseUrl || "未设置 Base URL"}
                </div>
              </div>
              <div className="flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 w-7 p-0"
                  onClick={() => handleEdit(p.id)}
                  style={{ color: TEXT_SECONDARY }}
                >
                  <Pencil size={12} />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 w-7 p-0"
                  onClick={() => removeProvider(p.id)}
                  style={{ color: "#ef4444" }}
                >
                  <Trash2 size={12} />
                </Button>
              </div>
            </div>
          ))}
        </div>

        {showForm && (
          <div
            className="mt-3 rounded-lg border p-4 space-y-3"
            style={{ borderColor: BORDER_COLOR, background: CARD_BG }}
          >
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>名称</Label>
                <Input
                  value={form.name}
                  onChange={(e) => setForm({ ...form, name: e.target.value })}
                  placeholder="如：DeepSeek 官方"
                  className="h-8 text-[12px]"
                  style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
                />
              </div>
              <div className="space-y-1">
                <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>协议</Label>
                <Select
                  value={form.protocol}
                  onValueChange={(v) => setForm({ ...form, protocol: v as ModelProtocol })}
                >
                  <SelectTrigger className="h-8 text-[12px]" style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PROTOCOLS.map((p) => (
                      <SelectItem key={p.value} value={p.value}>
                        {p.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="space-y-1">
              <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>Base URL</Label>
              <Input
                value={form.baseUrl}
                onChange={(e) => setForm({ ...form, baseUrl: e.target.value })}
                placeholder="https://api.deepseek.com/v1"
                className="h-8 text-[12px]"
                style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
              />
            </div>
            <div className="space-y-1">
              <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>API Key</Label>
              <Input
                type="password"
                value={form.apiKey}
                onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
                placeholder="sk-..."
                className="h-8 text-[12px]"
                style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
              />
            </div>
            <div className="flex justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={handleCancel}
                className="h-7 text-[12px]"
                style={{ borderColor: BORDER_COLOR, color: TEXT_SECONDARY, background: "transparent" }}
              >
                取消
              </Button>
              <Button
                size="sm"
                onClick={handleSave}
                className="h-7 text-[12px]"
                disabled={!form.name.trim()}
                style={{ background: ACCENT, color: "#fff" }}
              >
                保存
              </Button>
            </div>
          </div>
        )}
      </div>

      {/* Scenario bindings */}
      <div>
        <h3 className="text-[13px] font-semibold mb-3" style={{ color: TEXT_PRIMARY }}>
          场景绑定
        </h3>
        <div className="space-y-3">
          {SCENARIOS.map(({ key, label, desc }) => {
            const binding = scenarioModels[key];
            return (
              <div
                key={key}
                className="flex items-center gap-4 rounded-lg border px-3 py-2"
                style={{ borderColor: BORDER_COLOR, background: CARD_BG }}
              >
                <div className="w-20 flex-shrink-0">
                  <div className="text-[13px] font-medium" style={{ color: TEXT_PRIMARY }}>
                    {label}
                  </div>
                  <div className="text-[10px]" style={{ color: TEXT_MUTED }}>
                    {desc}
                  </div>
                </div>
                <Select
                  value={binding.providerId ?? ""}
                  onValueChange={(v) =>
                    setScenarioBinding(key, { ...binding, providerId: v || null })
                  }
                >
                  <SelectTrigger className="h-8 w-[160px] text-[12px]" style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}>
                    <SelectValue placeholder="选择供应商" />
                  </SelectTrigger>
                  <SelectContent>
                    {providers.map((p) => (
                      <SelectItem key={p.id} value={p.id}>
                        {p.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Input
                  value={binding.model}
                  onChange={(e) =>
                    setScenarioBinding(key, { ...binding, model: e.target.value })
                  }
                  placeholder="模型名称（如 deepseek-chat）"
                  className="h-8 flex-1 text-[12px]"
                  style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
                />
              </div>
            );
          })}
        </div>
      </div>
    </>
  );
}
