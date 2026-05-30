import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import { runTerminal, terminalCwd } from "../../api";

interface Line {
  kind: "prompt" | "stdout" | "stderr";
  text: string;
}

export function TerminalPlugin() {
  const { t } = useTranslation();
  const [cwd, setCwd] = useState("");
  const [lines, setLines] = useState<Line[]>([]);
  const [input, setInput] = useState("");
  const [running, setRunning] = useState(false);
  const scrollRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    terminalCwd().then(setCwd).catch(() => {});
  }, []);

  useEffect(() => {
    // Keep the view scrolled to the latest output.
    scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight });
  }, [lines, running]);

  const promptLabel = `PS ${cwd || "…"}>`;

  const submit = async () => {
    const cmd = input.trim();
    if (!cmd || running) return;
    setInput("");
    setLines((prev) => [...prev, { kind: "prompt", text: `${promptLabel} ${cmd}` }]);
    setRunning(true);
    try {
      const res = await runTerminal(cmd);
      setLines((prev) => {
        const next = [...prev];
        if (res.stdout) next.push({ kind: "stdout", text: res.stdout.replace(/\n$/, "") });
        if (res.stderr) next.push({ kind: "stderr", text: res.stderr.replace(/\n$/, "") });
        return next;
      });
      if (res.cwd) setCwd(res.cwd);
    } catch (e) {
      setLines((prev) => [...prev, { kind: "stderr", text: String(e) }]);
    } finally {
      setRunning(false);
      inputRef.current?.focus();
    }
  };

  return (
    <div className="w-full h-full flex flex-col bg-white">
      {/* Terminal View */}
      <div className="flex-1 flex overflow-hidden">
        <div
          ref={scrollRef}
          className="flex-1 overflow-y-auto bg-white p-4 font-mono text-[13px] leading-relaxed text-text-base"
          onClick={() => inputRef.current?.focus()}
        >
          <div className="mb-4">
            Windows PowerShell<br />
            {t("plugins.terminal.copyright")}
          </div>

          {/* History */}
          {lines.map((line, i) => (
            <div
              key={i}
              className={`whitespace-pre-wrap ${
                line.kind === "stderr"
                  ? "text-red-500"
                  : line.kind === "prompt"
                  ? "text-text-base"
                  : "text-text-secondary"
              }`}
            >
              {line.text}
            </div>
          ))}

          {/* Active input line */}
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

        {/* Scrollbar placeholder right side */}
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
