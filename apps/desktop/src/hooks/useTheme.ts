import { useState, useEffect, useRef } from "react";
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

const THEME_TRANSITION_DURATION_MS = 600;
const THEME_COLOR_FADE_DURATION_MS = 240;
let activeThemeRipple: Animation | null = null;
let activeThemeRippleElement: HTMLDivElement | null = null;
let activeThemeCommitTimer: number | null = null;
let activeThemeColorFadeTimer: number | null = null;

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

function playThemeRipple(origin: ThemeSwitchOrigin, color: string, onCovered: () => void) {
  activeThemeRipple?.cancel();
  activeThemeRippleElement?.remove();
  if (activeThemeCommitTimer !== null) {
    window.clearTimeout(activeThemeCommitTimer);
  }

  const viewportWidth = window.visualViewport?.width ?? window.innerWidth;
  const viewportHeight = window.visualViewport?.height ?? window.innerHeight;
  const x = Number.isFinite(origin.x) ? origin.x : viewportWidth / 2;
  const y = Number.isFinite(origin.y) ? origin.y : viewportHeight / 2;
  const radius = Math.hypot(
    Math.max(x, viewportWidth - x),
    Math.max(y, viewportHeight - y),
  ) + 2;

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

function beginThemeColorFade() {
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
  const configRef = useRef(config);
  const displayedIsDarkRef = useRef(resolveIsDark(config.mode));

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
    configRef.current = updated;
    displayedIsDarkRef.current = isDark;
    applyCssVariables(isDark ? updated.dark : updated.light, isDark);
    setActiveIsDark(isDark);
    setConfigState(updated);
    localStorage.setItem("codex-theme-config", JSON.stringify(updated));
    window.dispatchEvent(new Event("codex-theme-changed"));
  };

  const updateConfig = (newConfig: Partial<ThemeConfig> | ((prev: ThemeConfig) => ThemeConfig)) => {
    const next = typeof newConfig === "function"
      ? newConfig(configRef.current)
      : { ...configRef.current, ...newConfig };
    commitConfig(next);
  };

  const switchTheme = (mode: ThemeMode, origin: ThemeSwitchOrigin) => {
    const currentConfig = configRef.current;
    if (mode === currentConfig.mode) return;

    const nextConfig = normalizeConfig({ ...currentConfig, mode });
    const nextIsDark = resolveIsDark(mode);
    configRef.current = nextConfig;

    if (displayedIsDarkRef.current === nextIsDark) {
      activeThemeRipple?.cancel();
      activeThemeRippleElement?.remove();
      if (activeThemeCommitTimer !== null) {
        window.clearTimeout(activeThemeCommitTimer);
        activeThemeCommitTimer = null;
      }
      commitConfig(nextConfig);
      return;
    }

    const nextTheme = nextIsDark ? nextConfig.dark : nextConfig.light;
    playThemeRipple(origin, nextTheme.bg, () => {
      beginThemeColorFade();
      commitConfig(nextConfig);
    });
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
