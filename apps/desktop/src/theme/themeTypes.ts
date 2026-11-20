import type { FontId } from "./themeFonts";

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

export interface ThemeBackgroundImage {
  dataUrl: string;
  name: string;
}

export interface ThemePreferences {
  uiFont: FontId<"ui">;
  codeFont: FontId<"code">;
  translucentSidebar: boolean;
  backgroundImage: ThemeBackgroundImage | null;
  /** Interface background transparency: 0 = solid, 100 = clear; wallpaper stays opaque. */
  backgroundTransparency: number;
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

export const THEME_STORAGE_KEY = "deepagent:theme:v2";
export const LEGACY_THEME_STORAGE_KEY = "codex-theme-config";
export const THEME_BROADCAST_CHANNEL = "deepagent-theme";
