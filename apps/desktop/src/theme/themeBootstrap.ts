import { loadThemeState } from "./themeStorage";
import { applyTokens, paletteToTokens } from "./themeTokens";
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
    if (state.preferences.translucentSidebar) {
      document.documentElement.classList.add("theme-translucent-sidebar");
    }
  } catch {
    // startup must not fail; leave defaults
  }
}
