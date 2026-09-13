import { create } from "zustand";

const STORAGE_KEY = "workflow-settings";

export type ModelProtocol = "openai" | "anthropic" | "gemini";

export interface ModelProvider {
  id: string;
  name: string;
  protocol: ModelProtocol;
  baseUrl: string;
  apiKey: string;
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

function loadPersisted(): Pick<WorkflowSettingsState, "providers" | "scenarioModels" | "workspace"> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { providers: [], scenarioModels: defaultBindings(), workspace: { imageDir: "", videoDir: "" } };
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return { providers: [], scenarioModels: defaultBindings(), workspace: { imageDir: "", videoDir: "" } };

    const providers =
      Array.isArray(parsed.providers) && parsed.providers.every(isValidProvider)
        ? parsed.providers.map((provider: ModelProvider) => ({
            ...provider,
            protocol: normalizeProtocol(provider.protocol),
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
    set((s) => ({ providers: [...s.providers, { id, ...p }] }));
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
