import { mixHex } from "./colorUtils";
import type { ThemePalette } from "./themeTypes";

// The contrast slider (0-100) reshapes derived tokens around a neutral of 50.
// Below 50 we pull derived colors toward the background (softer separation).
// Above 50 we push them toward the foreground (crisper separation).
function contrastBlend(
  base: string,
  towardFg: string,
  towardBg: string,
  contrast: number,
  fgWeight: number,
  bgWeight: number,
): string {
  const c = Math.max(0, Math.min(100, contrast));
  const delta = (c - 50) / 50;
  if (delta === 0) return base;
  if (delta > 0) {
    return mixHex(base, towardFg, delta * fgWeight);
  }
  return mixHex(base, towardBg, -delta * bgWeight);
}

export function paletteToTokens(palette: ThemePalette): Record<string, string> {
  const { background, foreground, contrast } = palette;
  const foregroundMuted = contrastBlend(
    palette.foregroundMuted,
    foreground,
    background,
    contrast,
    0.45,
    0.35,
  );
  const border = contrastBlend(
    palette.border,
    foreground,
    background,
    contrast,
    0.35,
    0.45,
  );
  const hover = contrastBlend(
    palette.hover,
    foreground,
    background,
    contrast,
    0.25,
    0.45,
  );
  const selection = contrastBlend(
    palette.selection,
    palette.accent,
    background,
    contrast,
    0.2,
    0.35,
  );
  const sidebar = contrastBlend(
    palette.sidebar,
    foreground,
    background,
    contrast,
    0.12,
    0.35,
  );
  return {
    "--theme-accent": palette.accent,
    "--theme-accent-hover": palette.accentHover,
    "--theme-bg": palette.background,
    "--theme-sidebar": sidebar,
    "--theme-elevated": palette.elevated,
    "--theme-fg": palette.foreground,
    "--theme-text-secondary": foregroundMuted,
    "--theme-border": border,
    "--theme-hover": hover,
    "--theme-selection": selection,
  };
}

export function applyTokens(
  tokens: Record<string, string>,
  fontTokens: { uiFont: string; codeFont: string },
  isDark: boolean
): void {
  const root = document.documentElement;
  for (const [k, v] of Object.entries(tokens)) {
    root.style.setProperty(k, v);
  }
  root.style.setProperty("--ui-font", fontTokens.uiFont);
  root.style.setProperty("--code-font", fontTokens.codeFont);
  if (isDark) {
    root.classList.add("dark");
  } else {
    root.classList.remove("dark");
  }
}
