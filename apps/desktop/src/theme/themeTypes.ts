export type ThemeMode = "light" | "dark" | "system";
export type ThemeVariant = "light" | "dark";
export type ThemePresetSource = "builtin" | "custom" | "imported";

export interface ThemeSwitchOrigin {
  x: number;
  y: number;
}

export interface ThemePalette {
  accent: string;
  accentHover: string;
  background: string;
  sidebar: string;
  elevated: string;
  foreground: string;
  foregroundMuted: string;
  border: string;
  hover: string;
  selection: string;
  contrast: number;
}

export interface ThemePreset {
  id: string;
  name: string;
  source: ThemePresetSource;
  variants: Partial<Record<ThemeVariant, ThemePalette>>;
  createdAt?: string;
  updatedAt?: string;
}

export interface ThemePreferences {
  uiFont: string;
  codeFont: string;
  translucentSidebar: boolean;
}

export interface PersistedThemeStateV2 {
  schemaVersion: 2;
  mode: ThemeMode;
  selectedPresetIds: Record<ThemeVariant, string>;
  workingPalettes: Record<ThemeVariant, ThemePalette>;
  preferences: ThemePreferences;
  customPresets: ThemePreset[];
  revision: number;
}

export interface ThemeImportResult {
  ok: boolean;
  presetId?: string;
  variant?: ThemeVariant;
  error?: string;
}

export const DEFAULT_UI_FONT =
  'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto';
export const DEFAULT_CODE_FONT =
  'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas';

export const THEME_STORAGE_KEY = "deepagent:theme:v2";
export const LEGACY_THEME_STORAGE_KEY = "codex-theme-config";
export const THEME_BROADCAST_CHANNEL = "deepagent-theme";
