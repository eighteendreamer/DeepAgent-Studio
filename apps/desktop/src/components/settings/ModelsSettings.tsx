import { useEffect, useMemo, useState } from "react";
import { Boxes, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Label } from "../shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../shadcn/select";
import {
  HARNESS_BALANCE_KINDS,
  HARNESS_PROTOCOLS,
  fetchHarnessModels,
  listHarnessModels,
  listHarnessProviders,
  removeHarnessProvider,
  saveHarnessProvider,
  setHarnessModelActive,
  type HarnessBalanceKind,
  type HarnessModelView,
  type HarnessProtocol,
  type HarnessProviderInput,
  type HarnessProviderView,
} from "../../harnessModelsApi";

const PROTOCOL_LABEL: Record<HarnessProtocol, string> = {
  responses: "Responses",
  chat_completions: "Chat Completions",
  messages: "Messages (Anthropic)",
};

const BALANCE_LABEL: Record<HarnessBalanceKind, string> = {
  auto: "自动探测",
  none: "不查询",
  official: "官方接口",
  new_api: "New API",
  sub2api: "sub2api",
};

/** Form state. `id === null` means a provider not saved yet. */
interface Draft {
  id: string | null;
  channel: string;
  logo: string;
  modelUrl: string;
  protocol: HarnessProtocol;
  balanceKind: HarnessBalanceKind;
  enabled: boolean;
  /** Typed key. Empty keeps the stored key. Never pre-filled from the backend. */
  apiKey: string;
}

const EMPTY_DRAFT: Draft = {
  id: null,
  channel: "",
  logo: "",
  modelUrl: "",
  protocol: "chat_completions",
  balanceKind: "auto",
  enabled: true,
  apiKey: "",
};

function draftFromProvider(provider: HarnessProviderView): Draft {
  return {
    id: provider.id,
    channel: provider.channel,
    logo: provider.logo,
    modelUrl: provider.modelUrl,
    protocol: provider.protocol,
    balanceKind: provider.balanceKind,
    enabled: provider.enabled,
    apiKey: "",
  };
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function ModelsSettings() {
  const [providers, setProviders] = useState<HarnessProviderView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [models, setModels] = useState<HarnessModelView[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(
    () => providers.find((provider) => provider.id === selectedId) ?? null,
    [providers, selectedId],
  );

  const refreshProviders = async (): Promise<HarnessProviderView[]> => {
    const next = await listHarnessProviders();
    setProviders(next);
    return next;
  };

  useEffect(() => {
    refreshProviders().catch((err) => setError(errorText(err)));
  }, []);

  useEffect(() => {
    if (!selectedId) {
      setModels([]);
      return;
    }
    listHarnessModels(selectedId)
      .then(setModels)
      .catch((err) => setError(errorText(err)));
  }, [selectedId]);

  const selectProvider = (provider: HarnessProviderView) => {
    setError(null);
    setSelectedId(provider.id);
    setDraft(draftFromProvider(provider));
  };

  const startCreate = () => {
    setError(null);
    setSelectedId(null);
    setDraft(EMPTY_DRAFT);
  };

  const update = <K extends keyof Draft>(key: K, value: Draft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      const input: HarnessProviderInput = {
        id: draft.id,
        channel: draft.channel.trim(),
        logo: draft.logo.trim(),
        modelUrl: draft.modelUrl.trim(),
        protocol: draft.protocol,
        balanceKind: draft.balanceKind,
        enabled: draft.enabled,
        apiKey: draft.apiKey.trim() ? draft.apiKey.trim() : null,
      };
      const saved = await saveHarnessProvider(input);
      await refreshProviders();
      selectProvider(saved);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!draft.id) return;
    if (!window.confirm(`删除供应商「${draft.channel || draft.id}」？其模型与已保存的密钥会一并删除。`)) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await removeHarnessProvider(draft.id);
      await refreshProviders();
      startCreate();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const fetchModels = async () => {
    if (!draft.id) return;
    setBusy(true);
    setError(null);
    try {
      setModels(await fetchHarnessModels(draft.id));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const toggleActive = async (model: HarnessModelView, active: boolean) => {
    setError(null);
    try {
      await setHarnessModelActive(model.providerId, model.modelName, active);
      setModels((current) =>
        current.map((item) =>
          item.modelName === model.modelName ? { ...item, active } : item,
        ),
      );
    } catch (err) {
      setError(errorText(err));
    }
  };

  const canSave = draft.channel.trim() !== "" && draft.modelUrl.trim() !== "" && !busy;
  const keyPlaceholder = selected?.hasApiKey ? "已保存，留空则保持不变" : "输入 API Key";

  return (
    <div className="flex h-full min-h-0 flex-col gap-4 p-1">
      <div className="flex items-center gap-2">
        <Boxes className="size-5 text-muted-foreground" aria-hidden />
        <h2 className="text-base font-semibold">模型</h2>
        <span className="text-sm text-muted-foreground">
          添加供应商、拉取模型，激活后会出现在会话窗口的模型列表中。
        </span>
      </div>

      {error ? (
        <div role="alert" className="rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-sm text-destructive">
          {error}
        </div>
      ) : null}

      <div className="grid min-h-0 flex-1 grid-cols-[220px_minmax(0,1fr)] gap-4">
        <aside className="flex min-h-0 flex-col gap-2 overflow-y-auto">
          <Button variant="outline" size="sm" onClick={startCreate} className="justify-start">
            <Plus className="size-4" aria-hidden />
            添加供应商
          </Button>
          {providers.map((provider) => (
            <button
              key={provider.id}
              type="button"
              onClick={() => selectProvider(provider)}
              className={`flex flex-col items-start rounded-md border px-3 py-2 text-left text-sm transition-colors ${
                provider.id === selectedId ? "border-primary bg-accent" : "border-border hover:bg-accent/50"
              }`}
            >
              <span className="font-medium">{provider.channel || provider.id}</span>
              <span className="text-xs text-muted-foreground">
                {PROTOCOL_LABEL[provider.protocol]}
                {provider.enabled ? "" : " · 已停用"}
                {provider.hasApiKey ? "" : " · 未设置密钥"}
              </span>
            </button>
          ))}
        </aside>

        <section className="flex min-h-0 flex-col gap-5 overflow-y-auto">
          <div className="grid grid-cols-2 gap-4">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="harness-channel">供应商名称</Label>
              <Input
                id="harness-channel"
                value={draft.channel}
                onChange={(event) => update("channel", event.target.value)}
                placeholder="例如 DeepSeek"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="harness-logo">图标标识（可选）</Label>
              <Input
                id="harness-logo"
                value={draft.logo}
                onChange={(event) => update("logo", event.target.value)}
              />
            </div>
            <div className="col-span-2 flex flex-col gap-1.5">
              <Label htmlFor="harness-url">Base URL</Label>
              <Input
                id="harness-url"
                value={draft.modelUrl}
                onChange={(event) => update("modelUrl", event.target.value)}
                placeholder="https://api.example.com/v1"
              />
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>协议</Label>
              <Select value={draft.protocol} onValueChange={(value) => update("protocol", value as HarnessProtocol)}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {HARNESS_PROTOCOLS.map((protocol) => (
                    <SelectItem key={protocol} value={protocol}>
                      {PROTOCOL_LABEL[protocol]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="flex flex-col gap-1.5">
              <Label>余额查询</Label>
              <Select value={draft.balanceKind} onValueChange={(value) => update("balanceKind", value as HarnessBalanceKind)}>
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {HARNESS_BALANCE_KINDS.map((kind) => (
                    <SelectItem key={kind} value={kind}>
                      {BALANCE_LABEL[kind]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="col-span-2 flex flex-col gap-1.5">
              <Label htmlFor="harness-key">API Key</Label>
              <Input
                id="harness-key"
                type="password"
                autoComplete="off"
                value={draft.apiKey}
                onChange={(event) => update("apiKey", event.target.value)}
                placeholder={keyPlaceholder}
              />
            </div>
            <label className="col-span-2 flex items-center gap-2 text-sm">
              <Checkbox
                checked={draft.enabled}
                onCheckedChange={(checked) => update("enabled", checked === true)}
              />
              启用此供应商
            </label>
          </div>

          <div className="flex items-center gap-2">
            <Button onClick={save} disabled={!canSave}>
              保存
            </Button>
            {draft.id ? (
              <Button variant="outline" onClick={remove} disabled={busy}>
                <Trash2 className="size-4" aria-hidden />
                删除供应商
              </Button>
            ) : null}
          </div>

          <div className="flex flex-col gap-3 border-t pt-4">
            <div className="flex items-center justify-between">
              <h3 className="text-sm font-medium">模型</h3>
              <Button variant="outline" size="sm" onClick={fetchModels} disabled={!draft.id || busy}>
                <RefreshCw className="size-4" aria-hidden />
                拉取模型
              </Button>
            </div>
            {!draft.id ? (
              <p className="text-sm text-muted-foreground">保存供应商后即可拉取模型。</p>
            ) : models.length === 0 ? (
              <p className="text-sm text-muted-foreground">暂无模型。点击“拉取模型”从供应商获取列表。</p>
            ) : (
              <ul className="flex flex-col divide-y rounded-md border">
                {models.map((model) => (
                  <li key={model.modelName} className="flex items-center gap-3 px-3 py-2">
                    <Checkbox
                      aria-label={`激活 ${model.modelName}`}
                      checked={model.active}
                      onCheckedChange={(checked) => toggleActive(model, checked === true)}
                    />
                    <div className="flex min-w-0 flex-1 flex-col">
                      <span className="truncate text-sm font-medium">{model.displayName}</span>
                      <span className="truncate text-xs text-muted-foreground">{model.modelName}</span>
                    </div>
                    <span className="text-xs text-muted-foreground">
                      上下文 {model.contextWindow.toLocaleString()} · 输出 {model.outputLimit.toLocaleString()}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}
