import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { sshStatus, type SshConnection } from "../../api";
import { useTranslation } from "react-i18next";
import { Button } from "../shadcn/button";
import { TerminalSessionController, type TerminalControllerState } from "./terminalSessionController";
import type { Terminal as XTerm } from "@xterm/xterm";
import type { FitAddon, ITerminalDimensions } from "@xterm/addon-fit";
import type { PluginDefinition } from "./pluginTypes";
import { useThemeContext } from "../../theme/ThemeProvider";
import { mixHex } from "../../theme/colorUtils";
import type { ThemePalette } from "../../theme/themeTypes";

interface TerminalPluginProps {
  mode?: "local" | "remote";
  connectionId?: string | null;
  active?: boolean;
  initialCwd?: string;
  onDirectoryChange?: (path: string) => void;
}

type TerminalRuntime = {
  Terminal: typeof import("@xterm/xterm").Terminal;
  FitAddon: typeof import("@xterm/addon-fit").FitAddon;
};

let terminalRuntimePromise: Promise<TerminalRuntime> | null = null;

function loadTerminalRuntime() {
  terminalRuntimePromise ??= Promise.all([
    import("@xterm/xterm"),
    import("@xterm/addon-fit"),
    import("@xterm/xterm/css/xterm.css"),
  ]).then(([xterm, fit]) => ({
    Terminal: xterm.Terminal,
    FitAddon: fit.FitAddon,
  }));
  return terminalRuntimePromise;
}

function buildTerminalTheme(palette: ThemePalette, isDark: boolean) {
  const bg = palette.background;
  const fg = palette.foreground;
  const muted = palette.foregroundMuted;
  const border = palette.border;
  const accent = palette.accent;
  const accentHover = palette.accentHover;
  const makeShade = (lightT: number, darkT: number) =>
    mixHex(bg, fg, isDark ? darkT : lightT);

  return {
    background: bg,
    foreground: fg,
    cursor: fg,
    cursorAccent: bg,
    selectionBackground: palette.selection,
    selectionForeground: fg,
    selectionInactiveBackground: palette.selection,
    scrollbarSliderBackground: mixHex(bg, muted, isDark ? 0.55 : 0.38),
    scrollbarSliderHoverBackground: mixHex(bg, muted, isDark ? 0.68 : 0.52),
    scrollbarSliderActiveBackground: mixHex(bg, muted, isDark ? 0.8 : 0.64),
    overviewRulerBorder: border,
    black: makeShade(0.88, 0.22),
    red: isDark ? "#F87171" : "#DC2626",
    green: isDark ? "#34D399" : "#059669",
    yellow: isDark ? "#FBBF24" : "#B45309",
    blue: isDark ? accentHover : "#2563EB",
    magenta: isDark ? "#C084FC" : "#7C3AED",
    cyan: isDark ? "#22D3EE" : "#0891B2",
    white: makeShade(0.18, 0.84),
    brightBlack: makeShade(0.7, 0.4),
    brightRed: isDark ? "#FCA5A5" : "#EF4444",
    brightGreen: isDark ? "#6EE7B7" : "#10B981",
    brightYellow: isDark ? "#FCD34D" : "#F59E0B",
    brightBlue: isDark ? accent : "#3B82F6",
    brightMagenta: isDark ? "#D8B4FE" : "#8B5CF6",
    brightCyan: isDark ? "#67E8F9" : "#06B6D4",
    brightWhite: makeShade(0.06, 0.96),
  };
}

export function TerminalPlugin({ mode = "local", connectionId = null, active = true, initialCwd, onDirectoryChange }: TerminalPluginProps) {
  const { activePalette, activeIsDark } = useThemeContext();
  const { t } = useTranslation();
  const texts = useRef(t);
  const activeRef = useRef(active);
  const directoryChanged = useRef(onDirectoryChange);
  directoryChanged.current = onDirectoryChange;
  texts.current = t;
  activeRef.current = active;
  const [remoteConn, setRemoteConn] = useState<SshConnection | null>(null);
  const [state, setState] = useState<TerminalControllerState>("starting");
  const [error, setError] = useState<string>();
  const [retry, setRetry] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const controllerRef = useRef<TerminalSessionController | null>(null);
  const layoutFrame = useRef<number | null>(null);
  const resizeTimer = useRef<number | null>(null);
  const terminalTheme = useMemo(() => buildTerminalTheme(activePalette, activeIsDark), [activePalette, activeIsDark]);
  const isRemote = mode === "remote";

  useEffect(() => {
    let current = true;
    setRemoteConn(null);
    if (isRemote && connectionId) {
      sshStatus(connectionId).then(
        value => { if (current) setRemoteConn(value); },
        () => { if (current) setRemoteConn(null); },
      );
    }
    return () => { current = false; };
  }, [isRemote, connectionId]);

  const syncLayout = useCallback((notifyPty: boolean): ITerminalDimensions | null => {
    const term = termRef.current, fit = fitRef.current, container = containerRef.current;
    if (!term || !fit || !container) return null;
    const rect = container.getBoundingClientRect();
    if (rect.width < 40 || rect.height < 24) return null;
    const size = fit.proposeDimensions();
    if (!size) return null;
    const cols = Math.max(size.cols, 2), rows = Math.max(size.rows, 1);
    if (term.cols !== cols || term.rows !== rows) term.resize(cols, rows);
    if (notifyPty) {
      if (resizeTimer.current !== null) window.clearTimeout(resizeTimer.current);
      const controller = controllerRef.current;
      resizeTimer.current = window.setTimeout(() => {
        resizeTimer.current = null;
        if (controller && controllerRef.current === controller) void controller.resize(cols, rows).catch(() => {});
      }, 120);
    }
    return { cols, rows };
  }, []);

  const scheduleLayout = useCallback((notifyPty: boolean) => {
    if (layoutFrame.current !== null) window.cancelAnimationFrame(layoutFrame.current);
    layoutFrame.current = window.requestAnimationFrame(() => {
      layoutFrame.current = null;
      syncLayout(notifyPty);
    });
  }, [syncLayout]);

  useEffect(() => {
    if (!containerRef.current || (isRemote && !connectionId)) return;
    let cancelled = false;
    let cleanup: (() => void) | undefined;
    setState("starting");
    setError(undefined);
    void loadTerminalRuntime().then(async ({ Terminal, FitAddon }) => {
      if (cancelled || !containerRef.current) return;
      const term = new Terminal({
        cursorBlink: true, cursorStyle: "bar", cursorWidth: 1,
        fontFamily: '\"Cascadia Mono\", \"Consolas\", \"Courier New\", monospace',
        fontSize: 13, lineHeight: 1.35, theme: terminalTheme, scrollback: 3000, convertEol: false,
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(containerRef.current);
      const directorySignal = term.parser.registerOscHandler(7, value => {
        try {
          const url = new URL(value);
          if (url.protocol === "file:") {
            let path = decodeURIComponent(url.pathname);
            if (!isRemote && /^\/[A-Za-z]:\//.test(path)) path = path.slice(1);
            if (path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path)) directoryChanged.current?.(path);
          }
        } catch { /* Ignore malformed shell integration sequences. */ }
        return true;
      });
      termRef.current = term;
      fitRef.current = fit;
      let acceptsInput = false;
      const controller = new TerminalSessionController(
        isRemote && connectionId ? { kind: "ssh", connectionId } : { kind: "local" },
        `ui-terminal:${crypto.randomUUID()}`,
        {
          write: text => new Promise<void>(resolve => {
            if (cancelled) { resolve(); return; }
            term.write(text, resolve);
          }),
          state: (next, detail) => {
            if (cancelled) return;
            acceptsInput = next === "running";
            setState(next);
            setError(detail);
          },
          gap: () => {
            if (cancelled) return;
            term.reset();
            term.writeln(`[${texts.current("terminalSession.historyGap")}]`);
          },
        },
        initialCwd,
      );
      controllerRef.current = controller;
      const input = term.onData(data => {
        if (acceptsInput && !cancelled) void controller.write(data).catch(() => {});
      });
      const observer = new ResizeObserver(() => scheduleLayout(true));
      observer.observe(containerRef.current);
      cleanup = () => {
        input.dispose(); directorySignal.dispose(); observer.disconnect();
        if (layoutFrame.current !== null) window.cancelAnimationFrame(layoutFrame.current);
        if (resizeTimer.current !== null) window.clearTimeout(resizeTimer.current);
        layoutFrame.current = resizeTimer.current = null;
        if (controllerRef.current === controller) controllerRef.current = null;
        void controller.close().catch(error => console.warn("Terminal session cleanup failed", error));
        term.dispose();
        if (termRef.current === term) { termRef.current = null; fitRef.current = null; }
      };
      await document.fonts.ready;
      if (cancelled) return;
      const size = syncLayout(false) ?? { cols: 80, rows: 24 };
      await controller.open(size.cols, size.rows);
      if (!cancelled) {
        scheduleLayout(true);
        if (activeRef.current) term.focus();
      }
    }).catch(error => {
      if (!cancelled) { setState("error"); setError(String(error)); }
    });
    return () => { cancelled = true; cleanup?.(); };
  // Palette/language/visibility changes update the existing terminal, never reopen its shell.
  }, [connectionId, isRemote, initialCwd, retry, scheduleLayout, syncLayout]);

  useEffect(() => {
    if (termRef.current) termRef.current.options.theme = terminalTheme;
  }, [terminalTheme]);

  useEffect(() => {
    if (active) {
      scheduleLayout(true);
      termRef.current?.refresh(0, Math.max(0, termRef.current.rows - 1));
      termRef.current?.focus();
    }
  }, [active, scheduleLayout]);

  if (isRemote && !connectionId) {
    return <div className="flex h-full items-center justify-center text-text-secondary">{t("remote.emptyTitle")}</div>;
  }
  return <div className="relative h-full w-full overflow-hidden bg-bg-base">
    {state === "starting" && <div role="status" className="absolute inset-0 z-10 flex items-center justify-center bg-bg-base/80 text-[12px] text-text-secondary">{t("terminalSession.starting")}</div>}
    <div ref={containerRef} className="terminal-surface h-full w-full px-3 py-2" />
    {isRemote && remoteConn && <div className="pointer-events-none absolute right-3 top-2 rounded-full bg-elevated-bg/90 px-2 py-0.5 text-[11px] text-text-secondary">{remoteConn.username}@{remoteConn.host}</div>}
    {(state === "error" || state === "exited") && <div role="status" className={`absolute left-3 bottom-2 ${isRemote ? "right-16" : "right-3"} flex items-center justify-between gap-2 rounded-lg bg-elevated-bg/95 px-3 py-2 text-[12px] text-text-secondary shadow-sm`}>
      <span className="min-w-0 break-words">{t(`terminalSession.${state}`)}{error ? `: ${error}` : ""}</span>
      <Button variant="outline" size="sm" onClick={() => setRetry(value => value + 1)}>{t("terminalSession.retry")}</Button>
    </div>}
  </div>;
}

export const terminalPluginDefinition: PluginDefinition = {
  type: "terminal",
  icon: ["fas", "terminal"],
  titleKey: "terminal",
  descKey: "terminalDesc",
  fallbackTitle: "Terminal",
  fallbackDesc: "Launch interactive shell",
  getTabTitle: ({ activeProjectPath, envMode = "local", selectedConnection }) => {
    if (envMode === "remote") {
      if (selectedConnection?.name) return selectedConnection.name;
      if (selectedConnection?.username && selectedConnection?.host) {
        return `${selectedConnection.username}@${selectedConnection.host}`;
      }
      return "SSH Terminal";
    }
    const path = activeProjectPath?.trim();
    return path && path.length > 0 ? path : "Terminal";
  },
  render: ({ envMode = "local", selectedConnectionId }) => (
    <TerminalPlugin mode={envMode} connectionId={selectedConnectionId} />
  ),
};
