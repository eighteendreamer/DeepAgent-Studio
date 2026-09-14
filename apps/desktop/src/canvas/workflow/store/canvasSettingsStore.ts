import { create } from "zustand";

const STORAGE_KEY = "workflow-settings";

export type ModelProtocol = "openai" | "anthropic" | "gemini";

/** 模型可被路由到的使用场景。一个模型可以同时支持多个场景。 */
export type ModelScenario =
  | "text"
  | "image_generation"
  | "video_generation"
  | "speech_to_text"
  | "text_to_speech";

export interface ProviderModelConfig {
  id: string;
  name: string;
  description?: string;
  enabled: boolean;
  scenarios?: ModelScenario[];
}

export interface ModelProvider {
  id: string;
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  apiKey: string;
  enabled?: boolean;
  models?: ProviderModelConfig[];
  logo?: string;
}

export interface ScenarioBinding {
  providerId: string | null;
  model: string;
}

export type ScenarioKind = "text" | "image" | "video";

export interface WorkspaceConfig {
  imageDir: string;
  videoDir: string;
}

interface WorkflowSettingsState {
  providers: ModelProvider[];
  scenarioModels: Record<ScenarioKind, ScenarioBinding>;
  workspace: WorkspaceConfig;
  addProvider: (p: Omit<ModelProvider, "id">) => string;
  updateProvider: (id: string, patch: Partial<Omit<ModelProvider, "id">>) => void;
  removeProvider: (id: string) => void;
  toggleProviderEnabled: (id: string) => void;
  updateProviderModels: (id: string, models: ProviderModelConfig[]) => void;
  addModelToProvider: (providerId: string, model: ProviderModelConfig) => void;
  setModelScenarios: (providerId: string, modelId: string, scenarios: ModelScenario[]) => void;
  toggleModelEnabled: (providerId: string, modelId: string) => void;
  removeModelFromProvider: (providerId: string, modelId: string) => void;
  setScenarioBinding: (scenario: ScenarioKind, binding: ScenarioBinding) => void;
  setWorkspaceDir: (kind: keyof WorkspaceConfig, dir: string) => void;
}

let _idCounter = 0;
function nextProviderId(existing: string[]): string {
  const taken = new Set(existing);
  let id: string;
  do {
    id = `provider-${++_idCounter}`;
  } while (taken.has(id));
  return id;
}

function defaultBindings(): Record<ScenarioKind, ScenarioBinding> {
  return {
    text: { providerId: null, model: "" },
    image: { providerId: null, model: "" },
    video: { providerId: null, model: "" },
  };
}

function isValidBinding(v: unknown): v is ScenarioBinding {
  if (!v || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  return (o.providerId === null || typeof o.providerId === "string") && typeof o.model === "string";
}

function isValidProvider(v: unknown): v is ModelProvider {
  if (!v || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  return (
    typeof o.id === "string" &&
    typeof o.name === "string" &&
    (o.protocol === "openai" || o.protocol === "anthropic" || o.protocol === "gemini" || o.protocol === "deepseek" || o.protocol === "custom") &&
    typeof o.baseUrl === "string" &&
    typeof o.apiKey === "string"
  );
}

function normalizeProtocol(protocol: string): ModelProtocol {
  if (protocol === "anthropic" || protocol === "gemini") return protocol;
  // 旧版本的 DeepSeek/自定义供应商均按 OpenAI 兼容协议保留，避免已有配置失效。
  return "openai";
}

const MODEL_SCENARIOS: ModelScenario[] = [
  "text",
  "image_generation",
  "video_generation",
  "speech_to_text",
  "text_to_speech",
];

function normalizeModelScenarios(value: unknown): ModelScenario[] {
  if (!Array.isArray(value)) return [];
  return value.filter((scenario): scenario is ModelScenario =>
    typeof scenario === "string" && MODEL_SCENARIOS.includes(scenario as ModelScenario),
  );
}

function loadPersisted(): Pick<WorkflowSettingsState, "providers" | "scenarioModels" | "workspace"> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { providers: [], scenarioModels: defaultBindings(), workspace: { imageDir: "", videoDir: "" } };
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return { providers: [], scenarioModels: defaultBindings(), workspace: { imageDir: "", videoDir: "" } };

    const providers =
      Array.isArray(parsed.providers) && parsed.providers.every(isValidProvider)
        ? parsed.providers.map((provider: any) => ({
            ...provider,
            protocol: normalizeProtocol(provider.protocol),
            enabled: provider.enabled !== false,
            logo: typeof provider.logo === "string" ? provider.logo : undefined,
            models: Array.isArray(provider.models)
              ? provider.models.map((m: any) => ({
                  id: String(m.id || ""),
                  name: String(m.name || m.id || ""),
                  description: typeof m.description === "string" ? m.description : "",
                  enabled: m.enabled !== false,
                  scenarios: normalizeModelScenarios(m.scenarios),
                }))
              : [],
          }))
        : [];
    providers.forEach((p: ModelProvider) => {
      const n = Number(p.id.replace("provider-", ""));
      if (Number.isFinite(n) && n >= _idCounter) _idCounter = n;
    });

    const sm = parsed.scenarioModels;
    const scenarioModels: Record<ScenarioKind, ScenarioBinding> =
      sm && typeof sm === "object" && isValidBinding(sm.text) && isValidBinding(sm.image) && isValidBinding(sm.video)
        ? { text: sm.text, image: sm.image, video: sm.video }
        : defaultBindings();

    const ws = parsed.workspace;
    const workspace: WorkspaceConfig =
      ws && typeof ws === "object" && typeof ws.imageDir === "string" && typeof ws.videoDir === "string"
        ? { imageDir: ws.imageDir, videoDir: ws.videoDir }
        : { imageDir: "", videoDir: "" };

    return { providers, scenarioModels, workspace };
  } catch {
    return { providers: [], scenarioModels: defaultBindings(), workspace: { imageDir: "", videoDir: "" } };
  }
}

function serialize(state: WorkflowSettingsState): string {
  return JSON.stringify({
    schemaVersion: 1,
    providers: state.providers,
    scenarioModels: state.scenarioModels,
    workspace: state.workspace,
  });
}

const persisted = loadPersisted();

export const useCanvasSettingsStore = create<WorkflowSettingsState>((set, get) => ({
  ...persisted,

  addProvider: (p) => {
    const id = nextProviderId(get().providers.map((x) => x.id));
    set((s) => ({
      providers: [
        ...s.providers,
        {
          ...p,
          id,
          enabled: p.enabled !== false,
          models: (p.models ?? []).map((model) => ({
            ...model,
            scenarios: model.scenarios ?? [],
          })),
        },
      ],
    }));
    return id;
  },

  updateProvider: (id, patch) => {
    set((s) => ({
      providers: s.providers.map((p) => (p.id === id ? { ...p, ...patch } : p)),
    }));
  },

  removeProvider: (id) => {
    set((s) => {
      const scenarioModels = { ...s.scenarioModels };
      (["text", "image", "video"] as ScenarioKind[]).forEach((k) => {
        if (scenarioModels[k].providerId === id) {
          scenarioModels[k] = { providerId: null, model: "" };
        }
      });
      return {
        providers: s.providers.filter((p) => p.id !== id),
        scenarioModels,
      };
    });
  },

  toggleProviderEnabled: (id) => {
    set((s) => ({
      providers: s.providers.map((p) => (p.id === id ? { ...p, enabled: !(p.enabled !== false) } : p)),
    }));
  },

  updateProviderModels: (id, models) => {
    set((s) => ({
      providers: s.providers.map((p) =>
        p.id === id
          ? {
              ...p,
              models: models.map((model) => ({
                ...model,
                scenarios: model.scenarios ?? [],
              })),
            }
          : p,
      ),
    }));
  },

  addModelToProvider: (providerId, model) => {
    set((s) => ({
      providers: s.providers.map((p) => {
        if (p.id !== providerId) return p;
        const existing = p.models ?? [];
        if (existing.some((m) => m.id === model.id)) {
          return {
            ...p,
            models: existing.map((m) =>
              m.id === model.id
                ? { ...m, ...model, scenarios: model.scenarios ?? m.scenarios ?? [] }
                : m,
            ),
          };
        }
        return { ...p, models: [...existing, { ...model, scenarios: model.scenarios ?? [] }] };
      }),
    }));
  },

  setModelScenarios: (providerId, modelId, scenarios) => {
    const normalized = normalizeModelScenarios(scenarios);
    set((s) => ({
      providers: s.providers.map((p) =>
        p.id !== providerId
          ? p
          : {
              ...p,
              models: (p.models ?? []).map((m) =>
                m.id === modelId ? { ...m, scenarios: normalized } : m,
              ),
            },
      ),
    }));
  },

  toggleModelEnabled: (providerId, modelId) => {
    set((s) => ({
      providers: s.providers.map((p) => {
        if (p.id !== providerId) return p;
        return {
          ...p,
          models: (p.models ?? []).map((m) => (m.id === modelId ? { ...m, enabled: !m.enabled } : m)),
        };
      }),
    }));
  },

  removeModelFromProvider: (providerId, modelId) => {
    set((s) => ({
      providers: s.providers.map((p) => {
        if (p.id !== providerId) return p;
        return {
          ...p,
          models: (p.models ?? []).filter((m) => m.id !== modelId),
        };
      }),
    }));
  },

  setScenarioBinding: (scenario, binding) => {
    set((s) => ({
      scenarioModels: { ...s.scenarioModels, [scenario]: binding },
    }));
  },

  setWorkspaceDir: (kind, dir) => {
    set((s) => ({
      workspace: { ...s.workspace, [kind]: dir },
    }));
  },
}));

useCanvasSettingsStore.subscribe((state) => {
  try {
    localStorage.setItem(STORAGE_KEY, serialize(state));
  } catch {
    // storage full or unavailable
  }
});
