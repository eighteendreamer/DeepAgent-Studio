import type { ThemePalette, ThemePreset, ThemeVariant } from "./themeTypes";

function palette(p: ThemePalette): ThemePalette {
  return Object.freeze({ ...p });
}

function preset(id: string, name: string, variants: Partial<Record<ThemeVariant, ThemePalette>>): ThemePreset {
  return Object.freeze({
    id,
    name,
    source: "builtin",
    variants: Object.freeze({ ...variants }) as Partial<Record<ThemeVariant, ThemePalette>>,
  }) as ThemePreset;
}

const CODEX_LIGHT = palette({
  accent: "#339CFF",
  accentHover: "#2563EB",
  background: "#FFFFFF",
  sidebar: "#F9F8F6",
  elevated: "#FFFFFF",
  foreground: "#1F2937",
  foregroundMuted: "#6B7280",
  border: "#E5E7EB",
  hover: "#F3F4F6",
  selection: "#DBEAFE",
  contrast: 45,
});

const CODEX_DARK = palette({
  accent: "#339CFF",
  accentHover: "#60A5FA",
  background: "#000000",
  sidebar: "#121212",
  elevated: "#1A1A1A",
  foreground: "#F9FAFB",
  foregroundMuted: "#9CA3AF",
  border: "#27272A",
  hover: "#1F1F23",
  selection: "#1E3A8A",
  contrast: 50,
});

const ABSOLUTELY_LIGHT = palette({
  accent: "#7C3AED",
  accentHover: "#6D28D9",
  background: "#FAFAFA",
  sidebar: "#F4F4F5",
  elevated: "#FFFFFF",
  foreground: "#18181B",
  foregroundMuted: "#71717A",
  border: "#E4E4E7",
  hover: "#F4F4F5",
  selection: "#EDE9FE",
  contrast: 45,
});

const ABSOLUTELY_DARK = palette({
  accent: "#A78BFA",
  accentHover: "#C4B5FD",
  background: "#09090B",
  sidebar: "#18181B",
  elevated: "#27272A",
  foreground: "#FAFAFA",
  foregroundMuted: "#A1A1AA",
  border: "#3F3F46",
  hover: "#27272A",
  selection: "#2E1065",
  contrast: 50,
});

const AYU_LIGHT = palette({
  accent: "#FF9940",
  accentHover: "#F08030",
  background: "#FAFAFA",
  sidebar: "#F3F4F5",
  elevated: "#FFFFFF",
  foreground: "#1A1A1A",
  foregroundMuted: "#8A9199",
  border: "#E7E8E9",
  hover: "#EEF0F2",
  selection: "#FFEACC",
  contrast: 45,
});

const AYU_DARK = palette({
  accent: "#FF9940",
  accentHover: "#FFB366",
  background: "#1F2430",
  sidebar: "#191E2A",
  elevated: "#2A3040",
  foreground: "#CBCCC6",
  foregroundMuted: "#707A8C",
  border: "#2D3347",
  hover: "#2A3040",
  selection: "#3D4A5C",
  contrast: 50,
});

const CATPPUCCIN_LIGHT = palette({
  accent: "#8839EF",
  accentHover: "#7527D7",
  background: "#EFF1F5",
  sidebar: "#E6E9EF",
  elevated: "#FFFFFF",
  foreground: "#4C4F69",
  foregroundMuted: "#9CA0B0",
  border: "#CCD0DA",
  hover: "#DCE0E8",
  selection: "#E0D9F5",
  contrast: 45,
});

const CATPPUCCIN_DARK = palette({
  accent: "#CBA6F7",
  accentHover: "#D8B4FE",
  background: "#1E1E2E",
  sidebar: "#181825",
  elevated: "#313244",
  foreground: "#CDD6F4",
  foregroundMuted: "#6C7086",
  border: "#45475A",
  hover: "#313244",
  selection: "#45475A",
  contrast: 50,
});

const DRACULA_DARK = palette({
  accent: "#BD93F9",
  accentHover: "#CFA9FF",
  background: "#282A36",
  sidebar: "#21222C",
  elevated: "#343746",
  foreground: "#F8F8F2",
  foregroundMuted: "#6272A4",
  border: "#44475A",
  hover: "#44475A",
  selection: "#44475A",
  contrast: 50,
});

const EVERFOREST_LIGHT = palette({
  accent: "#8DA101",
  accentHover: "#7A8E00",
  background: "#FDF6E3",
  sidebar: "#F4EDD3",
  elevated: "#FFFFFF",
  foreground: "#5C6A72",
  foregroundMuted: "#939F91",
  border: "#E0DCC7",
  hover: "#EDE8D4",
  selection: "#D5E8C8",
  contrast: 45,
});

const EVERFOREST_DARK = palette({
  accent: "#A7C080",
  accentHover: "#B8D094",
  background: "#2D353B",
  sidebar: "#272E33",
  elevated: "#343F44",
  foreground: "#D3C6AA",
  foregroundMuted: "#7A8478",
  border: "#414B50",
  hover: "#3D484D",
  selection: "#3D484D",
  contrast: 50,
});

const GITHUB_LIGHT = palette({
  accent: "#0969DA",
  accentHover: "#0550AE",
  background: "#FFFFFF",
  sidebar: "#F6F8FA",
  elevated: "#FFFFFF",
  foreground: "#1F2328",
  foregroundMuted: "#656D76",
  border: "#D0D7DE",
  hover: "#F3F4F6",
  selection: "#DDF4FF",
  contrast: 45,
});

const GITHUB_DARK = palette({
  accent: "#58A6FF",
  accentHover: "#79B8FF",
  background: "#0D1117",
  sidebar: "#161B22",
  elevated: "#21262D",
  foreground: "#E6EDF3",
  foregroundMuted: "#8B949E",
  border: "#30363D",
  hover: "#21262D",
  selection: "#1F3A5F",
  contrast: 50,
});

const GRUVBOX_LIGHT = palette({
  accent: "#D65D0E",
  accentHover: "#AF3A03",
  background: "#FBF1C7",
  sidebar: "#EBDBB2",
  elevated: "#FFFFFF",
  foreground: "#3C3836",
  foregroundMuted: "#928374",
  border: "#D5C4A1",
  hover: "#EBDBB2",
  selection: "#FBD796",
  contrast: 45,
});

const GRUVBOX_DARK = palette({
  accent: "#FE8019",
  accentHover: "#FABD2F",
  background: "#282828",
  sidebar: "#1D2021",
  elevated: "#3C3836",
  foreground: "#EBDBB2",
  foregroundMuted: "#928374",
  border: "#504945",
  hover: "#3C3836",
  selection: "#504945",
  contrast: 50,
});

const LINEAR_LIGHT = palette({
  accent: "#5E6AD2",
  accentHover: "#4A55C0",
  background: "#FFFFFF",
  sidebar: "#F7F8F8",
  elevated: "#FFFFFF",
  foreground: "#1A1A1A",
  foregroundMuted: "#8A8F98",
  border: "#E5E5E5",
  hover: "#F2F2F2",
  selection: "#EAEBF8",
  contrast: 45,
});

const LINEAR_DARK = palette({
  accent: "#5E6AD2",
  accentHover: "#7B84E0",
  background: "#0F0F0F",
  sidebar: "#1A1A1A",
  elevated: "#252525",
  foreground: "#F2F2F2",
  foregroundMuted: "#6B6F76",
  border: "#2E2E2E",
  hover: "#252525",
  selection: "#2A2D5A",
  contrast: 50,
});

const NOTION_LIGHT = palette({
  accent: "#2EAADC",
  accentHover: "#1A8FBF",
  background: "#FFFFFF",
  sidebar: "#F7F6F3",
  elevated: "#FFFFFF",
  foreground: "#37352F",
  foregroundMuted: "#9B9A97",
  border: "#E9E9E7",
  hover: "#F1F1EF",
  selection: "#D3EAF5",
  contrast: 45,
});

const NOTION_DARK = palette({
  accent: "#2EAADC",
  accentHover: "#4BBDE8",
  background: "#191919",
  sidebar: "#202020",
  elevated: "#2F2F2F",
  foreground: "#CFCFCF",
  foregroundMuted: "#787774",
  border: "#373737",
  hover: "#2F2F2F",
  selection: "#1A3A4A",
  contrast: 50,
});

const ONE_LIGHT = palette({
  accent: "#4078F2",
  accentHover: "#2D65E0",
  background: "#FAFAFA",
  sidebar: "#F0F0F0",
  elevated: "#FFFFFF",
  foreground: "#383A42",
  foregroundMuted: "#A0A1A7",
  border: "#D3D3D3",
  hover: "#EAEAEA",
  selection: "#D7E5FF",
  contrast: 45,
});

const ONE_DARK = palette({
  accent: "#61AFEF",
  accentHover: "#7BBFF5",
  background: "#282C34",
  sidebar: "#21252B",
  elevated: "#2C313A",
  foreground: "#ABB2BF",
  foregroundMuted: "#5C6370",
  border: "#3E4451",
  hover: "#2C313A",
  selection: "#3E4451",
  contrast: 50,
});

export const BUILTIN_PRESETS: readonly ThemePreset[] = Object.freeze([
  preset("codex", "Codex", { light: CODEX_LIGHT, dark: CODEX_DARK }),
  preset("absolutely", "Absolutely", { light: ABSOLUTELY_LIGHT, dark: ABSOLUTELY_DARK }),
  preset("ayu", "Ayu", { light: AYU_LIGHT, dark: AYU_DARK }),
  preset("catppuccin", "Catppuccin", { light: CATPPUCCIN_LIGHT, dark: CATPPUCCIN_DARK }),
  preset("dracula", "Dracula", { dark: DRACULA_DARK }),
  preset("everforest", "Everforest", { light: EVERFOREST_LIGHT, dark: EVERFOREST_DARK }),
  preset("github", "GitHub", { light: GITHUB_LIGHT, dark: GITHUB_DARK }),
  preset("gruvbox", "Gruvbox", { light: GRUVBOX_LIGHT, dark: GRUVBOX_DARK }),
  preset("linear", "Linear", { light: LINEAR_LIGHT, dark: LINEAR_DARK }),
  preset("notion", "Notion", { light: NOTION_LIGHT, dark: NOTION_DARK }),
  preset("one", "One", { light: ONE_LIGHT, dark: ONE_DARK }),
]);

export function getPresetById(id: string): ThemePreset | undefined {
  return BUILTIN_PRESETS.find((p) => p.id === id);
}

export const DEFAULT_LIGHT_PRESET_ID = "codex";
export const DEFAULT_DARK_PRESET_ID = "codex";
