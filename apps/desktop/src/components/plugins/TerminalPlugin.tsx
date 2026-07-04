import { useCallback, useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import {
  localPtyClose,
  localPtyRead,
  localPtyResize,
  localPtySpawn,
  localPtyWrite,
  sshPtyRead,
  sshPtyResize,
  sshPtySpawn,
  sshPtyWrite,
  sshStatus,
  type LocalPtyHandle,
  type SshConnection,
  type SshPtyHandle,
} from "../../api";
import { Terminal as XTerm } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import type { ITerminalDimensions } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import type { PluginDefinition } from "./pluginTypes";

interface TerminalPluginProps {
  mode?: "local" | "remote";
  connectionId?: string | null;
}

type TerminalSession =
  | { kind: "local"; handle: LocalPtyHandle }
  | { kind: "remote"; handle: SshPtyHandle };

type TerminalLayout = {
  width: number;
  height: number;
  cols: number;
  rows: number;
};

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function TerminalPlugin({ mode = "local", connectionId = null }: TerminalPluginProps) {
  const [remoteConn, setRemoteConn] = useState<SshConnection | null>(null);
  const [booting, setBooting] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<XTerm | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const sessionRef = useRef<TerminalSession | null>(null);
  const pollRef = useRef<number | null>(null);
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const resizeFrameRef = useRef<number | null>(null);
  const ptyResizeTimerRef = useRef<number | null>(null);
  const lastLayoutRef = useRef<TerminalLayout | null>(null);
  const lastSentPtySizeRef = useRef<{ cols: number; rows: number } | null>(null);
  const decoderRef = useRef(new TextDecoder());
  const readingRef = useRef(false);

  const isRemote = mode === "remote";

  useEffect(() => {
    if (isRemote && connectionId) {
      sshStatus(connectionId).then(setRemoteConn).catch(() => setRemoteConn(null));
    } else {
      setRemoteConn(null);
    }
  }, [isRemote, connectionId]);

  const stopPolling = useCallback(() => {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
    readingRef.current = false;
  }, []);

  const clearScheduledResize = useCallback(() => {
    if (resizeFrameRef.current !== null) {
      window.cancelAnimationFrame(resizeFrameRef.current);
      resizeFrameRef.current = null;
    }
    if (ptyResizeTimerRef.current !== null) {
      window.clearTimeout(ptyResizeTimerRef.current);
      ptyResizeTimerRef.current = null;
    }
  }, []);

  const closeSession = useCallback(async () => {
    stopPolling();
    clearScheduledResize();
    const current = sessionRef.current;
    sessionRef.current = null;
    lastSentPtySizeRef.current = null;
    if (current?.kind === "local") {
      try {
        await localPtyClose(current.handle.pty_id);
      } catch {
        // ignore best-effort cleanup errors
      }
    }
  }, [clearScheduledResize, stopPolling]);

  const schedulePtyResize = useCallback((cols: number, rows: number) => {
    const current = sessionRef.current;
    if (!current) return;

    const last = lastSentPtySizeRef.current;
    if (last && last.cols === cols && last.rows === rows) return;

    if (ptyResizeTimerRef.current !== null) {
      window.clearTimeout(ptyResizeTimerRef.current);
    }

    ptyResizeTimerRef.current = window.setTimeout(() => {
      ptyResizeTimerRef.current = null;
      const active = sessionRef.current;
      if (!active) return;

      lastSentPtySizeRef.current = { cols, rows };
      if (active.kind === "local") {
        void localPtyResize(active.handle.pty_id, cols, rows);
      } else {
        void sshPtyResize(active.handle.connection_id, cols, rows);
      }
    }, 120);
  }, []);

  const syncTerminalLayout = useCallback(
    (notifyPty: boolean): ITerminalDimensions | null => {
      const term = termRef.current;
      const fit = fitRef.current;
      const container = containerRef.current;
      if (!term || !fit || !container) return null;

      const rect = container.getBoundingClientRect();
      if (rect.width < 40 || rect.height < 24) return null;

      const proposed = fit.proposeDimensions();
      if (!proposed) return null;

      const previous = lastLayoutRef.current;
      const widthChanged = !previous || Math.abs(rect.width - previous.width) > 1;
      const heightChanged = !previous || Math.abs(rect.height - previous.height) > 1;

      let cols = proposed.cols;
      let rows = proposed.rows;

      // xterm fit addon 在只改高度时也可能重新算坏列数；我们按方向拆开处理。
      if (previous) {
        if (!widthChanged && heightChanged) {
          cols = term.cols;
        } else if (widthChanged && !heightChanged) {
          rows = term.rows;
        }
      }

      cols = Math.max(cols, 2);
      rows = Math.max(rows, 1);

      if (term.cols !== cols || term.rows !== rows) {
        term.resize(cols, rows);
        term.scrollToBottom();
      }

      lastLayoutRef.current = {
        width: rect.width,
        height: rect.height,
        cols,
        rows,
      };

      if (notifyPty) {
        schedulePtyResize(cols, rows);
      }

      return { cols, rows };
    },
    [schedulePtyResize]
  );

  const scheduleTerminalLayout = useCallback(
    (notifyPty: boolean) => {
      if (resizeFrameRef.current !== null) {
        window.cancelAnimationFrame(resizeFrameRef.current);
      }

      resizeFrameRef.current = window.requestAnimationFrame(() => {
        resizeFrameRef.current = null;
        syncTerminalLayout(notifyPty);
      });
    },
    [syncTerminalLayout]
  );

  useEffect(() => {
    if (!containerRef.current || termRef.current) return;

    const term = new XTerm({
      cursorBlink: true,
      cursorStyle: "bar",
      cursorWidth: 1,
      fontFamily: '"Cascadia Mono", "Consolas", "Courier New", monospace',
      fontSize: 13,
      lineHeight: 1.35,
      theme: {
        background: "#ffffff",
        foreground: "#111827",
        cursor: "#111827",
        selectionBackground: "rgba(59, 130, 246, 0.16)",
        black: "#111827",
        red: "#dc2626",
        green: "#059669",
        yellow: "#b45309",
        blue: "#2563eb",
        magenta: "#7c3aed",
        cyan: "#0891b2",
        white: "#e5e7eb",
        brightBlack: "#6b7280",
        brightRed: "#ef4444",
        brightGreen: "#10b981",
        brightYellow: "#f59e0b",
        brightBlue: "#3b82f6",
        brightMagenta: "#8b5cf6",
        brightCyan: "#06b6d4",
        brightWhite: "#f9fafb",
      },
      scrollback: 3000,
      convertEol: false,
    });

    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(containerRef.current);

    termRef.current = term;
    fitRef.current = fit;
    scheduleTerminalLayout(false);
    term.focus();

    const disposable = term.onData((data) => {
      const current = sessionRef.current;
      if (!current) return;
      if (current.kind === "local") {
        void localPtyWrite(current.handle.pty_id, data);
      } else {
        void sshPtyWrite(current.handle.connection_id, data);
      }
    });

    if (typeof ResizeObserver !== "undefined") {
      const observer = new ResizeObserver(() => {
        scheduleTerminalLayout(true);
      });
      observer.observe(containerRef.current);
      resizeObserverRef.current = observer;
    }

    return () => {
      disposable.dispose();
      resizeObserverRef.current?.disconnect();
      resizeObserverRef.current = null;
      clearScheduledResize();
      stopPolling();
      void closeSession();
      term.dispose();
      termRef.current = null;
      fitRef.current = null;
      lastLayoutRef.current = null;
    };
  }, [clearScheduledResize, closeSession, scheduleTerminalLayout, stopPolling]);

  useEffect(() => {
    if (!termRef.current || !fitRef.current) return;
    if (isRemote && !connectionId) return;

    let cancelled = false;

    const start = async () => {
      setBooting(true);
      await closeSession();
      if (cancelled || !termRef.current) return;

      const term = termRef.current;
      term.reset();

      const dims =
        syncTerminalLayout(false) ?? {
          cols: Math.max(term.cols, 2),
          rows: Math.max(term.rows, 1),
        };

      const cols = Math.max(dims.cols, 2);
      const rows = Math.max(dims.rows, 1);

      try {
        if (isRemote && connectionId) {
          const handle = await sshPtySpawn(connectionId, cols, rows);
          if (cancelled) return;
          sessionRef.current = { kind: "remote", handle };
        } else {
          const handle = await localPtySpawn(cols, rows);
          if (cancelled) {
            await localPtyClose(handle.pty_id).catch(() => {});
            return;
          }
          sessionRef.current = { kind: "local", handle };
        }

        lastSentPtySizeRef.current = { cols, rows };
        scheduleTerminalLayout(false);

        stopPolling();
        pollRef.current = window.setInterval(async () => {
          if (readingRef.current || !sessionRef.current || !termRef.current) return;
          readingRef.current = true;
          try {
            const current = sessionRef.current;
            const payload =
              current.kind === "local"
                ? await localPtyRead(current.handle.pty_id)
                : await sshPtyRead(current.handle.connection_id);
            if (payload.length > 0) {
              termRef.current.write(decoderRef.current.decode(new Uint8Array(payload)));
            }
          } catch {
            // ignore transient poll/read errors
          } finally {
            readingRef.current = false;
          }
        }, 30);
      } catch (error) {
        term.writeln("");
        term.writeln(`Terminal failed: ${formatError(error)}`);
      } finally {
        if (!cancelled) {
          setBooting(false);
          term.focus();
        }
      }
    };

    void start();

    return () => {
      cancelled = true;
    };
  }, [closeSession, connectionId, isRemote, scheduleTerminalLayout, stopPolling, syncTerminalLayout]);

  if (isRemote && !connectionId) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center bg-white text-text-secondary">
        <FontAwesomeIcon icon={["fas", "server"]} className="mb-4 text-4xl text-gray-300" />
        <div className="mb-2 text-[13px]">请先选择一个 SSH 连接</div>
        <div className="text-[12px] text-gray-400">切换到远程模式后，需要先绑定一条可用的远程连接。</div>
      </div>
    );
  }

  return (
    <div className="relative h-full w-full overflow-hidden bg-white">
      {booting && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-white/80 text-[12px] text-text-secondary">
          正在启动终端...
        </div>
      )}
      <div ref={containerRef} className="terminal-surface h-full w-full px-3 py-2" />
      {isRemote && remoteConn && (
        <div className="absolute right-3 top-2 rounded-full bg-white/90 px-2 py-0.5 text-[11px] text-text-secondary shadow-sm">
          {remoteConn.username}@{remoteConn.host}
        </div>
      )}
    </div>
  );
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
