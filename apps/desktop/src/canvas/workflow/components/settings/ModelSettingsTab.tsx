import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  Check,
  CheckCircle2,
  CircleAlert,
  Cpu,
  FileText,
  Image as ImageIcon,
  KeyRound,
  ListChecks,
  Pencil,
  Plus,
  Radio,
  RefreshCw,
  Search,
  Server,
  Trash2,
  Video,
  X,
  type LucideIcon,
} from "lucide-react";
import { Button } from "../../../../components/shadcn/button";
import { Dialog, DialogContent, DialogTitle } from "../../../../components/shadcn/dialog";
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
  type ModelProvider,
  type ScenarioKind,
} from "../../store/canvasSettingsStore";
import {
  ACCENT,
  BORDER_COLOR,
  INPUT_BG,
  TEXT_MUTED,
  TEXT_PRIMARY,
  TEXT_SECONDARY,
} from "../CanvasSettingsDialog";

const SOFT_LINE = "rgba(255,255,255,0.06)";
const SURFACE_BG = "rgba(255,255,255,0.025)";
const SURFACE_ACTIVE = "rgba(255,255,255,0.07)";
const SUCCESS = "#22c55e";
const WARNING = "#f59e0b";

const PROTOCOLS: { value: ModelProtocol; label: string; hint: string }[] = [
  { value: "openai", label: "OpenAI 兼容", hint: "Responses / Chat Completions 兼容供应商" },
  { value: "deepseek", label: "DeepSeek", hint: "DeepSeek 官方与兼容部署" },
  { value: "anthropic", label: "Anthropic", hint: "Claude 系列模型供应商" },
  { value: "custom", label: "自定义", hint: "内部网关、代理或私有模型服务" },
];

const SCENARIOS: { key: ScenarioKind; label: string; desc: string; Icon: LucideIcon }[] = [
  { key: "text", label: "文本", desc: "对话、代码、节点文本生成", Icon: FileText },
  { key: "image", label: "生图", desc: "文生图、图生图、图片编辑", Icon: ImageIcon },
  { key: "video", label: "视频", desc: "文生视频、图生视频、首尾帧", Icon: Video },
];

interface ProviderFormState {
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  apiKey: string;
}

type ProviderRuntimeStatus = {
  models?: string;
  connection?: string;
};

type ScenarioRuntimeStatus = Partial<Record<ScenarioKind, string>>;

function emptyForm(): ProviderFormState {
  return { name: "", protocol: "openai", baseUrl: "", apiKey: "" };
}

function protocolLabel(protocol: ModelProtocol) {
  return PROTOCOLS.find((item) => item.value === protocol)?.label ?? protocol;
}

function protocolHint(protocol: ModelProtocol) {
  return PROTOCOLS.find((item) => item.value === protocol)?.hint ?? "自定义模型服务";
}

function isProviderReady(provider: ModelProvider | undefined) {
  return Boolean(provider?.baseUrl.trim() && provider.apiKey.trim());
}

function providerStatus(provider: ModelProvider | undefined) {
  if (!provider) return "请选择供应商";
  if (!provider.baseUrl.trim() && !provider.apiKey.trim()) return "缺少 URL、API Key";
  if (!provider.baseUrl.trim()) return "缺少 Base URL";
  if (!provider.apiKey.trim()) return "缺少 API Key";
  return "配置完整";
}

function maskedKey(apiKey: string) {
  if (!apiKey.trim()) return "未配置 API Key";
  if (apiKey.length < 8) return "••••••••";
  return `${apiKey.slice(0, 3)}••••••••${apiKey.slice(-3)}`;
}

function StatusDot({ ready }: { ready: boolean }) {
  return (
    <span
      className="h-1.5 w-1.5 rounded-full"
      style={{ background: ready ? SUCCESS : WARNING }}
      aria-hidden="true"
    />
  );
}

function ActionButton({
  children,
  onClick,
  disabled,
  accent = false,
}: {
  children: ReactNode;
  onClick?: () => void;
  disabled?: boolean;
  accent?: boolean;
}) {
  return (
    <Button
      type="button"
      variant={accent ? "default" : "outline"}
      size="sm"
      onClick={onClick}
      disabled={disabled}
      className="h-8 rounded-xl px-3 text-[12px]"
      style={{
        borderColor: BORDER_COLOR,
        background: accent ? ACCENT : disabled ? "rgba(255,255,255,0.025)" : INPUT_BG,
        color: accent ? "#fff" : disabled ? TEXT_MUTED : TEXT_SECONDARY,
      }}
    >
      {children}
    </Button>
  );
}

function Field({
  label,
  value,
  placeholder,
  type = "text",
  readOnly = false,
  onChange,
}: {
  label: string;
  value: string;
  placeholder: string;
  type?: string;
  readOnly?: boolean;
  onChange?: (value: string) => void;
}) {
  return (
    <div className="min-w-0 space-y-1.5">
      <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>
        {label}
      </Label>
      <Input
        type={type}
        value={value}
        readOnly={readOnly}
        onChange={(event) => onChange?.(event.target.value)}
        placeholder={placeholder}
        className="h-9 rounded-xl text-[12px]"
        style={{
          background: readOnly ? "rgba(255,255,255,0.035)" : INPUT_BG,
          borderColor: BORDER_COLOR,
          color: TEXT_PRIMARY,
        }}
      />
    </div>
  );
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
  const [selectedProviderId, setSelectedProviderId] = useState<string | null>(providers[0]?.id ?? null);
  const [providerSearch, setProviderSearch] = useState("");
  const [modelSearch, setModelSearch] = useState("");
  const [providerRuntime, setProviderRuntime] = useState<Record<string, ProviderRuntimeStatus>>({});
  const [scenarioRuntime, setScenarioRuntime] = useState<ScenarioRuntimeStatus>({});

  useEffect(() => {
    if (providers.length === 0) {
      setSelectedProviderId(null);
      return;
    }
    if (!selectedProviderId || !providers.some((provider) => provider.id === selectedProviderId)) {
      setSelectedProviderId(providers[0].id);
    }
  }, [providers, selectedProviderId]);

  const selectedProvider = providers.find((provider) => provider.id === selectedProviderId);
  const providerUsage = useMemo(() => {
    const usage: Record<string, ScenarioKind[]> = {};
    providers.forEach((provider) => {
      usage[provider.id] = [];
    });
    SCENARIOS.forEach(({ key }) => {
      const providerId = scenarioModels[key].providerId;
      if (providerId) {
        usage[providerId] = [...(usage[providerId] ?? []), key];
      }
    });
    return usage;
  }, [providers, scenarioModels]);

  const filteredProviders = useMemo(() => {
    const query = providerSearch.trim().toLowerCase();
    if (!query) return providers;
    return providers.filter((provider) =>
      `${provider.name} ${provider.baseUrl} ${protocolLabel(provider.protocol)}`.toLowerCase().includes(query),
    );
  }, [providerSearch, providers]);

  const knownModels = useMemo(() => {
    if (!selectedProviderId) return [];
    return Array.from(
      new Set(
        SCENARIOS.map(({ key }) => scenarioModels[key])
          .filter((binding) => binding.providerId === selectedProviderId)
          .map((binding) => binding.model.trim())
          .filter(Boolean),
      ),
    );
  }, [scenarioModels, selectedProviderId]);

  const filteredModels = useMemo(() => {
    const query = modelSearch.trim().toLowerCase();
    if (!query) return knownModels;
    return knownModels.filter((model) => model.toLowerCase().includes(query));
  }, [knownModels, modelSearch]);

  const handleAdd = () => {
    setSelectedProviderId(null);
    setEditingId(null);
    setIsAdding(true);
    setForm(emptyForm());
  };

  const handleEdit = (id: string) => {
    const provider = providers.find((item) => item.id === id);
    if (!provider) return;
    setSelectedProviderId(id);
    setIsAdding(false);
    setEditingId(id);
    setForm({
      name: provider.name,
      protocol: provider.protocol,
      baseUrl: provider.baseUrl,
      apiKey: provider.apiKey,
    });
  };

  const handleSelectProvider = (id: string) => {
    setSelectedProviderId(id);
    setEditingId(null);
    setIsAdding(false);
    setForm(emptyForm());
    setModelSearch("");
  };

  const handleSave = () => {
    if (!form.name.trim()) return;
    if (editingId) {
      updateProvider(editingId, { ...form });
      setSelectedProviderId(editingId);
      setEditingId(null);
    } else {
      const id = addProvider(form);
      setSelectedProviderId(id);
      setIsAdding(false);
    }
    setForm(emptyForm());
  };

  const handleCancel = () => {
    setEditingId(null);
    setIsAdding(false);
    setForm(emptyForm());
    if (!selectedProviderId && providers[0]) {
      setSelectedProviderId(providers[0].id);
    }
  };

  const updateProviderRuntime = (providerId: string, patch: ProviderRuntimeStatus) => {
    setProviderRuntime((current) => ({
      ...current,
      [providerId]: { ...current[providerId], ...patch },
    }));
  };

  const handleDiscoverModels = (provider: ModelProvider) => {
    updateProviderRuntime(provider.id, {
      models: isProviderReady(provider)
        ? "查询入口已就绪；接入 Tauri 安全代理后，将从供应商 /models 同步并展示模型。"
        : `暂不能查询：${providerStatus(provider)}。`,
    });
  };

  const handleTestProvider = (provider: ModelProvider) => {
    updateProviderRuntime(provider.id, {
      connection: isProviderReady(provider)
        ? "连通性检测入口已就绪；检测请求将由后端携带密钥发起，前端不直接暴露 Key。"
        : `暂不能检测：${providerStatus(provider)}。`,
    });
  };

  const handleTestScenario = (scenario: ScenarioKind) => {
    const binding = scenarioModels[scenario];
    const provider = providers.find((item) => item.id === binding.providerId);
    const status =
      provider && binding.model.trim() && isProviderReady(provider)
        ? "待接入后端模型级连通性检测。"
        : "请先选择配置完整的供应商，并填写模型名称。";
    setScenarioRuntime((current) => ({ ...current, [scenario]: status }));
  };

  const renderProviderRow = (provider: ModelProvider) => {
    const active = selectedProviderId === provider.id && !isAdding;
    const ready = isProviderReady(provider);
    const usage = providerUsage[provider.id] ?? [];
    return (
      <button
        key={provider.id}
        type="button"
        onClick={() => handleSelectProvider(provider.id)}
        className="group w-full rounded-xl border px-3 py-3 text-left transition-colors"
        style={{
          borderColor: active ? "rgba(51,156,255,0.5)" : "transparent",
          background: active ? SURFACE_ACTIVE : "transparent",
        }}
      >
        <div className="flex items-start gap-3">
          <div
            className="mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-lg"
            style={{ background: INPUT_BG, color: active ? ACCENT : TEXT_SECONDARY }}
          >
            <Server size={14} strokeWidth={1.8} />
          </div>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <span className="truncate text-[12px] font-semibold" style={{ color: TEXT_PRIMARY }}>
                {provider.name}
              </span>
              <StatusDot ready={ready} />
            </div>
            <div className="mt-1 truncate text-[10px]" style={{ color: TEXT_MUTED }}>
              {protocolLabel(provider.protocol)}
            </div>
            <div className="mt-2 flex flex-wrap gap-1">
              {usage.length ? (
                usage.map((key) => (
                  <span key={key} className="rounded-md px-1.5 py-0.5 text-[9px]" style={{ background: INPUT_BG, color: TEXT_MUTED }}>
                    {SCENARIOS.find((scenario) => scenario.key === key)?.label}
                  </span>
                ))
              ) : (
                <span className="rounded-md px-1.5 py-0.5 text-[9px]" style={{ background: INPUT_BG, color: TEXT_MUTED }}>
                  未绑定场景
                </span>
              )}
            </div>
          </div>
          <div className="flex shrink-0 gap-1 opacity-0 transition-opacity group-hover:opacity-100">
            <span
              role="button"
              tabIndex={0}
              className="flex h-6 w-6 items-center justify-center rounded-lg"
              style={{ color: TEXT_SECONDARY, background: INPUT_BG }}
              onClick={(event) => {
                event.stopPropagation();
                handleEdit(provider.id);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  event.stopPropagation();
                  handleEdit(provider.id);
                }
              }}
            >
              <Pencil size={11} />
            </span>
            <span
              role="button"
              tabIndex={0}
              className="flex h-6 w-6 items-center justify-center rounded-lg"
              style={{ color: "#ef4444", background: INPUT_BG }}
              onClick={(event) => {
                event.stopPropagation();
                removeProvider(provider.id);
              }}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  event.stopPropagation();
                  removeProvider(provider.id);
                }
              }}
            >
              <Trash2 size={11} />
            </span>
          </div>
        </div>
      </button>
    );
  };

  return (
    <div className="flex min-h-full min-w-0">
      <aside
        className="flex w-[230px] shrink-0 flex-col border-r"
        style={{ borderColor: SOFT_LINE, background: "rgba(255,255,255,0.018)" }}
      >
        <div className="border-b px-4 py-5" style={{ borderColor: SOFT_LINE }}>
          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="text-[14px] font-semibold" style={{ color: TEXT_PRIMARY }}>
                供应商
              </div>
              <div className="mt-1 text-[10px] leading-4" style={{ color: TEXT_MUTED }}>
                管理多个模型服务与默认场景
              </div>
            </div>
            <button
              type="button"
              onClick={handleAdd}
              className="flex h-7 w-7 shrink-0 items-center justify-center rounded-lg"
              style={{ background: ACCENT, color: "#fff" }}
              aria-label="添加供应商"
              title="添加供应商"
            >
              <Plus size={14} />
            </button>
          </div>
          <div className="relative mt-4">
            <Search size={13} className="pointer-events-none absolute left-2.5 top-2.5" style={{ color: TEXT_MUTED }} />
            <Input
              value={providerSearch}
              onChange={(event) => setProviderSearch(event.target.value)}
              placeholder="搜索供应商"
              className="h-8 rounded-lg pl-8 text-[11px]"
              style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
            />
          </div>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto p-2.5">
          {filteredProviders.length ? (
            <div className="space-y-1">{filteredProviders.map(renderProviderRow)}</div>
          ) : (
            <div className="flex h-full min-h-[260px] flex-col items-center justify-center px-5 text-center">
              <Cpu size={20} strokeWidth={1.6} style={{ color: TEXT_MUTED }} />
              <div className="mt-3 text-[12px]" style={{ color: TEXT_PRIMARY }}>
                {providers.length ? "没有匹配的供应商" : "还没有供应商"}
              </div>
              <div className="mt-1 text-[10px] leading-5" style={{ color: TEXT_MUTED }}>
                {providers.length ? "换一个关键词试试。" : "点击右上角加号开始配置。"}
              </div>
            </div>
          )}
        </div>

        <div className="border-t px-4 py-3" style={{ borderColor: SOFT_LINE }}>
          <div className="flex items-center gap-2 text-[10px]" style={{ color: TEXT_MUTED }}>
            <KeyRound size={12} />
            API Key 仅用于本地配置展示
          </div>
        </div>
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto">
        {selectedProvider ? (
          <div className="min-w-0">
            <header className="border-b px-7 py-5" style={{ borderColor: SOFT_LINE }}>
              <div className="flex items-start justify-between gap-5">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <h2 className="truncate text-[18px] font-semibold" style={{ color: TEXT_PRIMARY }}>
                      {selectedProvider.name}
                    </h2>
                    <span className="rounded-md px-2 py-1 text-[10px]" style={{ background: INPUT_BG, color: TEXT_SECONDARY }}>
                      {protocolLabel(selectedProvider.protocol)}
                    </span>
                    <span className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[10px]" style={{ background: isProviderReady(selectedProvider) ? "rgba(34,197,94,0.12)" : "rgba(245,158,11,0.12)", color: isProviderReady(selectedProvider) ? "rgba(134,239,172,0.9)" : "rgba(253,186,116,0.9)" }}>
                      <StatusDot ready={isProviderReady(selectedProvider)} />
                      {providerStatus(selectedProvider)}
                    </span>
                  </div>
                  <div className="mt-1 text-[11px]" style={{ color: TEXT_MUTED }}>
                    {protocolHint(selectedProvider.protocol)}
                  </div>
                </div>
                <div className="flex shrink-0 gap-2">
                  {editingId ? (
                    <>
                      <ActionButton onClick={handleCancel}>
                        <X size={13} className="mr-1.5" />
                        取消
                      </ActionButton>
                      <ActionButton accent onClick={handleSave} disabled={!form.name.trim()}>
                        <Check size={13} className="mr-1.5" />
                        保存
                      </ActionButton>
                    </>
                  ) : (
                    <ActionButton onClick={() => handleEdit(selectedProvider.id)}>
                      <Pencil size={13} className="mr-1.5" />
                      编辑配置
                    </ActionButton>
                  )}
                </div>
              </div>

              <div className="mt-5 grid grid-cols-[1fr_1fr_170px] gap-3">
                <Field
                  label="API Key"
                  value={editingId ? form.apiKey : maskedKey(selectedProvider.apiKey)}
                  placeholder="未配置 API Key"
                  type={editingId ? "password" : "text"}
                  readOnly={!editingId}
                  onChange={editingId ? (value) => setForm({ ...form, apiKey: value }) : undefined}
                />
                <Field
                  label="Base URL"
                  value={editingId ? form.baseUrl : selectedProvider.baseUrl}
                  placeholder="https://api.example.com/v1"
                  readOnly={!editingId}
                  onChange={editingId ? (value) => setForm({ ...form, baseUrl: value }) : undefined}
                />
                <div className="space-y-1.5">
                  <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>
                    协议
                  </Label>
                  {editingId ? (
                    <Select value={form.protocol} onValueChange={(value) => setForm({ ...form, protocol: value as ModelProtocol })}>
                      <SelectTrigger className="h-9 rounded-xl text-[12px]" style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {PROTOCOLS.map((protocol) => (
                          <SelectItem key={protocol.value} value={protocol.value}>
                            {protocol.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  ) : (
                    <div className="flex h-9 items-center rounded-xl border px-3 text-[12px]" style={{ background: "rgba(255,255,255,0.035)", borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}>
                      {protocolLabel(selectedProvider.protocol)}
                    </div>
                  )}
                </div>
              </div>
            </header>

            <section className="border-b px-7 py-5" style={{ borderColor: SOFT_LINE }}>
              <div className="flex items-center justify-between gap-4">
                <div>
                  <div className="flex items-center gap-2">
                    <ListChecks size={15} style={{ color: TEXT_SECONDARY }} />
                    <h3 className="text-[14px] font-semibold" style={{ color: TEXT_PRIMARY }}>
                      模型列表
                    </h3>
                    <span className="rounded-full px-2 py-0.5 text-[10px]" style={{ background: INPUT_BG, color: TEXT_MUTED }}>
                      {knownModels.length} 个已绑定
                    </span>
                  </div>
                  <div className="mt-1 text-[11px]" style={{ color: TEXT_MUTED }}>
                    查询供应商支持的模型，并在场景绑定中直接使用。
                  </div>
                </div>
                <div className="flex shrink-0 gap-2">
                  <ActionButton
                    disabled={!isProviderReady(selectedProvider)}
                    onClick={() => handleDiscoverModels(selectedProvider)}
                  >
                    <RefreshCw size={13} className="mr-1.5" />
                    查询支持模型
                  </ActionButton>
                  <ActionButton
                    disabled={!isProviderReady(selectedProvider)}
                    onClick={() => handleTestProvider(selectedProvider)}
                  >
                    <Activity size={13} className="mr-1.5" />
                    测试连通性
                  </ActionButton>
                </div>
              </div>

              <div className="relative mt-4 max-w-[420px]">
                <Search size={13} className="pointer-events-none absolute left-2.5 top-2.5" style={{ color: TEXT_MUTED }} />
                <Input
                  value={modelSearch}
                  onChange={(event) => setModelSearch(event.target.value)}
                  placeholder="搜索模型名称"
                  className="h-8 rounded-lg pl-8 text-[11px]"
                  style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
                />
              </div>

              <div className="mt-4 overflow-hidden rounded-xl border" style={{ borderColor: SOFT_LINE, background: SURFACE_BG }}>
                <div className="grid grid-cols-[1fr_140px_110px] border-b px-3 py-2 text-[10px]" style={{ borderColor: SOFT_LINE, color: TEXT_MUTED }}>
                  <span>模型名称</span>
                  <span>使用场景</span>
                  <span className="text-right">状态</span>
                </div>
                {filteredModels.length ? (
                  filteredModels.map((model) => {
                    const usages = SCENARIOS.filter(({ key }) => scenarioModels[key].providerId === selectedProvider.id && scenarioModels[key].model.trim() === model);
                    return (
                      <div key={model} className="grid grid-cols-[1fr_140px_110px] items-center border-b px-3 py-3 last:border-b-0" style={{ borderColor: SOFT_LINE }}>
                        <div className="flex items-center gap-2 text-[12px]" style={{ color: TEXT_PRIMARY }}>
                          <Cpu size={13} style={{ color: TEXT_SECONDARY }} />
                          {model}
                        </div>
                        <div className="flex gap-1">
                          {usages.length ? usages.map(({ key, label }) => <span key={key} className="rounded-md px-1.5 py-0.5 text-[9px]" style={{ background: INPUT_BG, color: TEXT_MUTED }}>{label}</span>) : <span style={{ color: TEXT_MUTED }}>未绑定</span>}
                        </div>
                        <div className="flex items-center justify-end gap-1 text-[10px]" style={{ color: TEXT_MUTED }}>
                          <CheckCircle2 size={12} style={{ color: SUCCESS }} />
                          已配置
                        </div>
                      </div>
                    );
                  })
                ) : (
                  <div className="flex min-h-[130px] flex-col items-center justify-center px-6 text-center">
                    <CircleAlert size={19} style={{ color: TEXT_MUTED }} />
                    <div className="mt-2 text-[12px]" style={{ color: TEXT_SECONDARY }}>
                      {providerRuntime[selectedProvider.id]?.models ?? "暂无模型清单"}
                    </div>
                    <div className="mt-1 text-[10px]" style={{ color: TEXT_MUTED }}>
                      点击“查询支持模型”后，这里展示供应商返回的模型。
                    </div>
                  </div>
                )}
              </div>

              {(providerRuntime[selectedProvider.id]?.connection || providerRuntime[selectedProvider.id]?.models) && (
                <div className="mt-3 flex items-start gap-2 rounded-lg px-3 py-2 text-[10px] leading-5" style={{ background: "rgba(51,156,255,0.07)", color: TEXT_MUTED }}>
                  <Radio size={12} className="mt-0.5 shrink-0" style={{ color: ACCENT }} />
                  <span>{providerRuntime[selectedProvider.id]?.connection ?? providerRuntime[selectedProvider.id]?.models}</span>
                </div>
              )}
            </section>

            <section className="px-7 py-5">
              <div className="mb-4">
                <div className="text-[14px] font-semibold" style={{ color: TEXT_PRIMARY }}>
                  场景绑定
                </div>
                <div className="mt-1 text-[11px]" style={{ color: TEXT_MUTED }}>
                  为文本、生图、视频分别指定默认供应商与模型。
                </div>
              </div>

              <div className="overflow-hidden rounded-xl border" style={{ borderColor: SOFT_LINE, background: SURFACE_BG }}>
                {SCENARIOS.map(({ key, label, desc, Icon }) => {
                  const binding = scenarioModels[key];
                  const provider = providers.find((item) => item.id === binding.providerId);
                  const ready = Boolean(provider && binding.model.trim() && isProviderReady(provider));
                  return (
                    <div key={key} className="grid grid-cols-[190px_170px_1fr_auto] items-center gap-3 border-b px-3 py-3 last:border-b-0" style={{ borderColor: SOFT_LINE }}>
                      <div className="flex items-center gap-2.5">
                        <div className="flex h-8 w-8 items-center justify-center rounded-lg" style={{ background: INPUT_BG, color: TEXT_SECONDARY }}>
                          <Icon size={14} />
                        </div>
                        <div className="min-w-0">
                          <div className="text-[12px] font-semibold" style={{ color: TEXT_PRIMARY }}>{label}</div>
                          <div className="truncate text-[10px]" style={{ color: TEXT_MUTED }}>{desc}</div>
                        </div>
                      </div>
                      <Select
                        value={binding.providerId ?? ""}
                        onValueChange={(value) => {
                          setSelectedProviderId(value || null);
                          setScenarioBinding(key, { ...binding, providerId: value || null });
                        }}
                      >
                        <SelectTrigger className="h-8 rounded-lg text-[11px]" style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}>
                          <SelectValue placeholder="选择供应商" />
                        </SelectTrigger>
                        <SelectContent>
                          {providers.map((item) => <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>)}
                        </SelectContent>
                      </Select>
                      <Input
                        value={binding.model}
                        onChange={(event) => setScenarioBinding(key, { ...binding, model: event.target.value })}
                        placeholder="填写模型名称"
                        className="h-8 rounded-lg text-[11px]"
                        style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}
                      />
                      <button
                        type="button"
                        onClick={() => handleTestScenario(key)}
                        className="inline-flex h-8 items-center gap-1.5 rounded-lg border px-2.5 text-[10px]"
                        style={{ borderColor: BORDER_COLOR, background: ready ? "rgba(34,197,94,0.1)" : INPUT_BG, color: ready ? "rgba(134,239,172,0.9)" : TEXT_SECONDARY }}
                      >
                        {ready ? <CheckCircle2 size={11} /> : <Activity size={11} />}
                        测试
                      </button>
                      {scenarioRuntime[key] && (
                        <div className="col-span-3 col-start-2 -mt-1 text-[10px]" style={{ color: TEXT_MUTED }}>
                          {scenarioRuntime[key]}
                        </div>
                      )}
                    </div>
                  );
                })}
              </div>
            </section>
          </div>
        ) : (
          <div className="flex min-h-full flex-col items-center justify-center px-10 text-center">
            <Server size={28} strokeWidth={1.5} style={{ color: TEXT_MUTED }} />
            <div className="mt-4 text-[15px] font-semibold" style={{ color: TEXT_PRIMARY }}>
              选择一个供应商开始配置
            </div>
            <div className="mt-2 max-w-[380px] text-[11px] leading-5" style={{ color: TEXT_MUTED }}>
              在左侧添加或选择供应商后，可以在这里管理 API Key、Base URL、模型清单和使用场景。
            </div>
            <ActionButton accent onClick={handleAdd}>
              <Plus size={13} className="mr-1.5" />
              添加供应商
            </ActionButton>
          </div>
        )}
      </main>

      <Dialog
        open={isAdding}
        onOpenChange={(open) => {
          if (!open) handleCancel();
        }}
      >
        <DialogContent
          zIndexClass="z-[11000]"
          className="w-[min(640px,calc(100vw-32px))] max-w-none rounded-2xl border border-white/10 bg-[#17181b] p-0 text-white shadow-[0_28px_80px_rgba(0,0,0,0.55)] backdrop-blur-2xl"
        >
          <DialogTitle className="sr-only">添加供应商</DialogTitle>
          <div className="border-b px-6 py-5" style={{ borderColor: SOFT_LINE }}>
            <div className="text-[18px] font-semibold" style={{ color: TEXT_PRIMARY }}>
              添加供应商
            </div>
            <div className="mt-1 text-[11px]" style={{ color: TEXT_MUTED }}>
              配置完成后即可把它绑定到文本、生图或视频场景。
            </div>
          </div>

          <div className="space-y-5 px-6 py-6">
            <div className="grid grid-cols-2 gap-4">
              <Field
                label="供应商名称"
                value={form.name}
                placeholder="如：DeepSeek 官方"
                onChange={(value) => setForm({ ...form, name: value })}
              />
              <div className="space-y-1.5">
                <Label className="text-[11px]" style={{ color: TEXT_SECONDARY }}>
                  协议类型
                </Label>
                <Select value={form.protocol} onValueChange={(value) => setForm({ ...form, protocol: value as ModelProtocol })}>
                  <SelectTrigger className="h-9 rounded-xl text-[12px]" style={{ background: INPUT_BG, borderColor: BORDER_COLOR, color: TEXT_PRIMARY }}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PROTOCOLS.map((protocol) => (
                      <SelectItem key={protocol.value} value={protocol.value}>
                        {protocol.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <Field
                label="Base URL"
                value={form.baseUrl}
                placeholder="https://api.deepseek.com/v1"
                onChange={(value) => setForm({ ...form, baseUrl: value })}
              />
              <Field
                label="API Key"
                value={form.apiKey}
                placeholder="sk-..."
                type="password"
                onChange={(value) => setForm({ ...form, apiKey: value })}
              />
            </div>
            <div className="rounded-xl border px-4 py-3 text-[11px] leading-5" style={{ borderColor: SOFT_LINE, background: SURFACE_BG, color: TEXT_MUTED }}>
              保存后，供应商会出现在左侧目录。模型列表查询和连通性测试需要后端安全代理，不会在浏览器端直接发送 API Key。
            </div>
          </div>

          <div className="flex justify-end gap-2 border-t px-6 py-4" style={{ borderColor: SOFT_LINE }}>
            <ActionButton onClick={handleCancel}>
              <X size={13} className="mr-1.5" />
              取消
            </ActionButton>
            <ActionButton accent onClick={handleSave} disabled={!form.name.trim()}>
              <Check size={13} className="mr-1.5" />
              保存供应商
            </ActionButton>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  );
}
