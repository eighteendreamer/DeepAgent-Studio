import { useThemeContext } from "../theme/ThemeProvider";
import { fontStack } from "../theme/themeFonts";
import type {
  ThemeMode as NewThemeMode,
  ThemePalette,
  ThemePreferences,
  ThemeSwitchOrigin as NewThemeSwitchOrigin,
  ThemeVariant,
} from "../theme/themeTypes";

export type ThemeMode = NewThemeMode;
export type ThemeSwitchOrigin = NewThemeSwitchOrigin;

export interface ThemeConfigDetails {
  accent: string;
  bg: string;
  fg: string;
  uiFont: ThemePreferences["uiFont"];
  codeFont: ThemePreferences["codeFont"];
  translucentSidebar: boolean;
  contrast: number;
  cssVariables: Record<string, string>;
}

export interface ThemeConfig {
  mode: ThemeMode;
  light: ThemeConfigDetails;
  dark: ThemeConfigDetails;
}

function paletteToLegacy(
  palette: ThemePalette,
  preferences: Pick<ThemePreferences, "uiFont" | "codeFont" | "translucentSidebar">,
): ThemeConfigDetails {
  return {
    accent: palette.accent,
    bg: palette.background,
    fg: palette.foreground,
    uiFont: preferences.uiFont,
    codeFont: preferences.codeFont,
    translucentSidebar: preferences.translucentSidebar,
    contrast: palette.contrast,
    cssVariables: {
      "--theme-accent": palette.accent,
      "--theme-accent-hover": palette.accentHover,
      "--theme-bg": palette.background,
      "--theme-sidebar": palette.sidebar,
      "--theme-fg": palette.foreground,
      "--theme-text-secondary": palette.foregroundMuted,
      "--theme-border": palette.border,
      "--ui-font": fontStack("ui", preferences.uiFont),
      "--code-font": fontStack("code", preferences.codeFont),
    },
  };
}

export function useTheme() {
  const ctx = useThemeContext();
  const config: ThemeConfig = {
    mode: ctx.state.mode,
    light: paletteToLegacy(ctx.state.workingPalettes.light, ctx.state.preferences),
    dark: paletteToLegacy(ctx.state.workingPalettes.dark, ctx.state.preferences),
  };

  const switchTheme = (mode: ThemeMode, origin: ThemeSwitchOrigin) => {
    ctx.setMode(mode, origin);
  };

  const updateThemeDetails = (
    isDarkTheme: boolean,
    updates: Partial<Omit<ThemeConfigDetails, "cssVariables">>,
  ) => {
    const variant: ThemeVariant = isDarkTheme ? "dark" : "light";
    const palettePatch: Partial<ThemePalette> = {};
    if (updates.accent !== undefined) {
      palettePatch.accent = updates.accent;
      palettePatch.accentHover = updates.accent;
    }
    if (updates.bg !== undefined) palettePatch.background = updates.bg;
    if (updates.fg !== undefined) palettePatch.foreground = updates.fg;
    if (updates.contrast !== undefined) palettePatch.contrast = updates.contrast;
    if (Object.keys(palettePatch).length > 0) {
      ctx.updateWorkingPalette(variant, palettePatch);
    }
    const prefPatch: Partial<ThemePreferences> = {};
    if (updates.uiFont !== undefined) prefPatch.uiFont = updates.uiFont;
    if (updates.codeFont !== undefined) prefPatch.codeFont = updates.codeFont;
    if (updates.translucentSidebar !== undefined) {
      prefPatch.translucentSidebar = updates.translucentSidebar;
    }
    if (Object.keys(prefPatch).length > 0) {
      ctx.updatePreferences(prefPatch);
    }
  };

  const updateConfig = (
    next: Partial<ThemeConfig> | ((prev: ThemeConfig) => ThemeConfig),
  ) => {
    const merged = typeof next === "function" ? next(config) : { ...config, ...next };
    if (merged.mode !== config.mode) {
      ctx.setMode(merged.mode);
    }
  };

  return {
    config,
    activeIsDark: ctx.activeIsDark,
    updateConfig,
    updateThemeDetails,
    switchTheme,
  };
}
