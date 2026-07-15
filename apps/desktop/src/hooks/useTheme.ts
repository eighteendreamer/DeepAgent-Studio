import { useState, useEffect } from "react";
import { flushSync } from "react-dom";
import deepmerge from "deepmerge";

export type ThemeMode = "light" | "dark" | "system";

export interface ThemeSwitchOrigin {
  x: number;
  y: number;
}

export interface ThemeConfigDetails {
  accent: string;
  bg: string;
  fg: string;
  uiFont: string;
  codeFont: string;
  translucentSidebar: boolean;
  contrast: number;
  cssVariables: Record<string, string>;
}

export interface ThemeConfig {
  mode: ThemeMode;
  light: ThemeConfigDetails;
  dark: ThemeConfigDetails;
}

interface ViewTransitionLike {
  finished: Promise<void>;
  skipTransition?: () => void;
}

type ViewTransitionDocument = Document & {
  startViewTransition?: (update: () => void) => ViewTransitionLike;
};

let activeThemeTransition: ViewTransitionLike | null = null;
const THEME_TRANSITION_DURATION_MS = 2400;

export const DEFAULT_THEME_CONFIG: ThemeConfig = {
  mode: "system",
  light: {
    accent: "#339CFF",
    bg: "#FFFFFF",
    fg: "#1A1C1F",
    uiFont: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto',
    codeFont: 'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas',
    translucentSidebar: true,
    contrast: 45,
    cssVariables: {
      "--theme-accent": "#339CFF",
      "--theme-accent-hover": "#2563eb",
      "--theme-bg": "#FFFFFF",
      "--theme-sidebar": "#F9F8F6",
      "--theme-fg": "#1F2937",
      "--theme-text-secondary": "#6B7280",
      "--theme-border": "#E5E7EB",
      "--ui-font": 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto',
      "--code-font": 'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas'
    }
  },
  dark: {
    accent: "#339CFF",
    bg: "#000000",
    fg: "#F9FAFB",
    uiFont: 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto',
    codeFont: 'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas',
    translucentSidebar: true,
    contrast: 50,
    cssVariables: {
      "--theme-accent": "#339CFF",
      "--theme-accent-hover": "#2563eb",
      "--theme-bg": "#000000",
      "--theme-sidebar": "#121212",
      "--theme-fg": "#F9FAFB",
      "--theme-text-secondary": "#9CA3AF",
      "--theme-border": "#27272A",
      "--ui-font": 'Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto',
      "--code-font": 'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas'
    }
  }
};

function applyCssVariables(theme: ThemeConfigDetails, isDark: boolean) {
  const root = document.documentElement;
  for (const [key, value] of Object.entries(theme.cssVariables)) {
    root.style.setProperty(key, value);
  }
  if (isDark) {
    root.classList.add("dark");
  } else {
    root.classList.remove("dark");
  }
}

function resolveIsDark(mode: ThemeMode): boolean {
  return mode === "dark" || (
    mode === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches
  );
}

function computeCssVariables(details: Omit<ThemeConfigDetails, 'cssVariables'>, isDark: boolean): Record<string, string> {
  // A simple heuristic for derivatives based on bg/fg/accent
  // In a real app, you'd use a color library like 'color' or 'd3-color'
  let sidebar = details.bg;
  let border = isDark ? "#374151" : "#E5E7EB";
  
  if (isDark) {
    if (details.bg === "#000000" || details.bg === "#000") {
      sidebar = "#121212";
      border = "#27272A";
    } else {
      sidebar = "#1F2937";
    }
  } else {
    sidebar = "#F9F8F6";
  }

  const textSecondary = isDark ? "#9CA3AF" : "#6B7280";
  
  return {
    "--theme-accent": details.accent,
    "--theme-accent-hover": details.accent, // Simplified
    "--theme-bg": details.bg,
    "--theme-sidebar": sidebar,
    "--theme-fg": details.fg,
    "--theme-text-secondary": textSecondary,
    "--theme-border": border,
    "--ui-font": details.uiFont,
    "--code-font": details.codeFont
  };
}

export function useTheme() {
  const [config, setConfigState] = useState<ThemeConfig>(() => {
    try {
      const stored = localStorage.getItem("codex-theme-config");
      if (stored) {
        return deepmerge(DEFAULT_THEME_CONFIG, JSON.parse(stored));
      }
    } catch (e) {
      console.error("Failed to parse theme config", e);
    }
    return DEFAULT_THEME_CONFIG;
  });

  const [activeIsDark, setActiveIsDark] = useState<boolean>(false);

  useEffect(() => {
    const handleSystemChange = (e: MediaQueryListEvent) => {
      if (config.mode === "system") {
        setActiveIsDark(e.matches);
        applyCssVariables(e.matches ? config.dark : config.light, e.matches);
      }
    };
    
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    mediaQuery.addEventListener("change", handleSystemChange);
    
    const isDark = resolveIsDark(config.mode);
    
    setActiveIsDark(isDark);
    const activeTheme = isDark ? config.dark : config.light;
    applyCssVariables(activeTheme, isDark);

    return () => {
      mediaQuery.removeEventListener("change", handleSystemChange);
    };
  }, [config]);

  const normalizeConfig = (next: ThemeConfig): ThemeConfig => ({
    ...next,
    light: {
      ...next.light,
      cssVariables: computeCssVariables(next.light, false),
    },
    dark: {
      ...next.dark,
      cssVariables: computeCssVariables(next.dark, true),
    },
  });

  const commitConfig = (next: ThemeConfig) => {
    const updated = normalizeConfig(next);
    const isDark = resolveIsDark(updated.mode);
    applyCssVariables(isDark ? updated.dark : updated.light, isDark);
    setActiveIsDark(isDark);
    setConfigState(updated);
    localStorage.setItem("codex-theme-config", JSON.stringify(updated));
    window.dispatchEvent(new Event("codex-theme-changed"));
  };

  const updateConfig = (newConfig: Partial<ThemeConfig> | ((prev: ThemeConfig) => ThemeConfig)) => {
    const next = typeof newConfig === "function"
      ? newConfig(config)
      : { ...config, ...newConfig };
    commitConfig(next);
  };

  const switchTheme = (mode: ThemeMode, origin: ThemeSwitchOrigin) => {
    if (mode === config.mode) return;

    const nextConfig = { ...config, mode };
    const currentIsDark = resolveIsDark(config.mode);
    const nextIsDark = resolveIsDark(mode);
    const root = document.documentElement;
    const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const commit = () => commitConfig(nextConfig);

    // Switching to "system" can change only the selected control while keeping
    // the same effective palette. Avoid a full-screen reveal in that case.
    if (prefersReducedMotion || currentIsDark === nextIsDark) {
      commit();
      return;
    }

    const x = Number.isFinite(origin.x) ? origin.x : window.innerWidth / 2;
    const y = Number.isFinite(origin.y) ? origin.y : window.innerHeight / 2;
    root.style.setProperty("--theme-switch-x", `${x}px`);
    root.style.setProperty("--theme-switch-y", `${y}px`);

    const transitionDocument = document as ViewTransitionDocument;
    if (!transitionDocument.startViewTransition) {
      root.classList.add("theme-transition-fallback");
      flushSync(commit);
      window.setTimeout(
        () => root.classList.remove("theme-transition-fallback"),
        THEME_TRANSITION_DURATION_MS,
      );
      return;
    }

    activeThemeTransition?.skipTransition?.();
    root.classList.add("theme-transition-active");
    const transition = transitionDocument.startViewTransition(() => {
      flushSync(commit);
    });
    activeThemeTransition = transition;
    const finishTransition = () => {
      if (activeThemeTransition === transition) {
        activeThemeTransition = null;
        root.classList.remove("theme-transition-active");
      }
    };
    void transition.finished.then(finishTransition, finishTransition);
  };

  const updateThemeDetails = (isDarkTheme: boolean, updates: Partial<Omit<ThemeConfigDetails, 'cssVariables'>>) => {
    updateConfig(prev => {
      const targetKey = isDarkTheme ? 'dark' : 'light';
      return {
        ...prev,
        [targetKey]: {
          ...prev[targetKey],
          ...updates
        }
      };
    });
  };

  return { config, activeIsDark, updateConfig, updateThemeDetails, switchTheme };
}
