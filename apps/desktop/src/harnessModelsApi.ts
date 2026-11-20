// Harness model registry: TypeScript mirror of the Rust DTOs in
// `crates/deepagent-app-core/src/harness_model_service.rs` and the Tauri
// commands registered in `apps/desktop/src-tauri/src/lib.rs`.
// Field names are camelCase on the wire (serde rename_all = "camelCase").
// Keep this file in sync with those DTOs; API keys never come back to the UI.

import { invoke } from "@tauri-apps/api/core";

/** Wire protocols accepted by the backend (`HARNESS_PROTOCOLS`). */
export const HARNESS_PROTOCOLS = ["responses", "chat_completions", "messages"] as const;
export type HarnessProtocol = (typeof HARNESS_PROTOCOLS)[number];

export const HARNESS_THINKING_LEVELS = ["off", "low", "medium", "high", "max"] as const;
export type HarnessThinkingFormat = "effort_only" | "enabled" | "adaptive" | "budget";
export type HarnessThinkingLevel = (typeof HARNESS_THINKING_LEVELS)[number];
/** Omitted inherits the catalog; false removes controls; only off may omit its wire value. */
export type HarnessReasoningEfforts = false | Partial<Record<HarnessThinkingLevel, string | null>>;

/** Balance query kinds supported by the backend adapter. */
export const HARNESS_BALANCE_KINDS = ["auto", "none", "official", "new_api", "sub2api"] as const;
export type HarnessBalanceKind = (typeof HARNESS_BALANCE_KINDS)[number];

export interface HarnessProviderView {
  id: string;
  channel: string;
  logo: string;
  websiteUrl: string;
  modelUrl: string;
  protocol: HarnessProtocol;
  balanceKind: HarnessBalanceKind;
  enabled: boolean;
  hasApiKey: boolean;
  revision: number;
}

export interface HarnessProviderInput {
  /** `null` creates a new provider; an existing id updates it. */
  id: string | null;
  channel: string;
  logo: string;
  websiteUrl: string;
  modelUrl: string;
  protocol: HarnessProtocol;
  balanceKind: HarnessBalanceKind;
  enabled: boolean;
  /** `null` keeps the stored key; a non-empty string replaces it. */
  apiKey: string | null;
}

export interface HarnessModelView {
  providerId: string;
  modelName: string;
  displayName: string;
  /** `null` inherits the provider protocol. */
  protocol: HarnessProtocol | null;
  contextWindow: number;
  outputLimit: number;
  active: boolean;
  thinkingLevels: string[];
  thinkingDistinct: string[];
  thinkingDisable: boolean;
  reasoningEfforts?: HarnessReasoningEfforts;
  effectiveReasoningEfforts: HarnessReasoningEfforts;
  thinkingFormat?: HarnessThinkingFormat | null;
  effectiveThinkingFormat: HarnessThinkingFormat;
  defaultReasoningEffort?: HarnessThinkingLevel | null;
  effectiveDefaultReasoningEffort?: HarnessThinkingLevel | null;
}

export type HarnessModelInput = Omit<HarnessModelView,
  "active" | "thinkingLevels" | "thinkingDistinct" | "thinkingDisable" | "effectiveReasoningEfforts" | "effectiveDefaultReasoningEffort" | "effectiveThinkingFormat">;

export interface HarnessActiveModelView {
  providerId: string;
  providerChannel: string;
  providerLogo: string;
  modelUrl: string;
  modelName: string;
  displayName: string;
  protocol: HarnessProtocol;
  contextWindow: number;
  outputLimit: number;
  thinkingLevels: string[];
  thinkingDistinct: string[];
  thinkingDisable: boolean;
  /** Explicit selection; null resolves to an explicit application level. */
  reasoningEffort: string | null;
  defaultReasoningEffort: string | null;
}

export interface HarnessBalanceView {
  providerId: string;
  balanceKind: HarnessBalanceKind;
  amount: {
    amount: number | null;
    unit: string;
    unlimited: boolean;
  };
  available: boolean | null;
}

export function listHarnessProviders(): Promise<HarnessProviderView[]> {
  return invoke("harness_providers_list");
}

export function connectHarnessProvider(providerId: string, apiKey: string): Promise<void> {
  return invoke("harness_provider_connect", { providerId, apiKey });
}

export function saveHarnessProvider(provider: HarnessProviderInput): Promise<HarnessProviderView> {
  return invoke("harness_provider_save", { provider });
}

export function removeHarnessProvider(providerId: string): Promise<boolean> {
  return invoke("harness_provider_remove", { providerId });
}

export function listHarnessModels(providerId: string): Promise<HarnessModelView[]> {
  return invoke("harness_models_list", { providerId });
}

export async function saveHarnessModel(model: HarnessModelInput): Promise<HarnessModelView> {
  const saved = await invoke<HarnessModelView>("harness_model_save", { model });
  window.dispatchEvent(new CustomEvent("deepagent:settings-changed", {
    detail: { reason: "model_capabilities", affectsBalance: false },
  }));
  return saved;
}

/** Pulls `GET /models` with the stored key. Newly found models start inactive. */
export function fetchHarnessModels(providerId: string): Promise<HarnessModelView[]> {
  return invoke("harness_models_fetch", { providerId });
}

export function setHarnessModelActive(
  providerId: string,
  modelName: string,
  active: boolean,
): Promise<boolean> {
  return invoke("harness_model_set_active", { providerId, modelName, active });
}

export function listActiveHarnessModels(): Promise<HarnessActiveModelView[]> {
  return invoke("harness_active_models");
}

export function selectHarnessModel(
  providerId: string,
  modelName: string,
  affectsBalance = false,
  reasoningEffort: string | null = null,
): Promise<HarnessActiveModelView> {
  const request = invoke<HarnessActiveModelView>("harness_model_select", { providerId, modelName, reasoningEffort });
  return request.then((view) => {
    window.dispatchEvent(new CustomEvent("deepagent:settings-changed", {
      detail: { reason: "chat_model", affectsBalance },
    }));
    return view;
  });
}

export function getHarnessModelSelection(): Promise<HarnessActiveModelView | null> {
  return invoke("harness_model_selection");
}

export function fetchHarnessBalance(providerId: string): Promise<HarnessBalanceView> {
  return invoke("harness_balance", { providerId });
}

export async function fetchSelectedHarnessBalance(): Promise<HarnessBalanceView | null> {
  const selected = await getHarnessModelSelection();
  return selected ? fetchHarnessBalance(selected.providerId) : null;
}
