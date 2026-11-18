import { loadThemeState } from "./themeStorage";
import { applyTokens, paletteToTokens } from "./themeTokens";
import { applyThemeBackground } from "./themeBackground";
import type { ThemeMode, ThemeVariant } from "./themeTypes";

export function resolveVariant(mode: ThemeMode): ThemeVariant {
  if (mode === "light") return "light";
  if (mode === "dark") return "dark";
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

export function bootstrapTheme(): void {
  try {
    const state = loadThemeState();
    const variant = resolveVariant(state.mode);
    const palette = state.workingPalettes[variant];
    applyTokens(
      paletteToTokens(palette),
      { uiFont: state.preferences.uiFont, codeFont: state.preferences.codeFont },
      variant === "dark",
    );
    applyThemeBackground(state.preferences);
  } catch {
    // startup must not fail; leave defaults
  }
}
