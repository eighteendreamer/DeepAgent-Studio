import { useMemo } from "react";
import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import {
  onCanvasPreferenceChanged,
  readCanvasPreference,
  writeCanvasPreference,
} from "../utils/canvasPreferences";

/**
 * Canvas model-provider state.
 *
 * The backend is the single source of truth: providers, models and scenario
 * bindings live in the application database (same store the main-window
 * DeepSeek settings use) and api keys live in its encrypted secret store.
 * Output directories are preference documents in the same database, so this
 * store never touches localStorage; and a plaintext key never comes back from
 * the backend — only `apiKeySet` + a masked preview.
 */

export type ModelProtocol = "openai" | "anthropic" | "gemini";

/** 模型可被路由到的使用场景。一个模型可以同时支持多个场景。 */
export type ModelScenario =
  | "text"
  | "image_generation"
  | "video_generation"
  | "speech_to_text"
  | "text_to_speech"
  | "embedding";

export const MODEL_SCENARIOS: ModelScenario[] = [
  "text",
  "image_generation",
  "video_generation",
  "speech_to_text",
  "text_to_speech",
  "embedding",
];

export interface ProviderModelConfig {
  id: string;
  name: string;
  description?: string;
  enabled: boolean;
  scenarios?: ModelScenario[];
  priority?: number;
  /** Declared ceiling on reference images; carried so an edit never drops it. */
  maxReferenceImages?: number;
}

export interface ModelProvider {
  id: string;
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  /** 密钥是否已保存在后端密钥库中（前端永远拿不到明文）。 */
  apiKeySet: boolean;
  /** 后端返回的掩码预览，只用于展示。 */
  apiKeyMasked?: string;
  enabled?: boolean;
  models?: ProviderModelConfig[];
  logo?: string;
}

/** 表单草稿：`apiKey` 与其他字段共用同一条防抖落库链路，明文只进不出，不落到任何本地存储。 */
export interface ProviderDraft {
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  apiKey?: string;
  logo?: string;
  enabled?: boolean;
  models?: ProviderModelConfig[];
}

export interface ScenarioBinding {
  providerId: string | null;
  model: string;
}

export type ScenarioKind =
  | "text"
  | "image"
  | "video"
  | "speech_to_text"
  | "text_to_speech"
  | "embedding";

export interface WorkspaceConfig {
  imageDir: string;
  videoDir: string;
}

interface ProviderModelDto {
  id: string;
  name: string;
  description?: string;
  enabled: boolean;
  scenarios: string[];
  priority: number;
  maxReferenceImages?: number;
}

interface ProviderDto {
  id: string;
  name: string;
  protocol: string;
  baseUrl: string;
  enabled: boolean;
  apiKeySet: boolean;
  apiKeyMasked?: string;
  logo?: string;
  models: ProviderModelDto[];
}

interface BindingDto {
  scenario: string;
  providerId: string;
  modelId: string;
  enabled: boolean;
}

interface CanvasSettingsDto {
  providers: ProviderDto[];
  bindings: BindingDto[];
}

export interface ConnectionTestResult {
  ok: boolean;
  code: string;
  message: string;
  endpoint: string;
  modelId: string;
  latencyMs: number;
}

/** 节点上选择的模型：`providerId::modelId`。 */
export const MODEL_REF_SEPARATOR = "::";

export function encodeModelRef(providerId: string, modelId: string): string {
  return `${providerId}${MODEL_REF_SEPARATOR}${modelId}`;
}

export function decodeModelRef(ref: string): { providerId: string; modelId: string } | null {
  const index = ref.indexOf(MODEL_REF_SEPARATOR);
  if (index <= 0) return null;
  const providerId = ref.slice(0, index);
  const modelId = ref.slice(index + MODEL_REF_SEPARATOR.length);
  return modelId ? { providerId, modelId } : null;
}

/**
 * 某个场景下可选择模型的列表（供应商名 / 模型名）。
 *
 * `current` 用于把历史遗留的裸模型名（如 `deepseek-chat`）保留成可见选项，
 * 避免切换数据源后节点上已存的值凭空消失。
 */
export function scenarioModelOptions(
  providers: ModelProvider[],
  scenario: ModelScenario,
  current?: string,
): Array<{ value: string; label: string }> {
  const options = providers
    .filter((provider) => provider.enabled !== false)
    .flatMap((provider) =>
      (provider.models ?? [])
        .filter((model) => model.enabled && (model.scenarios ?? []).includes(scenario))
        .map((model) => ({
          value: encodeModelRef(provider.id, model.id),
          label: `${provider.name} / ${model.name || model.id}`,
        })),
    );
  if (current && !options.some((option) => option.value === current)) {
    options.unshift({ value: current, label: current });
  }
  return options;
}

interface WorkflowSettingsState {
  providers: ModelProvider[];
  scenarioModels: Record<ScenarioKind, ScenarioBinding>;
  workspace: WorkspaceConfig;
  loaded: boolean;
  loading: boolean;
  /** 最近一次后端写入失败的原因，用于在界面上如实提示。 */
  lastError: string | null;
  loadFromBackend: () => Promise<void>;
  clearError: () => void;
  addProvider: (draft: ProviderDraft) => Promise<string>;
  updateProvider: (id: string, patch: Partial<ProviderDraft>) => Promise<void>;
  /** 立即结算某供应商尚未落库的草稿（含密钥），返回时保证已写入后端。 */
  flushProvider: (id: string) => Promise<void>;
  removeProvider: (id: string) => Promise<void>;
  toggleProviderEnabled: (id: string) => Promise<void>;
  updateProviderModels: (id: string, models: ProviderModelConfig[]) => Promise<void>;
  addModelToProvider: (providerId: string, model: ProviderModelConfig) => Promise<void>;
  setModelScenarios: (
    providerId: string,
    modelId: string,
    scenarios: ModelScenario[],
  ) => Promise<void>;
  toggleModelEnabled: (providerId: string, modelId: string) => Promise<void>;
  removeModelFromProvider: (providerId: string, modelId: string) => Promise<void>;
  discoverModels: (providerId: string) => Promise<string[]>;
  setScenarioBinding: (scenario: ScenarioKind, binding: ScenarioBinding) => Promise<void>;
  testConnection: (
    providerId: string,
    modelId: string,
    scenario: ModelScenario,
  ) => Promise<ConnectionTestResult>;
  setWorkspaceDir: (kind: keyof WorkspaceConfig, dir: string) => void;
}

/** 连续输入合并写库的等待时间。 */
const PERSIST_DEBOUNCE_MS = 400;

const SCENARIO_KIND_TO_BACKEND: Record<ScenarioKind, ModelScenario> = {
  text: "text",
  image: "image_generation",
  video: "video_generation",
  speech_to_text: "speech_to_text",
  text_to_speech: "text_to_speech",
  embedding: "embedding",
};

const BACKEND_SCENARIO_TO_KIND: Record<string, ScenarioKind> = {
  text: "text",
  image_generation: "image",
  video_generation: "video",
  speech_to_text: "speech_to_text",
  text_to_speech: "text_to_speech",
  embedding: "embedding",
};

function defaultBindings(): Record<ScenarioKind, ScenarioBinding> {
  return {
    text: { providerId: null, model: "" },
    image: { providerId: null, model: "" },
    video: { providerId: null, model: "" },
    speech_to_text: { providerId: null, model: "" },
    text_to_speech: { providerId: null, model: "" },
    embedding: { providerId: null, model: "" },
  };
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function normalizeProtocol(protocol: string): ModelProtocol {
  if (protocol === "anthropic" || protocol === "gemini") return protocol;
  // 旧版 DeepSeek / 自定义供应商都按 OpenAI 兼容协议保留。
  return "openai";
}

function normalizeScenarios(values: unknown): ModelScenario[] {
  if (!Array.isArray(values)) return [];
  return values.filter((value): value is ModelScenario =>
    typeof value === "string" && (MODEL_SCENARIOS as string[]).includes(value),
  );
}

function modelFromDto(dto: ProviderModelDto): ProviderModelConfig {
  return {
    id: dto.id,
    name: dto.name || dto.id,
    description: dto.description ?? "",
    enabled: dto.enabled,
    scenarios: normalizeScenarios(dto.scenarios),
    priority: dto.priority ?? 0,
    maxReferenceImages: dto.maxReferenceImages,
  };
}

function providerFromDto(dto: ProviderDto): ModelProvider {
  return {
    id: dto.id,
    name: dto.name,
    protocol: normalizeProtocol(dto.protocol),
    baseUrl: dto.baseUrl,
    apiKeySet: dto.apiKeySet,
    apiKeyMasked: dto.apiKeyMasked,
    enabled: dto.enabled,
    logo: dto.logo,
    models: dto.models.map(modelFromDto),
  };
}

function toWireModel(model: ProviderModelConfig) {
  return {
    id: model.id,
    name: model.name || model.id,
    description: model.description ?? "",
    enabled: model.enabled !== false,
    scenarios: model.scenarios ?? [],
    priority: model.priority ?? 0,
    maxReferenceImages: model.maxReferenceImages,
  };
}

function toWireProvider(id: string, draft: ProviderDraft) {
  return {
    id,
    name: draft.name.trim(),
    protocol: draft.protocol,
    baseUrl: draft.baseUrl.trim(),
    enabled: draft.enabled !== false,
    logo: draft.logo?.trim() || null,
    apiKey: draft.apiKey?.trim() ? draft.apiKey.trim() : null,
    models: (draft.models ?? []).map(toWireModel),
  };
}

function draftOf(provider: ModelProvider): ProviderDraft {
  return {
    name: provider.name,
    protocol: provider.protocol,
    baseUrl: provider.baseUrl,
    logo: provider.logo,
    enabled: provider.enabled,
    models: provider.models ?? [],
  };
}

function bindingsToWire(scenarioModels: Record<ScenarioKind, ScenarioBinding>): BindingDto[] {
  return (Object.keys(SCENARIO_KIND_TO_BACKEND) as ScenarioKind[])
    .filter((kind) => scenarioModels[kind]?.providerId && scenarioModels[kind]?.model)
    .map((kind) => ({
      scenario: SCENARIO_KIND_TO_BACKEND[kind],
      providerId: scenarioModels[kind].providerId as string,
      modelId: scenarioModels[kind].model,
      enabled: true,
    }));
}

/** 每个供应商一行 workspace 级目录配置，属于本机路径而非模型资产。 */
function loadWorkspace(): WorkspaceConfig {
  const value = readCanvasPreference<Partial<WorkspaceConfig> | null>("workspace-dirs", null);
  return { imageDir: value?.imageDir ?? "", videoDir: value?.videoDir ?? "" };
}

function saveWorkspace(workspace: WorkspaceConfig): void {
  writeCanvasPreference("workspace-dirs", workspace);
}

interface PendingWrite {
  timer: ReturnType<typeof setTimeout>;
  resolvers: Array<() => void>;
}

export const useCanvasSettingsStore = create<WorkflowSettingsState>((set, get) => {
  /** 尚未落库的编辑草稿。 */
  const drafts = new Map<string, ProviderDraft>();
  /** 每个供应商一个防抖写入。 */
  const pending = new Map<string, PendingWrite>();

  const reportFailure = (action: string, error: unknown) => {
    const message = error instanceof Error ? error.message : String(error);
    console.error(`[canvas] ${action}失败:`, error);
    set({ lastError: message });
  };

  const persistProvider = async (id: string) => {
    const draft = drafts.get(id);
    if (!draft) return;
    try {
      const saved = await invoke<ProviderDto>("canvas_provider_save", {
        provider: toWireProvider(id, draft),
      });
      drafts.delete(id);
      set((state) => ({
        providers: state.providers.map((p) =>
          p.id === id ? providerFromDto(saved) : p,
        ),
      }));
    } catch (error) {
      reportFailure(`保存供应商 ${draft.name}`, error);
    }
  };

  /** 取消待写入计时器并立即落库，随后放行所有等待这次写入的调用方。 */
  const flushPersist = async (id: string) => {
    const inFlight = pending.get(id);
    if (inFlight) pending.delete(id);
    try {
      await persistProvider(id);
    } finally {
      inFlight?.resolvers.forEach((done) => done());
    }
  };

  /** 删除后端已存的供应商密钥并刷新掩码；先丢弃草稿明文，未配置时调用也是幂等的。 */
  const clearStoredApiKey = async (id: string) => {
    const draft = drafts.get(id);
    if (draft) delete draft.apiKey;
    try {
      await invoke("canvas_secret_clear", { providerId: id });
      set((state) => ({
        providers: state.providers.map((p) =>
          p.id === id ? { ...p, apiKeySet: false, apiKeyMasked: undefined } : p,
        ),
      }));
    } catch (error) {
      reportFailure("清除 APIKey", error);
    }
  };

  const schedulePersist = (id: string) =>
    new Promise<void>((resolve) => {
      const existing = pending.get(id);
      if (existing) {
        clearTimeout(existing.timer);
        existing.resolvers.push(resolve);
        pending.set(id, {
          timer: setTimeout(() => void flushPersist(id), PERSIST_DEBOUNCE_MS),
          resolvers: existing.resolvers,
        });
        return;
      }
      const resolvers = [resolve];
      pending.set(id, {
        timer: setTimeout(() => void flushPersist(id), PERSIST_DEBOUNCE_MS),
        resolvers,
      });
    });

  return {
    providers: [],
    scenarioModels: defaultBindings(),
    workspace: loadWorkspace(),
    loaded: false,
    loading: false,
    lastError: null,

    clearError: () => set({ lastError: null }),

    loadFromBackend: async () => {
      if (!isTauriRuntime()) {
        set({ loaded: true });
        return;
      }
      if (get().loading) return;
      set({ loading: true });
      try {
        const dto = await invoke<CanvasSettingsDto>("canvas_settings_read", {
          workspaceId: null,
        });
        const scenarioModels = defaultBindings();
        for (const binding of dto.bindings) {
          const kind = BACKEND_SCENARIO_TO_KIND[binding.scenario];
          if (!kind) continue;
          scenarioModels[kind] = { providerId: binding.providerId, model: binding.modelId };
        }
        const providers = dto.providers.map(providerFromDto);
        // 保留未落库的输入草稿，避免刷新覆盖用户正在编辑的文本。
        const mergedProviders = providers.map((p) => {
          const draft = drafts.get(p.id);
          if (!draft) return p;
          return {
            ...p,
            name: draft.name,
            protocol: draft.protocol,
            baseUrl: draft.baseUrl,
            logo: draft.logo,
            enabled: draft.enabled,
            models: draft.models ?? p.models,
          };
        });
        set({
          providers: mergedProviders,
          scenarioModels,
          loaded: true,
        });
      } catch (error) {
        reportFailure("读取模型供应商配置", error);
        set({ loaded: true });
      } finally {
        set({ loading: false });
      }
    },

    addProvider: async (draft) => {
      try {
        const saved = await invoke<ProviderDto>("canvas_provider_save", {
          provider: toWireProvider("", draft),
        });
        set((state) => ({ providers: [...state.providers, providerFromDto(saved)] }));
        return saved.id;
      } catch (error) {
        reportFailure("新增供应商", error);
        return "";
      }
    },

    updateProvider: async (id, patch) => {
      const current = get().providers.find((p) => p.id === id);
      if (!current) return;
      const merged: ProviderDraft = {
        ...(drafts.get(id) ?? draftOf(current)),
        ...patch,
        models: patch.models ?? (drafts.get(id) ?? draftOf(current)).models ?? [],
      };
      drafts.set(id, merged);
      set((state) => ({
        providers: state.providers.map((p) =>
          p.id === id
            ? {
                ...p,
                name: merged.name,
                protocol: merged.protocol,
                baseUrl: merged.baseUrl,
                logo: merged.logo,
                enabled: merged.enabled,
                models: merged.models ?? [],
              }
            : p,
        ),
      }));
      // 后端把空 apiKey 当作“不动已存密钥”，所以删空输入框必须显式走删除接口。
      if (patch.apiKey !== undefined && !patch.apiKey.trim()) {
        await clearStoredApiKey(id);
      }
      await schedulePersist(id);
    },

    flushProvider: (id) => flushPersist(id),

    removeProvider: async (id) => {
      drafts.delete(id);
      pending.delete(id);
      try {
        await invoke("canvas_provider_remove", { providerId: id });
      } catch (error) {
        reportFailure("删除供应商", error);
        return;
      }
      set((state) => {
        const scenarioModels = { ...state.scenarioModels };
        (Object.keys(scenarioModels) as ScenarioKind[]).forEach((kind) => {
          if (scenarioModels[kind].providerId === id) {
            scenarioModels[kind] = { providerId: null, model: "" };
          }
        });
        return {
          providers: state.providers.filter((p) => p.id !== id),
          scenarioModels,
        };
      });
    },

    toggleProviderEnabled: async (id) => {
      const current = get().providers.find((p) => p.id === id);
      if (!current) return;
      await get().updateProvider(id, { enabled: !(current.enabled !== false) });
    },

    updateProviderModels: async (id, models) => {
      await get().updateProvider(id, { models });
    },

    addModelToProvider: async (providerId, model) => {
      try {
        const saved = await invoke<ProviderDto>("canvas_model_save", {
          providerId,
          model: toWireModel(model),
        });
        drafts.delete(providerId);
        set((state) => ({
          providers: state.providers.map((p) =>
            p.id === providerId ? providerFromDto(saved) : p,
          ),
        }));
      } catch (error) {
        reportFailure("保存模型", error);
      }
    },

    setModelScenarios: async (providerId, modelId, scenarios) => {
      const provider = get().providers.find((p) => p.id === providerId);
      if (!provider) return;
      const normalized = normalizeScenarios(scenarios);
      const models = (provider.models ?? []).map((model) =>
        model.id === modelId ? { ...model, scenarios: normalized } : model,
      );
      await get().updateProviderModels(providerId, models);
    },

    toggleModelEnabled: async (providerId, modelId) => {
      const provider = get().providers.find((p) => p.id === providerId);
      if (!provider) return;
      const models = (provider.models ?? []).map((model) =>
        model.id === modelId ? { ...model, enabled: !model.enabled } : model,
      );
      await get().updateProviderModels(providerId, models);
    },

    removeModelFromProvider: async (providerId, modelId) => {
      try {
        await invoke("canvas_model_remove", { providerId, modelId });
        drafts.delete(providerId);
        set((state) => ({
          providers: state.providers.map((p) =>
            p.id === providerId
              ? { ...p, models: (p.models ?? []).filter((m) => m.id !== modelId) }
              : p,
          ),
        }));
      } catch (error) {
        reportFailure("删除模型", error);
      }
    },

    discoverModels: async (providerId) => {
      try {
        return await invoke<string[]>("canvas_models_discover", { providerId });
      } catch (error) {
        reportFailure("获取模型清单", error);
        return [];
      }
    },

    setScenarioBinding: async (scenario, binding) => {
      const scenarioModels = { ...get().scenarioModels, [scenario]: binding };
      set({ scenarioModels });
      try {
        await invoke("canvas_bindings_save", {
          workspaceId: null,
          bindings: bindingsToWire(scenarioModels),
        });
      } catch (error) {
        reportFailure("保存场景绑定", error);
      }
    },

    testConnection: async (providerId, modelId, scenario) => {
      try {
        return await invoke<ConnectionTestResult>("canvas_provider_test", {
          providerId,
          modelId,
          scenario,
        });
      } catch (error) {
        reportFailure("测试连接", error);
        return {
          ok: false,
          code: "InvokeFailed",
          message: error instanceof Error ? error.message : String(error),
          endpoint: "",
          modelId,
          latencyMs: 0,
        };
      }
    },

    setWorkspaceDir: (kind, dir) => {
      const workspace = { ...get().workspace, [kind]: dir };
      set({ workspace });
      saveWorkspace(workspace);
    },
  };
});

// 画布窗口一启动就向内核拉取一次真实配置。
void useCanvasSettingsStore.getState().loadFromBackend();

// 目录偏好可能晚于本 store 初始化才从内核载入，载入完成后刷新展示值。
onCanvasPreferenceChanged((key) => {
  if (key !== "workspace-dirs") return;
  const next = loadWorkspace();
  const current = useCanvasSettingsStore.getState().workspace;
  if (next.imageDir !== current.imageDir || next.videoDir !== current.videoDir) {
    useCanvasSettingsStore.setState({ workspace: next });
  }
});

/**
 * 节点配置面板用：某个场景下可选的模型列表。
 *
 * 后端还没有配置任何该场景的模型时，返回一个明确的“未配置模型”占位，
 * 不再写死 DeepSeek 之类的默认模型名。
 */
export function useScenarioModelOptions(
  scenario: ModelScenario,
  current?: string,
): Array<{ value: string; label: string }> {
  const providers = useCanvasSettingsStore((state) => state.providers);
  return useMemo(() => {
    const options = scenarioModelOptions(providers, scenario, current);
    return options.length > 0 ? options : [{ value: "", label: "未配置模型" }];
  }, [providers, scenario, current]);
}
