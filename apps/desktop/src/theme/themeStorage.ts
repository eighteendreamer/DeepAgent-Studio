import {
  BUILTIN_PRESETS,
  DEFAULT_DARK_PRESET_ID,
  DEFAULT_LIGHT_PRESET_ID,
} from "./themePresets";
import type {
  PersistedThemeStateV2,
  ThemePalette,
  ThemePreferences,
  ThemeVariant,
} from "./themeTypes";
import {
  DEFAULT_CODE_FONT,
  DEFAULT_UI_FONT,
  LEGACY_THEME_STORAGE_KEY,
  THEME_STORAGE_KEY,
} from "./themeTypes";

function defaultPalette(variant: ThemeVariant): ThemePalette {
  const preset = BUILTIN_PRESETS.find((p) => p.id === "codex")!;
  return { ...preset.variants[variant]! };
}

function defaultPreferences(): ThemePreferences {
  return {
    uiFont: DEFAULT_UI_FONT,
    codeFont: DEFAULT_CODE_FONT,
    translucentSidebar: true,
  };
}

export function defaultV2State(): PersistedThemeStateV2 {
  return {
    schemaVersion: 2,
    mode: "system",
    selectedPresetIds: {
      light: DEFAULT_LIGHT_PRESET_ID,
      dark: DEFAULT_DARK_PRESET_ID,
    },
    workingPalettes: {
      light: defaultPalette("light"),
      dark: defaultPalette("dark"),
    },
    preferences: defaultPreferences(),
    customPresets: [],
    revision: 0,
  };
}

function isValidHex(v: unknown): boolean {
  return typeof v === "string" && /^#[0-9A-Fa-f]{3,8}$/.test(v);
}

function isValidPalette(p: unknown): p is ThemePalette {
  if (!p || typeof p !== "object") return false;
  const o = p as Record<string, unknown>;
  return (
    isValidHex(o.accent) &&
    isValidHex(o.accentHover) &&
    isValidHex(o.background) &&
    isValidHex(o.sidebar) &&
    isValidHex(o.elevated) &&
    isValidHex(o.foreground) &&
    isValidHex(o.foregroundMuted) &&
    isValidHex(o.border) &&
    isValidHex(o.hover) &&
    isValidHex(o.selection) &&
    typeof o.contrast === "number"
  );
}

function isValidV2(raw: unknown): raw is PersistedThemeStateV2 {
  if (!raw || typeof raw !== "object") return false;
  const o = raw as Record<string, unknown>;
  return (
    o.schemaVersion === 2 &&
    (o.mode === "light" || o.mode === "dark" || o.mode === "system") &&
    isValidPalette((o.workingPalettes as any)?.light) &&
    isValidPalette((o.workingPalettes as any)?.dark)
  );
}

function migrateFromLegacy(): PersistedThemeStateV2 | null {
  try {
    const raw = localStorage.getItem(LEGACY_THEME_STORAGE_KEY);
    if (!raw) return null;
    const old = JSON.parse(raw);
    const state = defaultV2State();
    if (old.mode === "light" || old.mode === "dark" || old.mode === "system") {
      state.mode = old.mode;
    }
    if (old.light?.uiFont) state.preferences.uiFont = old.light.uiFont;
    if (old.light?.codeFont) state.preferences.codeFont = old.light.codeFont;
    if (typeof old.light?.translucentSidebar === "boolean") {
      state.preferences.translucentSidebar = old.light.translucentSidebar;
    }
    if (isValidHex(old.light?.accent)) {
      state.workingPalettes.light.accent = old.light.accent;
      state.workingPalettes.light.accentHover = old.light.accent;
      state.selectedPresetIds.light = "custom-migrated-light";
    }
    if (isValidHex(old.dark?.accent)) {
      state.workingPalettes.dark.accent = old.dark.accent;
      state.workingPalettes.dark.accentHover = old.dark.accent;
      state.selectedPresetIds.dark = "custom-migrated-dark";
    }
    if (isValidHex(old.light?.bg)) {
      state.workingPalettes.light.background = old.light.bg;
    }
    if (isValidHex(old.light?.fg)) {
      state.workingPalettes.light.foreground = old.light.fg;
    }
    if (isValidHex(old.dark?.bg)) {
      state.workingPalettes.dark.background = old.dark.bg;
    }
    if (isValidHex(old.dark?.fg)) {
      state.workingPalettes.dark.foreground = old.dark.fg;
    }
    return state;
  } catch {
    return null;
  }
}

export function migrateThemeState(input: unknown): PersistedThemeStateV2 {
  if (isValidV2(input)) {
    const state = input as PersistedThemeStateV2;
    if (!isValidPalette(state.workingPalettes.light)) {
      state.workingPalettes.light = defaultPalette("light");
    }
    if (!isValidPalette(state.workingPalettes.dark)) {
      state.workingPalettes.dark = defaultPalette("dark");
    }
    return state;
  }
  const migrated = migrateFromLegacy();
  if (migrated) return migrated;
  return defaultV2State();
}

export function loadThemeState(): PersistedThemeStateV2 {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (raw) {
      return migrateThemeState(JSON.parse(raw));
    }
  } catch {
    // fall through
  }
  return migrateThemeState(null);
}

export function saveThemeState(state: PersistedThemeStateV2): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, JSON.stringify(state));
  } catch {
    // storage full or unavailable
  }
}
