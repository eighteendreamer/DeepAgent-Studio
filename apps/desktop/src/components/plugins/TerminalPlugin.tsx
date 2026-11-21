import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { sshStatus, sshConnectionDetails, type SshConnection, type SshConnectionDetails } from "../../api";
import { useTranslation } from "react-i18next";
import { Button } from "../shadcn/button";
import { TerminalSessionController, type TerminalControllerState } from "./terminalSessionController";
import type { Terminal as XTerm } from "@xterm/xterm";
import type { FitAddon, ITerminalDimensions } from "@xterm/addon-fit";
import { useThemeContext } from "../../theme/ThemeProvider";
import { fontStack, resolveFont, type FontId } from "../../theme/themeFonts";
import { mixHex } from "../../theme/colorUtils";
import type { ThemePalette } from "../../theme/themeTypes";
import { TerminalMetadata, safeTerminalLabel, type TerminalMetadataEvent } from "../terminal/terminalMetadata";
import { bracketPaste } from "../terminal/terminalInputPolicy";
import { TerminalRecording, downloadArtifact } from "../terminal/terminalRecording";
import { TerminalZmodem } from "../terminal/terminalZmodem";
import { shellIntegrationCommand } from "../terminal/terminalShellIntegration";
import { TerminalHighlighter } from "../terminal/terminalHighlight";
import type { HighlightRule } from "../terminal/terminalPreferences";
import type { TerminalViewHandle, TerminalViewStatus } from "../terminal/terminalViewTypes";

export interface TerminalPluginProps {
  mode?: "local" | "remote";
  connectionId?: string | null;
  active?: boolean;
  initialCwd?: string;
  onDirectoryChange?: (path: string) => void;
  onMetadata?: (event: TerminalMetadataEvent) => void;
  onControls?: (controls: TerminalViewHandle | null) => void;
  onStatus?: (status: TerminalViewStatus) => void;
  onUserInput?: (data: string, source: "keys" | "paste" | "direct") => void;
  onUnlockRequest?: () => void;
  highlights?: HighlightRule[];
}

type TerminalRuntime = {
  Terminal: typeof import("@xterm/xterm").Terminal;
  FitAddon: typeof import("@xterm/addon-fit").FitAddon;
};

const TERMINAL_FONT_SIZE = 13;

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

async function prepareTerminalFont(id: FontId<"code">): Promise<string> {
  const { family } = resolveFont("code", id);
  if (family) {
    const font = `${TERMINAL_FONT_SIZE}px "${family}"`;
    try {
      await Promise.all([document.fonts.load(font), document.fonts.load(`700 ${font}`)]);
    } catch (error) {
      console.warn(`Could not load terminal font ${family}`, error);
    }
  }
  return fontStack("code", id);
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

export function TerminalPlugin({ mode = "local", connectionId = null, active = true, initialCwd, onDirectoryChange, onMetadata, onControls, onStatus, onUserInput, onUnlockRequest, highlights = [] }: TerminalPluginProps) {
  const { activePalette, activeIsDark, state: { preferences: { codeFont } } } = useThemeContext();
  const { t } = useTranslation();
  const texts = useRef(t);
  const activeRef = useRef(active);
  const codeFontRef = useRef(codeFont);
  const directoryChanged = useRef(onDirectoryChange);
  directoryChanged.current = onDirectoryChange;
  const callbacks = useRef({ onMetadata, onControls, onStatus, onUserInput, onUnlockRequest });
  callbacks.current = { onMetadata, onControls, onStatus, onUserInput, onUnlockRequest };
  const rules = useRef(highlights); rules.current = highlights;
  const highlighterRef = useRef<TerminalHighlighter | null>(null);
  const [lockDisplay, setLockDisplay] = useState({ locked: false, hideOutput: false });
  texts.current = t;
  activeRef.current = active;
  codeFontRef.current = codeFont;
  const [remoteConn, setRemoteConn] = useState<SshConnection | null>(null);
  const [forwarding, setForwarding] = useState<SshConnectionDetails | null>(null);
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

  useEffect(() => {
    let current = true; setForwarding(null);
    if (isRemote && connectionId && state === "running") void sshConnectionDetails(connectionId).then(value => { if (current) setForwarding(value); }, error => { if (current) setError(String(error)); });
    return () => { current = false; };
  }, [isRemote, connectionId, state]);

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
      let selection = codeFontRef.current;
      let fontFamily = await prepareTerminalFont(selection);
      while (!cancelled && selection !== codeFontRef.current) {
        selection = codeFontRef.current;
        fontFamily = await prepareTerminalFont(selection);
      }
      if (cancelled || !containerRef.current) return;
      const term = new Terminal({
        allowProposedApi: true,
        cursorBlink: true, cursorStyle: "bar", cursorWidth: 1,
        fontFamily,
        fontSize: TERMINAL_FONT_SIZE, lineHeight: 1.35, theme: terminalTheme, scrollback: 3000, convertEol: false,
      });
      const fit = new FitAddon();
      term.loadAddon(fit);
      term.open(containerRef.current);
      let integrationTimer: ReturnType<typeof setTimeout> | null = null;
      const metadata = new TerminalMetadata(event => {
        if (event.kind === "directory") directoryChanged.current?.(event.path);
        if (event.kind === "integration" && event.status !== "waiting" && integrationTimer) { clearTimeout(integrationTimer); integrationTimer = null; }
        callbacks.current.onMetadata?.(event);
      }, isRemote);
      const signals = [
        term.parser.registerOscHandler(7, value => metadata.osc(7, value)),
        term.parser.registerOscHandler(777, value => metadata.osc(777, value)),
        term.onTitleChange(title => callbacks.current.onMetadata?.({ kind: "title", title: safeTerminalLabel(title) })),
        term.onBell(() => metadata.bell()),
      ];
      termRef.current = term;
      fitRef.current = fit;
      let acceptsInput = false;
      setLockDisplay({ locked: false, hideOutput: false });
      const status: TerminalViewStatus = { state: "starting", locked: false, hideOutput: false, recording: false, hasRecording: false, zmodem: null };
      const publish = () => { if (!cancelled) callbacks.current.onStatus?.({ ...status }); };
      const showError = (detail: unknown) => { if (!cancelled) setError(String(detail)); };
      let recorder: TerminalRecording | null = null;
      const highlighter = new TerminalHighlighter(term, rules.current, showError); highlighterRef.current = highlighter;
      const display = async (text: string) => {
        if (cancelled) return;
        if (status.recording && recorder && !status.locked) {
          try { recorder.output(text); } catch (error) { status.recording = false; publish(); showError(error); }
        }
        await new Promise<void>(resolve => term.write(text, resolve));
        if (activeRef.current) highlighter.schedule();
      };
      let binary: TerminalZmodem;
      const controller = new TerminalSessionController(
        isRemote && connectionId ? { kind: "ssh", connectionId } : { kind: "local" },
        `ui-terminal:${crypto.randomUUID()}`,
        {
          write: display,
          bytes: (bytes, eof) => binary.consume(bytes, eof),
          inputError: showError,
          state: (next, detail) => {
            if (cancelled) return;
            acceptsInput = next === "running";
            setState(next);
            setError(detail);
            status.state = next; publish();
          },
          gap: () => {
            if (cancelled) return;
            term.reset();
            metadata.gap();
            binary.cancel();
            term.writeln(`[${texts.current("terminalSession.historyGap")}]`);
          },
        },
        initialCwd,
      );
      const makeBinary = () => new TerminalZmodem(controller, display, value => { status.zmodem = value; publish(); }, (name, chunks) => {
        if (!cancelled) downloadArtifact(name, new Blob(chunks.map(bytes => bytes.slice().buffer)));
      }, showError);
      binary = makeBinary();
      controllerRef.current = controller;
      const controls: TerminalViewHandle = {
        send: async data => { if (!acceptsInput || status.locked || binary.busy) throw new Error("Terminal input is unavailable"); await controller.write(data); },
        paste: async (text, execute = false) => { if (!acceptsInput || status.locked || binary.busy) throw new Error("Terminal input is unavailable"); await controller.write(bracketPaste(text, term.modes.bracketedPasteMode) + (execute ? "\r" : "")); },
        lock: async (password, hideOutput) => {
          binary.cancel();
          await controller.setLock(password, hideOutput); status.locked = true; status.hideOutput = hideOutput; status.recording = false;
          term.options.disableStdin = true; setLockDisplay({ locked: true, hideOutput }); publish();
        },
        unlock: async password => { await controller.unlock(password); status.locked = false; status.hideOutput = false; term.options.disableStdin = false; setError(undefined); setLockDisplay({ locked: false, hideOutput: false }); publish(); scheduleLayout(true); },
        integrate: async shell => {
          if (!acceptsInput || status.locked || binary.busy) throw new Error("Terminal input is unavailable");
          const nonce = crypto.randomUUID(); metadata.expectIntegration(nonce);
          if (integrationTimer) clearTimeout(integrationTimer);
          integrationTimer = setTimeout(() => { metadata.gap(); integrationTimer = null; }, 10_000);
          await controller.write(shellIntegrationCommand(shell, nonce));
        },
        record: enabled => {
          if (enabled && status.locked) throw new Error("Terminal session is locked");
          if (enabled) { recorder?.dispose(); recorder = new TerminalRecording(term.cols, term.rows); status.hasRecording = true; }
          status.recording = enabled; publish();
        },
        recording: () => recorder?.export() ?? null,
        discardRecording: () => { recorder?.dispose(); recorder = null; status.recording = false; status.hasRecording = false; publish(); },
        focus: () => term.focus(),
        confirmTransfer: () => binary.confirm(), acceptTransfer: () => binary.acceptReceive(), skipTransfer: () => binary.skip(),
        cancelTransfer: () => binary.cancel(),
        sendFiles: files => binary.sendFiles(files),
      };
      callbacks.current.onControls?.(controls);
      let userKey = "";
      const key = term.onKey(event => { userKey = event.key; });
      const beforeInput = (event: InputEvent) => { if (event.data && event.inputType !== "insertFromPaste") userKey = event.data; };
      containerRef.current.addEventListener("beforeinput", beforeInput as EventListener, true);
      const input = term.onData(data => {
        if (!acceptsInput || cancelled || status.locked || binary.busy) return;
        const source = userKey === data ? "keys" : "direct"; userKey = "";
        if (callbacks.current.onUserInput) callbacks.current.onUserInput(data, source);
        else void controls.send(data).catch(showError);
      });
      const binaryInput = term.onBinary(data => { if (acceptsInput && !cancelled && !status.locked && !binary.busy) void controller.writeBytes(Uint8Array.from(data, c => c.charCodeAt(0) & 255)).catch(showError); });
      const paste = (event: ClipboardEvent) => {
        event.preventDefault(); event.stopImmediatePropagation();
        if (acceptsInput && !status.locked && !binary.busy) callbacks.current.onUserInput?.(event.clipboardData?.getData("text/plain") ?? "", "paste");
      };
      containerRef.current.addEventListener("paste", paste, true);
      const surface = containerRef.current;
      const resize = term.onResize(size => { if (status.recording && recorder) { try { recorder.resize(size.cols, size.rows); } catch (error) { status.recording = false; publish(); showError(error); } } highlighter.schedule(); });
      const scroll = term.onScroll(() => { if (activeRef.current) highlighter.schedule(); });
      const observer = new ResizeObserver(() => scheduleLayout(true));
      observer.observe(containerRef.current);
      cleanup = () => {
        input.dispose(); key.dispose(); binaryInput.dispose(); resize.dispose(); scroll.dispose(); signals.forEach(signal => signal.dispose()); observer.disconnect();
        surface.removeEventListener("beforeinput", beforeInput as EventListener, true);
        surface.removeEventListener("paste", paste, true); highlighter.dispose(); binary.dispose(); recorder?.dispose();
        if (integrationTimer) clearTimeout(integrationTimer);
        callbacks.current.onControls?.(null);
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

  useEffect(() => { highlighterRef.current?.configure(highlights); }, [highlights]);

  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    let cancelled = false;
    void prepareTerminalFont(codeFont).then(fontFamily => {
      if (cancelled || termRef.current !== term || codeFontRef.current !== codeFont) return;
      if (term.options.fontFamily === fontFamily) return;
      term.options.fontFamily = fontFamily;
      scheduleLayout(true);
    });
    return () => { cancelled = true; };
  }, [codeFont, scheduleLayout]);

  useEffect(() => {
    if (active) {
      scheduleLayout(true);
      highlighterRef.current?.schedule();
      termRef.current?.refresh(0, Math.max(0, termRef.current.rows - 1));
      termRef.current?.focus();
    }
  }, [active, scheduleLayout]);

  if (isRemote && !connectionId) {
    return <div className="flex h-full items-center justify-center text-text-secondary">{t("remote.emptyTitle")}</div>;
  }
  return <div className="relative flex h-full w-full flex-col overflow-hidden bg-bg-base">
    {isRemote && remoteConn && <div className="flex shrink-0 flex-wrap gap-x-2 border-b border-border-theme bg-elevated-bg/90 px-3 py-1 text-[11px] text-text-secondary"><span>{remoteConn.username}@{remoteConn.host}</span>{forwarding && <span>Agent {t(`terminalWorkbench.${forwarding.agent_forwarding_enabled ? "enabled" : "disabled"}`)} · X11 {t(`terminalWorkbench.${forwarding.x11_forwarding_enabled ? "enabled" : "disabled"}`)}</span>}</div>}
    {state === "starting" && <div role="status" className="absolute inset-0 z-10 flex items-center justify-center bg-bg-base/80 text-[12px] text-text-secondary">{t("terminalSession.starting")}</div>}
    <div ref={containerRef} className="terminal-surface min-h-0 w-full flex-1 px-3 py-2" style={{ visibility: lockDisplay.locked && lockDisplay.hideOutput ? "hidden" : undefined }} />
    {lockDisplay.locked && <div className={`absolute inset-0 z-20 flex items-center justify-center ${lockDisplay.hideOutput ? "bg-bg-base" : "bg-bg-base/40"}`}><Button variant="outline" onClick={() => callbacks.current.onUnlockRequest?.()}>{t("terminalWorkbench.unlock")}</Button></div>}
    {error && state === "running" && <div role="alert" className="absolute bottom-1 left-2 right-2 z-10 flex items-center justify-between rounded bg-elevated-bg px-2 text-xs text-red-500"><span>{error}</span><Button size="sm" variant="ghost" onClick={() => setError(undefined)}>×</Button></div>}
    {(state === "error" || state === "exited") && <div role="status" className={`absolute left-3 bottom-2 ${isRemote ? "right-16" : "right-3"} flex items-center justify-between gap-2 rounded-lg bg-elevated-bg/95 px-3 py-2 text-[12px] text-text-secondary shadow-sm`}>
      <span className="min-w-0 break-words">{t(`terminalSession.${state}`)}{error ? `: ${error}` : ""}</span>
      <Button variant="outline" size="sm" onClick={() => setRetry(value => value + 1)}>{t("terminalSession.retry")}</Button>
    </div>}
  </div>;
}
