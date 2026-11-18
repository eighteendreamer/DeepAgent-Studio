import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Boxes, ExternalLink, Pencil, Plus, RefreshCw, Search, Trash2, X } from "lucide-react";
import { Button } from "../shadcn/button";
import { Checkbox } from "../shadcn/checkbox";
import { Input } from "../shadcn/input";
import { Dialog, DialogCloseIcon, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "../shadcn/dialog";
import { HarnessModelEditor } from "./HarnessModelEditor";
import { HarnessProviderEditor, PROTOCOL_LABEL } from "./HarnessProviderEditor";
import { ProviderLogo } from "./ProviderLogo";
import { VirtualList, type VirtualListPosition } from "../ui/VirtualList";
import { openExternalUrl } from "../../api";
import {
  fetchHarnessModels, listHarnessModels, listHarnessProviders, removeHarnessProvider,
  saveHarnessProvider, saveHarnessModel, setHarnessModelActive,
  type HarnessModelView, type HarnessProviderInput, type HarnessProviderView,
} from "../../harnessModelsApi";
import "./ModelsSettings.css";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const modelKey = (model: HarnessModelView) => model.modelName;
const ModelRow = memo(function ModelRow({ model, onEdit, onToggle }: {
  model: HarnessModelView;
  onEdit: (model: HarnessModelView) => void;
  onToggle: (model: HarnessModelView, active: boolean) => void;
}) {
  return <div data-model-name={model.modelName} className="grid grid-cols-[minmax(0,1fr)_96px_80px_64px] items-center gap-3 border-b border-border-theme py-3 max-[780px]:grid-cols-[minmax(0,1fr)_72px_64px]">
    <div className="min-w-0 flex-1"><div className="truncate text-[13px] font-medium">{model.displayName}</div></div>
    <span className="text-xs tabular-nums text-text-secondary">{model.contextWindow.toLocaleString()}</span>
    <span className="text-xs tabular-nums text-text-secondary max-[780px]:hidden">{model.outputLimit.toLocaleString()}</span>
    <div className="flex items-center justify-end gap-2">
      <Checkbox aria-label={`激活 ${model.modelName}`} checked={model.active} onCheckedChange={checked => onToggle(model, checked === true)} />
      <Button variant="ghost" size="icon" className="size-7" aria-label={`编辑 ${model.modelName}`} onClick={() => onEdit(model)}><Pencil className="size-3.5" /></Button>
    </div>
  </div>;
});

export function ModelsSettings() {
  const [providers, setProviders] = useState<HarnessProviderView[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [modelCache, setModelCache] = useState<Record<string, HarnessModelView[]>>({});
  const [modelErrors, setModelErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [fetchingId, setFetchingId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<HarnessModelView | null>(null);
  const [providerEditor, setProviderEditor] = useState<{ provider: HarnessProviderView | null } | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [query, setQuery] = useState("");
  const modelRequests = useRef(new Map<string, number>());
  const [providerEditorOpen, setProviderEditorOpen] = useState(false);
  const [modelEditorOpen, setModelEditorOpen] = useState(false);
  const pendingActivations = useRef(new Set<string>());
  const listPositions = useRef(new Map<string, VirtualListPosition>());
  const selected = useMemo(() => providers.find(provider => provider.id === selectedId) ?? null, [providers, selectedId]);
  const models = modelCache[selectedId ?? ""] ?? [];
  const modelsLoading = selectedId !== null && !(selectedId in modelCache) && !modelErrors[selectedId];
  const visibleModels = useMemo(() => models.filter(model => `${model.displayName} ${model.modelName}`.toLowerCase().includes(query.trim().toLowerCase())), [models, query]);
  const positionKey = selectedId ?? "";
  let listPosition = listPositions.current.get(positionKey);
  if (!listPosition) {
    listPosition = { filter: query, top: 0, width: 0, heights: new Map() };
    listPositions.current.set(positionKey, listPosition);
  }
  if (listPosition.filter !== query) {
    listPosition = { ...listPosition, filter: query, top: 0 };
    listPositions.current.set(positionKey, listPosition);
  }

  useEffect(() => {
    let cancelled = false;
    listHarnessProviders().then(next => {
      if (cancelled) return;
      setProviders(next);
      setSelectedId(next[0]?.id ?? null);
    }).catch(failure => { if (!cancelled) setError(errorText(failure)); })
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    // Local registry rows are loaded once per provider. Switching only selects a snapshot.
    // Keep per-provider versions so background replies cannot overwrite a newer mutation.
    for (const provider of providers) {
      if (modelRequests.current.has(provider.id)) continue;
      const request = 1;
      modelRequests.current.set(provider.id, request);
      listHarnessModels(provider.id).then(next => {
        if (modelRequests.current.get(provider.id) === request) {
          setModelCache(current => ({ ...current, [provider.id]: next }));
        }
      }).catch(failure => {
        if (modelRequests.current.get(provider.id) === request) {
          setModelErrors(current => ({ ...current, [provider.id]: errorText(failure) }));
        }
      });
    }
  }, [providers]);

  const editProvider = (provider: HarnessProviderView | null) => {
    setProviderEditor({ provider });
    setProviderEditorOpen(true);
  };
  const editModel = useCallback((model: HarnessModelView) => {
    setEditing(model);
    setModelEditorOpen(true);
  }, []);

  const selectProvider = (provider: HarnessProviderView) => {
    if (provider.id === selectedId) return;
    setError(null);
    setModelEditorOpen(false);
    setQuery("");
    setSelectedId(provider.id);
  };

  const saveProvider = async (input: HarnessProviderInput) => {
    const saved = await saveHarnessProvider(input);
    setProviders(current => [...current.filter(provider => provider.id !== saved.id), saved]
      .sort((a, b) => a.channel.localeCompare(b.channel)));
    selectProvider(saved);
    setProviderEditorOpen(false);
  };

  const remove = async () => {
    if (!selected) return;
    setBusy(true);
    setError(null);
    try {
      await removeHarnessProvider(selected.id);
      const next = providers.filter(provider => provider.id !== selected.id);
      modelRequests.current.set(selected.id, (modelRequests.current.get(selected.id) ?? 0) + 1);
      setProviders(next);
      setModelCache(current => { const updated = { ...current }; delete updated[selected.id]; return updated; });
      listPositions.current.delete(selected.id);
      setSelectedId(next[0]?.id ?? null);
      setQuery("");
      setDeleting(false);
    } catch (failure) {
      setError(errorText(failure));
      setDeleting(false);
    } finally {
      setBusy(false);
    }
  };

  const fetchModels = async () => {
    if (!selectedId || fetchingId) return;
    const providerId = selectedId;
    const request = (modelRequests.current.get(providerId) ?? 0) + 1;
    modelRequests.current.set(providerId, request);
    setFetchingId(providerId);
    setError(null);
    try {
      const next = await fetchHarnessModels(providerId);
      if (request === modelRequests.current.get(providerId)) {
        setModelCache(current => ({ ...current, [providerId]: next }));
        setModelErrors(current => { const updated = { ...current }; delete updated[providerId]; return updated; });
      }
    } catch (failure) {
      if (request === modelRequests.current.get(providerId)) setModelErrors(current => ({ ...current, [providerId]: errorText(failure) }));
    } finally {
      setFetchingId(null);
    }
  };

  const toggleActive = useCallback(async (model: HarnessModelView, active: boolean) => {
    const key = `${model.providerId}:${model.modelName}`;
    if (pendingActivations.current.has(key)) return;
    pendingActivations.current.add(key);
    modelRequests.current.set(model.providerId, (modelRequests.current.get(model.providerId) ?? 0) + 1);
    setError(null);
    setModelErrors(current => {
      if (!(model.providerId in current)) return current;
      const next = { ...current }; delete next[model.providerId]; return next;
    });
    setModelCache(current => ({ ...current, [model.providerId]: (current[model.providerId] ?? []).map(item => item.modelName === model.modelName ? { ...item, active } : item) }));
    try {
      await setHarnessModelActive(model.providerId, model.modelName, active);
    } catch (failure) {
      setModelCache(current => ({ ...current, [model.providerId]: (current[model.providerId] ?? []).map(item => item.modelName === model.modelName ? { ...item, active: model.active } : item) }));
      setModelErrors(current => ({ ...current, [model.providerId]: errorText(failure) }));
    } finally {
      pendingActivations.current.delete(key);
    }
  }, []);
  const renderModel = useCallback((model: HarnessModelView) => <ModelRow model={model} onEdit={editModel} onToggle={toggleActive} />, [editModel, toggleActive]);

  return (
    <div className="models-settings-surface flex h-full min-h-0 min-w-0 flex-col gap-5 overflow-hidden">
      <div className="flex shrink-0 items-center justify-between gap-3">
        <div className="flex items-center gap-2.5"><Boxes className="size-5 text-text-secondary" aria-hidden /><h2 className="text-lg font-semibold">模型</h2></div>
        <Button variant="outline" size="sm" onClick={() => editProvider(null)} disabled={busy}><Plus className="size-4" aria-hidden />添加供应商</Button>
      </div>
      {error && <div role="alert" className="rounded-xl bg-red-50 px-4 py-3 text-sm text-red-700">{error}</div>}
      <div className="grid min-h-0 min-w-0 flex-1 grid-cols-[184px_minmax(0,1fr)] gap-7 overflow-hidden max-[900px]:grid-cols-[152px_minmax(0,1fr)] max-[900px]:gap-4">
        <aside aria-label="供应商" className="flex min-h-0 min-w-0 flex-col border-r border-border-theme pr-4">
          <div className="shrink-0 px-2 py-2 text-xs font-medium text-text-secondary">供应商 <span className="ml-1 tabular-nums">{providers.length}</span></div>
          <div data-provider-scroll className="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto overscroll-contain">
          {providers.map(provider => (
            <Button key={provider.id} variant="ghost" aria-label={`选择供应商 ${provider.channel}`} aria-pressed={provider.id === selectedId}
              onClick={() => selectProvider(provider)} disabled={busy}
              className={`h-auto min-w-0 shrink-0 justify-start gap-2.5 rounded-xl px-2 py-3 text-left ${provider.id === selectedId ? "bg-elevated-bg text-text-base shadow-sm" : "hover:bg-hover-bg"}`}>
              <ProviderLogo logo={provider.logo} name={provider.channel} className="size-8 rounded-lg" />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="truncate text-[13px] font-medium text-text-base">{provider.channel}</span>
                <span className="truncate text-[11px] font-normal text-text-secondary">{provider.enabled ? PROTOCOL_LABEL[provider.protocol] : "已停用"}</span>
              </span>
            </Button>
          ))}
          </div>
        </aside>
        <section className="flex min-h-0 min-w-0 flex-col overflow-hidden">
          {selected ? <>
            <div className="flex shrink-0 items-start gap-3">
              <ProviderLogo logo={selected.logo} name={selected.channel} className="size-10 rounded-lg" />
              <div className="min-w-0 flex-1 pt-0.5">
                <h3 className="truncate text-base font-semibold">{selected.channel}</h3>
                <div className="mt-1 flex items-center gap-1.5 text-xs text-text-secondary"><span className={`size-1.5 rounded-full ${selected.enabled && selected.hasApiKey ? "bg-emerald-500" : "bg-gray-400"}`} />{!selected.enabled ? "已停用" : selected.hasApiKey ? "已配置密钥" : "未配置密钥"}</div>
              </div>
              <Button variant="outline" size="sm" aria-label="编辑供应商" onClick={() => editProvider(selected)} disabled={busy}>配置</Button>
              <Button variant="ghost" size="icon" className="size-8 hover:text-red-600" aria-label="删除供应商" onClick={() => setDeleting(true)} disabled={busy}><Trash2 className="size-3.5" /></Button>
            </div>
            <div className="mt-3 flex min-w-0 shrink-0 items-center gap-2 text-xs text-text-secondary">
              <span className="rounded-md bg-hover-bg px-2 py-1">{PROTOCOL_LABEL[selected.protocol]}</span>
              <span className="min-w-0 flex-1 truncate">{selected.modelUrl}</span>
              {selected.websiteUrl ? <Button variant="outline" size="sm" className="shrink-0 gap-1.5 text-text-base" onClick={() => void openExternalUrl(selected.websiteUrl).catch(failure => setError(errorText(failure)))}>访问官网<ExternalLink className="size-3.5" aria-hidden /></Button> : <span />}
            </div>
            <div className="mt-5 flex shrink-0 items-center justify-between border-t border-border-theme pt-4">
              <h4 className="text-sm font-semibold">模型 <span className="ml-1 font-normal tabular-nums text-text-secondary">{models.filter(model => model.active).length} / {models.length}</span></h4>
              <Button variant="outline" size="sm" onClick={fetchModels} disabled={busy || fetchingId !== null || !selected.hasApiKey}>
                <RefreshCw className={`size-3.5 ${fetchingId === selectedId ? "animate-spin" : ""}`} aria-hidden />拉取模型
              </Button>
            </div>
            {models.length > 0 && <div className="relative mt-4 shrink-0"><Search className="pointer-events-none absolute left-3 top-3 size-4 text-text-secondary" /><Input aria-label="搜索模型" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索模型" className="pl-9 shadow-none" /></div>}
            <div className="mt-4 grid shrink-0 grid-cols-[minmax(0,1fr)_96px_80px_64px] gap-3 border-b border-border-theme pb-2 text-xs text-text-secondary max-[780px]:grid-cols-[minmax(0,1fr)_72px_64px]"><span>模型</span><span>上下文</span><span className="max-[780px]:hidden">输出上限</span><span className="text-right">启用</span></div>
            <div aria-busy={modelsLoading} className="min-h-0 flex-1">
              <VirtualList key={selectedId} items={visibleModels} itemKey={modelKey} renderItem={renderModel}
                estimatedHeight={53} itemHeight={53} position={listPosition} label="模型列表" empty={null}
                className="h-full overflow-y-auto overscroll-contain" />
            </div>
            {modelErrors[selected.id] ? <p role="alert" className="py-6 text-sm text-red-700">{modelErrors[selected.id]}</p>
              : modelsLoading ? <div className="py-6 text-sm text-text-secondary">正在读取已保存的模型…</div>
              : models.length === 0 ? <div className="flex min-h-32 flex-col items-center justify-center gap-3 text-text-secondary"><p className="text-sm">{selected.hasApiKey ? "暂无模型" : "添加 API Key 后拉取模型"}</p>{!selected.hasApiKey && <Button variant="outline" size="sm" onClick={() => editProvider(selected)}>配置供应商</Button>}</div>
              : visibleModels.length === 0 && <p className="py-10 text-center text-sm text-text-secondary">没有匹配的模型</p>}
          </> : <div className="flex min-h-80 items-center justify-center text-sm text-text-secondary">{loading ? "加载供应商…" : "添加供应商以配置模型"}</div>}
        </section>
      </div>
      <Dialog open={providerEditorOpen} onOpenChange={setProviderEditorOpen}>
        <DialogContent className="models-settings-surface max-w-[520px]" aria-labelledby="provider-editor-title" closeOnBackdrop={false}>
          <DialogHeader><DialogTitle id="provider-editor-title" className="text-base">{providerEditor?.provider ? "编辑供应商" : "添加供应商"}</DialogTitle><DialogCloseIcon aria-label="关闭供应商编辑"><X className="size-4" /></DialogCloseIcon></DialogHeader>
          {providerEditor && <HarnessProviderEditor key={providerEditor.provider?.id ?? "new"} provider={providerEditor.provider} onSave={saveProvider} onCancel={() => setProviderEditorOpen(false)} />}
        </DialogContent>
      </Dialog>
      <Dialog open={deleting} onOpenChange={setDeleting}>
        <DialogContent className="models-settings-surface max-w-[400px]" aria-labelledby="provider-delete-title" closeOnBackdrop={!busy} closeOnEscape={!busy}>
          <DialogHeader><DialogTitle id="provider-delete-title" className="text-base">删除 {selected?.channel}？</DialogTitle></DialogHeader>
          <p className="px-6 pb-4 text-sm text-text-secondary">此供应商的模型与已保存的密钥会一并删除。</p>
          <DialogFooter><Button variant="outline" disabled={busy} onClick={() => setDeleting(false)}>取消</Button><Button variant="destructive" disabled={busy} onClick={remove}>{busy ? "删除中" : "删除"}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={modelEditorOpen} onOpenChange={setModelEditorOpen}>
        <DialogContent className="models-settings-surface max-w-[520px]" aria-labelledby="model-editor-title" closeOnBackdrop={false}>
          <DialogHeader><DialogTitle id="model-editor-title" className="min-w-0 truncate text-base">{editing?.modelName}</DialogTitle><DialogCloseIcon aria-label="关闭模型编辑"><X className="size-4" /></DialogCloseIcon></DialogHeader>
          {editing && <HarnessModelEditor key={`${editing.providerId}:${editing.modelName}`} model={editing} onCancel={() => setModelEditorOpen(false)} onSave={async input => {
            modelRequests.current.set(input.providerId, (modelRequests.current.get(input.providerId) ?? 0) + 1);
            const saved = await saveHarnessModel(input);
            setModelCache(current => ({ ...current, [saved.providerId]: (current[saved.providerId] ?? []).map(model => model.modelName === saved.modelName ? saved : model) }));
            setModelEditorOpen(false);
          }} />}
        </DialogContent>
      </Dialog>
    </div>
  );
}
