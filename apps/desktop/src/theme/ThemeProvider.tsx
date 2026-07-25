import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { BUILTIN_PRESETS, getPresetById } from "./themePresets";
import { loadThemeState, saveThemeState } from "./themeStorage";
import { applyTokens, paletteToTokens } from "./themeTokens";
import { decodeThemeShare, encodeThemeShare } from "./themeShare";
import { setWindowTranslucent } from "../api";
import type {
  PersistedThemeStateV2,
  ThemeImportResult,
  ThemeMode,
  ThemePalette,
  ThemePreferences,
  ThemePreset,
  ThemeSwitchOrigin,
  ThemeVariant,
} from "./themeTypes";
import { THEME_BROADCAST_CHANNEL, THEME_STORAGE_KEY } from "./themeTypes";

interface ThemeContextValue {
  state: PersistedThemeStateV2;
  activeVariant: ThemeVariant;
  activeIsDark: boolean;
  activePalette: ThemePalette;
  availablePresets: ThemePreset[];
  currentPresetId: string;
  isCustom: boolean;
  setMode: (mode: ThemeMode, origin?: ThemeSwitchOrigin) => void;
  selectPreset: (variant: ThemeVariant, presetId: string) => void;
  updateWorkingPalette: (variant: ThemeVariant, patch: Partial<ThemePalette>) => void;
  saveWorkingPalette: (variant: ThemeVariant, name: string) => string;
  renameCustomPreset: (id: string, name: string) => void;
  deleteCustomPreset: (id: string) => void;
  updatePreferences: (patch: Partial<ThemePreferences>) => void;
  exportTheme: (variant: ThemeVariant) => string;
  importTheme: (value: string) => ThemeImportResult;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

const THEME_TRANSITION_DURATION_MS = 600;
const THEME_COLOR_FADE_DURATION_MS = 240;
let activeThemeRipple: Animation | null = null;
let activeThemeRippleElement: HTMLDivElement | null = null;
let activeThemeCommitTimer: number | null = null;
let activeThemeColorFadeTimer: number | null = null;

function playThemeRipple(
  origin: ThemeSwitchOrigin,
  color: string,
  onCovered: () => void,
) {
  activeThemeRipple?.cancel();
  activeThemeRippleElement?.remove();
  if (activeThemeCommitTimer !== null) {
    window.clearTimeout(activeThemeCommitTimer);
  }

  const viewportWidth = window.visualViewport?.width ?? window.innerWidth;
  const viewportHeight = window.visualViewport?.height ?? window.innerHeight;
  const x = Number.isFinite(origin.x) ? origin.x : viewportWidth / 2;
  const y = Number.isFinite(origin.y) ? origin.y : viewportHeight / 2;
  const radius =
    Math.hypot(Math.max(x, viewportWidth - x), Math.max(y, viewportHeight - y)) + 2;

  const ripple = document.createElement("div");
  ripple.className = "theme-switch-ripple";
  ripple.style.width = `${radius * 2}px`;
  ripple.style.height = `${radius * 2}px`;
  ripple.style.left = `${x - radius}px`;
  ripple.style.top = `${y - radius}px`;
  ripple.style.backgroundColor = color;
  document.body.appendChild(ripple);

  const animation = ripple.animate(
    [
      { transform: "scale(0)", opacity: 0.08, offset: 0 },
      { transform: "scale(1)", opacity: 0.18, offset: 0.82 },
      { transform: "scale(1)", opacity: 0, offset: 1 },
    ],
    {
      duration: THEME_TRANSITION_DURATION_MS,
      easing: "cubic-bezier(0.16, 1, 0.3, 1)",
      fill: "forwards",
    },
  );

  activeThemeRipple = animation;
  activeThemeRippleElement = ripple;
  activeThemeCommitTimer = window.setTimeout(() => {
    activeThemeCommitTimer = null;
    onCovered();
  }, THEME_TRANSITION_DURATION_MS * 0.82);

  animation.finished
    .catch(() => undefined)
    .finally(() => {
      if (activeThemeRipple === animation) {
        activeThemeRipple = null;
        activeThemeRippleElement = null;
      }
      ripple.remove();
    });
}

function beginColorFade() {
  const root = document.documentElement;
  if (activeThemeColorFadeTimer !== null) {
    window.clearTimeout(activeThemeColorFadeTimer);
  }
  root.classList.add("theme-color-transition");
  activeThemeColorFadeTimer = window.setTimeout(() => {
    root.classList.remove("theme-color-transition");
    activeThemeColorFadeTimer = null;
  }, THEME_COLOR_FADE_DURATION_MS);
}

function isCustomPresetId(id: string): boolean {
  return id.startsWith("custom-");
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<PersistedThemeStateV2>(() => loadThemeState());
  const stateRef = useRef(state);
  const [systemIsDark, setSystemIsDark] = useState<boolean>(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const channelRef = useRef<BroadcastChannel | null>(null);

  useEffect(() => {
    stateRef.current = state;
  }, [state]);

  const activeVariant: ThemeVariant = useMemo(() => {
    if (state.mode === "light") return "light";
    if (state.mode === "dark") return "dark";
    return systemIsDark ? "dark" : "light";
  }, [state.mode, systemIsDark]);

  const activePalette = state.workingPalettes[activeVariant];

  useEffect(() => {
    applyTokens(
      paletteToTokens(activePalette),
      { uiFont: state.preferences.uiFont, codeFont: state.preferences.codeFont },
      activeVariant === "dark",
    );
  }, [
    activePalette,
    activeVariant,
    state.preferences.uiFont,
    state.preferences.codeFont,
  ]);

  useEffect(() => {
    const root = document.documentElement;
    if (state.preferences.translucentSidebar) {
      root.classList.add("theme-translucent-sidebar");
    } else {
      root.classList.remove("theme-translucent-sidebar");
    }
    void setWindowTranslucent(state.preferences.translucentSidebar);
  }, [state.preferences.translucentSidebar]);

  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = (e: MediaQueryListEvent) => setSystemIsDark(e.matches);
    mq.addEventListener("change", handler);
    return () => mq.removeEventListener("change", handler);
  }, []);

  useEffect(() => {
    if (typeof BroadcastChannel === "undefined") return;
    const ch = new BroadcastChannel(THEME_BROADCAST_CHANNEL);
    channelRef.current = ch;
    ch.onmessage = (ev) => {
      const incoming = ev.data as PersistedThemeStateV2 | undefined;
      if (
        incoming &&
        incoming.schemaVersion === 2 &&
        (incoming.revision ?? 0) > (stateRef.current.revision ?? 0)
      ) {
        setState(incoming);
      }
    };
    return () => {
      ch.close();
      channelRef.current = null;
    };
  }, []);

  useEffect(() => {
    const handler = (e: StorageEvent) => {
      if (e.key !== THEME_STORAGE_KEY || !e.newValue) return;
      try {
        const next = JSON.parse(e.newValue) as PersistedThemeStateV2;
        if (
          next.schemaVersion === 2 &&
          (next.revision ?? 0) > (stateRef.current.revision ?? 0)
        ) {
          setState(next);
        }
      } catch {
        // ignore
      }
    };
    window.addEventListener("storage", handler);
    return () => window.removeEventListener("storage", handler);
  }, []);

  const commit = useCallback((next: PersistedThemeStateV2) => {
    const withRevision: PersistedThemeStateV2 = {
      ...next,
      revision: (stateRef.current.revision ?? 0) + 1,
    };
    stateRef.current = withRevision;
    setState(withRevision);
    saveThemeState(withRevision);
    channelRef.current?.postMessage(withRevision);
  }, []);

  const setMode = useCallback(
    (mode: ThemeMode, origin?: ThemeSwitchOrigin) => {
      const current = stateRef.current;
      if (current.mode === mode) return;
      const prevVariant: ThemeVariant =
        current.mode === "system"
          ? window.matchMedia("(prefers-color-scheme: dark)").matches
            ? "dark"
            : "light"
          : current.mode;
      const nextVariant: ThemeVariant =
        mode === "system"
          ? window.matchMedia("(prefers-color-scheme: dark)").matches
            ? "dark"
            : "light"
          : mode;
      const next = { ...current, mode };
      if (origin && prevVariant !== nextVariant) {
        const targetColor = current.workingPalettes[nextVariant].background;
        playThemeRipple(origin, targetColor, () => {
          beginColorFade();
          commit(next);
        });
      } else {
        commit(next);
      }
    },
    [commit],
  );

  const selectPreset = useCallback(
    (variant: ThemeVariant, presetId: string) => {
      const current = stateRef.current;
      const preset =
        getPresetById(presetId) ??
        current.customPresets.find((p) => p.id === presetId);
      if (!preset) return;
      const palette = preset.variants[variant];
      if (!palette) return;
      const next: PersistedThemeStateV2 = {
        ...current,
        selectedPresetIds: { ...current.selectedPresetIds, [variant]: presetId },
        workingPalettes: {
          ...current.workingPalettes,
          [variant]: { ...palette },
        },
      };
      // Only run the color fade when the visible variant is affected.
      const currentVariant: ThemeVariant =
        current.mode === "system"
          ? window.matchMedia("(prefers-color-scheme: dark)").matches
            ? "dark"
            : "light"
          : current.mode;
      if (currentVariant === variant) {
        beginColorFade();
      }
      commit(next);
    },
    [commit],
  );

  const updateWorkingPalette = useCallback(
    (variant: ThemeVariant, patch: Partial<ThemePalette>) => {
      const current = stateRef.current;
      const nextPalette = { ...current.workingPalettes[variant], ...patch };
      const currentId = current.selectedPresetIds[variant];
      const preset =
        getPresetById(currentId) ??
        current.customPresets.find((p) => p.id === currentId);
      const source = preset?.variants[variant];
      const isSame =
        source &&
        (Object.keys(nextPalette) as Array<keyof ThemePalette>).every(
          (k) => nextPalette[k] === source[k],
        );
      const nextId = isSame ? currentId : `custom-${variant}`;
      const next: PersistedThemeStateV2 = {
        ...current,
        workingPalettes: { ...current.workingPalettes, [variant]: nextPalette },
        selectedPresetIds: {
          ...current.selectedPresetIds,
          [variant]: nextId,
        },
      };
      commit(next);
    },
    [commit],
  );

  const saveWorkingPalette = useCallback(
    (variant: ThemeVariant, name: string): string => {
      const current = stateRef.current;
      const id = `custom-${variant}-${Date.now().toString(36)}`;
      const now = new Date().toISOString();
      const newPreset: ThemePreset = {
        id,
        name: name.trim() || `Custom ${variant}`,
        source: "custom",
        variants: { [variant]: { ...current.workingPalettes[variant] } },
        createdAt: now,
        updatedAt: now,
      };
      const next: PersistedThemeStateV2 = {
        ...current,
        customPresets: [...current.customPresets, newPreset],
        selectedPresetIds: { ...current.selectedPresetIds, [variant]: id },
      };
      commit(next);
      return id;
    },
    [commit],
  );

  const renameCustomPreset = useCallback(
    (id: string, name: string) => {
      const current = stateRef.current;
      const trimmed = name.trim();
      if (!trimmed) return;
      const next: PersistedThemeStateV2 = {
        ...current,
        customPresets: current.customPresets.map((p) =>
          p.id === id
            ? { ...p, name: trimmed, updatedAt: new Date().toISOString() }
            : p,
        ),
      };
      commit(next);
    },
    [commit],
  );

  const deleteCustomPreset = useCallback(
    (id: string) => {
      const current = stateRef.current;
      const next: PersistedThemeStateV2 = {
        ...current,
        customPresets: current.customPresets.filter((p) => p.id !== id),
      };
      // If the deleted preset was selected, fall back to codex default.
      (["light", "dark"] as ThemeVariant[]).forEach((v) => {
        if (next.selectedPresetIds[v] === id) {
          next.selectedPresetIds = { ...next.selectedPresetIds, [v]: "codex" };
          const codex = getPresetById("codex")!;
          next.workingPalettes = {
            ...next.workingPalettes,
            [v]: { ...codex.variants[v]! },
          };
        }
      });
      commit(next);
    },
    [commit],
  );

  const updatePreferences = useCallback(
    (patch: Partial<ThemePreferences>) => {
      const current = stateRef.current;
      const next: PersistedThemeStateV2 = {
        ...current,
        preferences: { ...current.preferences, ...patch },
      };
      commit(next);
    },
    [commit],
  );

  const exportTheme = useCallback((variant: ThemeVariant): string => {
    const current = stateRef.current;
    const palette = current.workingPalettes[variant];
    const selectedId = current.selectedPresetIds[variant];
    const presetName =
      getPresetById(selectedId)?.name ??
      current.customPresets.find((p) => p.id === selectedId)?.name ??
      (variant === "dark" ? "Dark Theme" : "Light Theme");
    return encodeThemeShare({
      schemaVersion: 1,
      name: presetName,
      variant,
      palette,
    });
  }, []);

  const importTheme = useCallback(
    (value: string): ThemeImportResult => {
      const parsed = decodeThemeShare(value);
      if (!parsed.ok || !parsed.payload) {
        return { ok: false, error: parsed.error };
      }
      const current = stateRef.current;
      const existingNames = new Set(
        current.customPresets.map((p) => p.name.toLowerCase()),
      );
      let name = parsed.payload.name;
      let suffix = 2;
      while (existingNames.has(name.toLowerCase())) {
        name = `${parsed.payload.name} ${suffix++}`;
      }
      const id = `custom-${parsed.payload.variant}-${Date.now().toString(36)}`;
      const now = new Date().toISOString();
      const preset: ThemePreset = {
        id,
        name,
        source: "imported",
        variants: { [parsed.payload.variant]: { ...parsed.payload.palette } },
        createdAt: now,
        updatedAt: now,
      };
      const next: PersistedThemeStateV2 = {
        ...current,
        customPresets: [...current.customPresets, preset],
      };
      commit(next);
      return { ok: true, presetId: id, variant: parsed.payload.variant };
    },
    [commit],
  );

  const availablePresets = useMemo<ThemePreset[]>(() => {
    const all = [...BUILTIN_PRESETS, ...state.customPresets];
    return all.filter((p) => p.variants[activeVariant]);
  }, [state.customPresets, activeVariant]);

  const currentPresetId = state.selectedPresetIds[activeVariant];
  const isCustom = isCustomPresetId(currentPresetId);

  const value: ThemeContextValue = {
    state,
    activeVariant,
    activeIsDark: activeVariant === "dark",
    activePalette,
    availablePresets,
    currentPresetId,
    isCustom,
    setMode,
    selectPreset,
    updateWorkingPalette,
    saveWorkingPalette,
    renameCustomPreset,
    deleteCustomPreset,
    updatePreferences,
    exportTheme,
    importTheme,
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useThemeContext(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useThemeContext must be used within ThemeProvider");
  return ctx;
}
