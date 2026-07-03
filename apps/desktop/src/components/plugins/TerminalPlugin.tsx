import { useEffect, useRef, useState, useCallback } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import {
  runTerminal,
  terminalCwd,
  sshExec,
  sshStatus,
  sshPtySpawn,
  sshPtyWrite,
  sshPtyRead,
  type SshConnection,
  type SshPtyHandle,
} from "../../api";

interface Line {
  kind: "prompt" | "stdout" | "stderr" | "system";
  text: string;
}

interface TerminalPluginProps {
  mode?: "local" | "remote";
  connectionId?: string | null;
}

export function TerminalPlugin({ mode = "local", connectionId = null }: TerminalPluginProps) {
  const { t } = useTranslation();
  const [cwd, setCwd] = useState("");
  const [lines, setLines] = useState<Line[]>([]);
  const [input, setInput] = useState("");
  const [running, setRunning] = useState(false);
  const [remoteConn, setRemoteConn] = useState<SshConnection | null>(null);
  const [ptyHandle, setPtyHandle] = useState<SshPtyHandle | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const ptyPollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const isRemote = mode === "remote";

  useEffect(() => {
    if (isRemote && connectionId) {
      sshStatus(connectionId).then(setRemoteConn).catch(() => setRemoteConn(null));
    } else {
      setRemoteConn(null);
    }
  }, [isRemote, connectionId]);

  useEffect(() => {
    if (!isRemote) {
      terminalCwd().then(setCwd).catch(() => {});
    }
  }, [isRemote]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [lines, running]);

  // Spawn PTY on first remote command
  const ensurePty = useCallback(async () => {
    if (ptyHandle || !connectionId) return null;
    const handle = await sshPtySpawn(connectionId, 80, 24);
    setPtyHandle(handle);
    return handle;
  }, [ptyHandle, connectionId]);

  // Poll PTY output when a PTY session is active
  useEffect(() => {
    if (ptyHandle && isRemote) {
      ptyPollRef.current = setInterval(async () => {
        try {
          const data = await sshPtyRead(ptyHandle.connection_id);
          if (data.length > 0) {
            const text = new TextDecoder().decode(new Uint8Array(data));
            setLines((prev) => [...prev, { kind: "stdout", text }]);
          }
        } catch {
          // ignore poll errors (e.g. PTY not yet spawned)
        }
      }, 100);
      return () => {
        if (ptyPollRef.current) clearInterval(ptyPollRef.current);
      };
    }
  }, [ptyHandle, isRemote]);

  const promptLabel = isRemote
    ? remoteConn
      ? `PS ${remoteConn.username}@${remoteConn.host}:${cwd || "~"}>`
      : "PS [remote]>"
    : `PS ${cwd || "…"}>`;

  const submit = useCallback(async () => {
    const cmd = input.trim();
    if (!cmd || running) return;

    if (isRemote && !connectionId) {
      setLines((prev) => [...prev, { kind: "system", text: "请先在设置中配置 SSH 连接" }]);
      return;
    }

    setInput("");
    setLines((prev) => [...prev, { kind: "prompt", text: `${promptLabel} ${cmd}` }]);
    setRunning(true);

    try {
      if (isRemote && connectionId) {
        // Use PTY streaming for interactive remote commands
        const handle = await ensurePty();
        if (handle) {
          await sshPtyWrite(handle.connection_id, cmd + "\n");
        } else {
          // Fallback to one-shot exec
          const res = await sshExec(connectionId, cmd);
          setLines((prev) => {
            const next = [...prev];
            if (res.stdout) next.push({ kind: "stdout", text: res.stdout.replace(/\n$/, "") });
            if (res.stderr) next.push({ kind: "stderr", text: res.stderr.replace(/\n$/, "") });
            return next;
          });
        }
      } else {
        const res = await runTerminal(cmd);
        setLines((prev) => {
          const next = [...prev];
          if (res.stdout) next.push({ kind: "stdout", text: res.stdout.replace(/\n$/, "") });
          if (res.stderr) next.push({ kind: "stderr", text: res.stderr.replace(/\n$/, "") });
          return next;
        });
        if (res.cwd) setCwd(res.cwd);
      }
    } catch (e) {
      setLines((prev) => [...prev, { kind: "stderr", text: String(e) }]);
    } finally {
      setRunning(false);
      inputRef.current?.focus();
    }
  }, [input, running, isRemote, connectionId, promptLabel, ensurePty]);

  if (isRemote && !connectionId) {
    return (
      <div className="w-full h-full flex flex-col items-center justify-center bg-white text-text-secondary">
        <FontAwesomeIcon icon={["fas", "server"]} className="text-4xl mb-4 text-gray-300" />
        <div className="text-[13px] mb-2">请先在设置中配置 SSH 连接</div>
        <div className="text-[12px] text-gray-400">切换到远程模式后，需要选择一个已配置的 SSH 连接</div>
      </div>
    );
  }

  return (
    <div className="w-full h-full flex flex-col bg-white">
      <div className="flex-1 flex overflow-hidden">
        <div
          ref={scrollRef}
          className="flex-1 overflow-y-auto bg-white p-4 font-mono text-[13px] leading-relaxed text-text-base"
          onClick={() => inputRef.current?.focus()}
        >
          <div className="mb-4">
            {isRemote
              ? `SSH: ${remoteConn?.username}@${remoteConn?.host}:${remoteConn?.port || 22}`
              : "Windows PowerShell"}
            <br />
            {t("plugins.terminal.copyright")}
          </div>

          {lines.map((line, i) => (
            <div
              key={i}
              className={`whitespace-pre-wrap ${
                line.kind === "stderr"
                  ? "text-red-500"
                  : line.kind === "prompt"
                  ? "text-text-base"
                  : line.kind === "system"
                  ? "text-yellow-600"
                  : "text-text-secondary"
              }`}
            >
              {line.text}
            </div>
          ))}

          <div className="flex items-center">
            <span className="mr-2 flex-shrink-0">{promptLabel}</span>
            <input
              ref={inputRef}
              type="text"
              value={input}
              autoFocus
              disabled={running}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") submit();
              }}
              className="flex-1 bg-transparent outline-none border-none font-mono text-[13px] text-text-base"
            />
            {running && <span className="ml-2 text-text-secondary">…</span>}
          </div>
        </div>

        <div className="w-4 border-l border-border-theme flex flex-col items-center py-2 text-text-secondary flex-shrink-0 bg-gray-50">
          <FontAwesomeIcon icon={["fas", "caret-up"]} className="text-[10px] cursor-pointer" />
          <div className="flex-1 w-full flex justify-center py-1">
            <div className="w-1.5 h-10 bg-gray-300 rounded-full"></div>
          </div>
          <FontAwesomeIcon icon={["fas", "caret-down"]} className="text-[10px] cursor-pointer" />
        </div>
      </div>
    </div>
  );
}
