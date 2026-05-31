import { useState, useEffect } from "react";
import deepmerge from "deepmerge";

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
  mode: "light" | "dark" | "system";
  light: ThemeConfigDetails;
  dark: ThemeConfigDetails;
}

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
      }
    };
    
    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    mediaQuery.addEventListener("change", handleSystemChange);
    
    let isDark = config.mode === "dark";
    if (config.mode === "system") {
      isDark = mediaQuery.matches;
    }
    
    setActiveIsDark(isDark);
    const activeTheme = isDark ? config.dark : config.light;
    applyCssVariables(activeTheme, isDark);

    return () => {
      mediaQuery.removeEventListener("change", handleSystemChange);
    };
  }, [config]);

  const updateConfig = (newConfig: Partial<ThemeConfig> | ((prev: ThemeConfig) => ThemeConfig)) => {
    setConfigState(prev => {
      let updated: ThemeConfig;
      if (typeof newConfig === 'function') {
        updated = newConfig(prev);
      } else {
        updated = { ...prev, ...newConfig };
      }
      
      // Recompute CSS variables whenever config changes
      updated.light.cssVariables = computeCssVariables(updated.light, false);
      updated.dark.cssVariables = computeCssVariables(updated.dark, true);

      localStorage.setItem("codex-theme-config", JSON.stringify(updated));
      // Dispatch an event so other tabs/windows or components can listen if needed
      window.dispatchEvent(new Event("codex-theme-changed"));
      return updated;
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

  return { config, activeIsDark, updateConfig, updateThemeDetails };
}
