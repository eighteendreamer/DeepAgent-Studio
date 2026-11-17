// Harness model registry: TypeScript mirror of the Rust DTOs in
// `crates/deepagent-app-core/src/harness_model_service.rs` and the Tauri
// commands registered in `apps/desktop/src-tauri/src/lib.rs`.
// Field names are camelCase on the wire (serde rename_all = "camelCase").
// Keep this file in sync with those DTOs; API keys never come back to the UI.

import { invoke } from "@tauri-apps/api/core";

/** Wire protocols accepted by the backend (`HARNESS_PROTOCOLS`). */
export const HARNESS_PROTOCOLS = ["responses", "chat_completions", "messages"] as const;
export type HarnessProtocol = (typeof HARNESS_PROTOCOLS)[number];

/** Balance query kinds. Dispatch is not implemented yet (plan P7); stored only. */
export const HARNESS_BALANCE_KINDS = ["auto", "none", "official", "new_api", "sub2api"] as const;
export type HarnessBalanceKind = (typeof HARNESS_BALANCE_KINDS)[number];

export interface HarnessProviderView {
  id: string;
  channel: string;
  logo: string;
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
}

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
}

export function listHarnessProviders(): Promise<HarnessProviderView[]> {
  return invoke("harness_providers_list");
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
