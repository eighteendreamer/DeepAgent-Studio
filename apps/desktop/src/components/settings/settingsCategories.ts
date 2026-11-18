import { lazy, memo, type ComponentType } from "react";

const loaders = {
  general: () => import("./GeneralSettings").then(module => ({ default: module.GeneralSettings })),
  appearance: () => import("./AppearanceSettings").then(module => ({ default: module.AppearanceSettings })),
  pet: () => import("./PetSettings").then(module => ({ default: module.PetSettings })),
  config: () => import("./ConfigSettings").then(module => ({ default: module.ConfigSettings })),
  models: () => import("./ModelsSettings").then(module => ({ default: module.ModelsSettings })),
  personalize: () => import("./PersonalizeSettings").then(module => ({ default: module.PersonalizeSettings })),
  shortcuts: () => import("./ShortcutsSettings").then(module => ({ default: module.ShortcutsSettings })),
  mcp: () => import("./MCPSettings").then(module => ({ default: module.MCPSettings })),
  hooks: () => import("./HooksSettings").then(module => ({ default: module.HooksSettings })),
  connections: () => import("./ConnectionsSettings").then(module => ({ default: module.ConnectionsSettings })),
  git: () => import("./GitSettings").then(module => ({ default: module.GitSettings })),
  env: () => import("./EnvSettings").then(module => ({ default: module.EnvSettings })),
  worktree: () => import("./WorktreeSettings").then(module => ({ default: module.WorktreeSettings })),
  browser: () => import("./BrowserSettings").then(module => ({ default: module.BrowserSettings })),
  computer: () => import("./ComputerSettings").then(module => ({ default: module.ComputerSettings })),
  project_map_debug: () => import("./ProjectMapDebugSettings").then(module => ({ default: module.ProjectMapDebugSettings })),
  archive: () => import("./ArchiveSettings").then(module => ({ default: module.ArchiveSettings })),
  about: () => import("./AboutSettings").then(module => ({ default: module.AboutSettings })),
};

export type SettingsCategory = keyof typeof loaders;
type CategoryModule = Awaited<ReturnType<(typeof loaders)[SettingsCategory]>>;
const pending = new Map<SettingsCategory, Promise<CategoryModule>>();

export function settingsCategory(id: string): SettingsCategory {
  return Object.prototype.hasOwnProperty.call(loaders, id) ? id as SettingsCategory : "general";
}

export function preloadSettingsCategory(id: string) {
  const category = settingsCategory(id);
  let request = pending.get(category);
  if (!request) {
    request = loaders[category]();
    pending.set(category, request);
    request.catch(() => pending.delete(category));
  }
  return request;
}

export const settingsCategories: Record<string, ComponentType> = Object.fromEntries(Object.keys(loaders).map(id => [
  id, memo(lazy(() => preloadSettingsCategory(id))),
]));
