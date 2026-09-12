import { useEffect, useRef, useState } from "react";
import { Send, X, Wrench } from "lucide-react";
import { startChatV2, sshPtyWrite } from "../../api";
import type { RuntimeEvent } from "../../api";

interface FloatingChatProps {
  connectionId: string | null;
  connected: boolean;
}

interface ToolCallDisplay {
  callId: string;
  name: string;
  status: "running" | "done" | "error";
  summary: string;
}

interface ChatMessage {
  role: "user" | "assistant";
  text: string;
  toolCalls?: ToolCallDisplay[];
}

const RUN_TIMEOUT_MS = 120_000;

export function FloatingChat({ connectionId, connected }: FloatingChatProps) {
  const [open, setOpen] = useState(false);
  const [input, setInput] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [sending, setSending] = useState(false);
  const [currentAssistant, setCurrentAssistant] = useState<string>("");
  const [currentTools, setCurrentTools] = useState<ToolCallDisplay[]>([]);
  // Runtime events arrive asynchronously. Keep refs alongside the rendered
  // state so the completion handler never reads a stale React closure.
  const currentAssistantRef = useRef("");
  const currentToolsRef = useRef<ToolCallDisplay[]>([]);
  const failureRef = useRef<string | null>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  const abortRef = useRef(false);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, currentAssistant, currentTools, sending]);

  const handleSend = async () => {
    const text = input.trim();
    if (!text || !connectionId || sending) return;
    setSending(true);
    setInput("");
    setCurrentAssistant("");
    setCurrentTools([]);
    currentAssistantRef.current = "";
    currentToolsRef.current = [];
    failureRef.current = null;
    abortRef.current = false;

    const userMsg: ChatMessage = { role: "user", text };
    setMessages((prev) => [...prev, userMsg]);

    const mod = await import("@tauri-apps/api/event");
    const runId = crypto.randomUUID();

    const unlistenEvent = await mod.listen<unknown>("chat://event", (e) => {
      const raw = e.payload;
      const maybeEnvelope = raw as { run_id?: string; payload?: RuntimeEvent };
      if (maybeEnvelope && typeof maybeEnvelope === "object" && "run_id" in maybeEnvelope && "payload" in maybeEnvelope) {
        if (maybeEnvelope.run_id !== runId) return;
        if (maybeEnvelope.payload) handleRuntimeEvent(maybeEnvelope.payload);
      } else {
        handleRuntimeEvent(raw as RuntimeEvent);
      }
    });

    const unlistenCompleted = await mod.listen<unknown>("session://completed", (e) => {
      const p = e.payload as { run_id?: string; session_id?: string | null; status?: string; error?: string | null };
      if (p && p.run_id === runId) {
        if (p.session_id) sessionIdRef.current = p.session_id;
        if (p.status === "failed") {
          failureRef.current = p.error || "远程 AI 运行失败";
        }
        abortRef.current = true;
      }
    });

    const timeoutId = setTimeout(() => {
      abortRef.current = true;
    }, RUN_TIMEOUT_MS);

    try {
      const ack = await startChatV2(text, {
        sessionId: sessionIdRef.current,
        envMode: "remote",
        connectionId,
        runId,
      });
      if (ack.session_id) sessionIdRef.current = ack.session_id;

      // Wait for completion or timeout
      await new Promise<void>((resolve) => {
        const check = () => {
          if (abortRef.current) {
            resolve();
          } else {
            setTimeout(check, 200);
          }
        };
        check();
      });

      const finalMsg: ChatMessage = {
        role: "assistant",
        text:
          currentAssistantRef.current ||
          failureRef.current ||
          "（AI 未返回内容）",
        toolCalls: currentToolsRef.current.length > 0 ? currentToolsRef.current : undefined,
      };
      setMessages((prev) => [...prev, finalMsg]);
    } catch (err) {
      const errMsg: ChatMessage = {
        role: "assistant",
        text: `Error: ${err instanceof Error ? err.message : String(err)}`,
      };
      setMessages((prev) => [...prev, errMsg]);
    } finally {
      clearTimeout(timeoutId);
      unlistenEvent();
      unlistenCompleted();
      setCurrentAssistant("");
      setCurrentTools([]);
      setSending(false);
    }
  };

  const handleRuntimeEvent = (event: RuntimeEvent) => {
    if (event.type === "session_registered") {
      const sessionId = event.session_id;
      if (typeof sessionId === "string" && sessionId) sessionIdRef.current = sessionId;
    } else if (event.type === "content_delta") {
      const delta = String(event.text ?? event.content ?? event.delta ?? "");
      if (!delta) return;
      currentAssistantRef.current += delta;
      setCurrentAssistant((prev) => prev + delta);
    } else if (event.type === "tool_started") {
      const toolName = (event.tool_name ?? event.name ?? "tool") as string;
      const toolInput = (event.input ?? event.arguments ?? {}) as Record<string, unknown>;
      const callId = String(event.call_id ?? `${toolName}-${currentToolsRef.current.length}`);
      let summary = "";
      if (toolName === "bash" && toolInput.command) {
        summary = String(toolInput.command);
        if (connectionId) sshPtyWrite(connectionId, summary + "\r").catch(() => {});
      } else if (toolName === "read_file" && toolInput.path) {
        summary = String(toolInput.path);
      } else if (toolName === "list_dir" && toolInput.path) {
        summary = String(toolInput.path);
      } else {
        summary = JSON.stringify(toolInput).slice(0, 80);
      }
      const tool = { callId, name: toolName, status: "running" as const, summary };
      currentToolsRef.current = [...currentToolsRef.current, tool];
      setCurrentTools(currentToolsRef.current);
    } else if (event.type === "tool_completed") {
      const callId = String(event.call_id ?? "");
      const ok = event.ok !== false;
      currentToolsRef.current = currentToolsRef.current.map((tool) =>
        (callId && tool.callId === callId) || (!callId && tool.status === "running")
          ? { ...tool, status: ok ? "done" : "error" }
          : tool,
      );
      setCurrentTools(currentToolsRef.current);
    } else if (event.type === "run_completed" || event.type === "run_failed") {
      if (event.type === "run_failed" && !failureRef.current) {
        failureRef.current = String(event.reason ?? "远程 AI 运行失败");
      }
      abortRef.current = true;
    }
  };

  if (!connected) return null;

  return (
    <>
      {/* FAB */}
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="fixed bottom-6 right-6 z-50 flex h-12 w-12 items-center justify-center rounded-full bg-white shadow-lg transition-transform hover:scale-105 active:scale-95"
        title="远程 AI 助手"
      >
        <img src="/logo.png" alt="logo" className="h-7 w-7" />
      </button>

      {/* Backdrop */}
      {open && (
        <div className="fixed inset-0 z-50 bg-black/20" onClick={() => setOpen(false)} />
      )}

      {/* Drawer */}
      <div
        className={`fixed bottom-4 right-4 z-50 flex w-[380px] flex-col overflow-hidden rounded-xl bg-white shadow-2xl transition-transform duration-300 ${
          open ? "translate-y-0 opacity-100" : "translate-y-4 opacity-0 pointer-events-none"
        }`}
        style={{ height: "min(480px, 50vh)" }}
      >
        {/* Header */}
        <div className="flex shrink-0 items-center justify-between border-b border-gray-100 px-4 py-3">
          <div className="flex items-center gap-2">
            <img src="/logo.png" alt="logo" className="h-5 w-5" />
            <span className="text-[14px] font-medium">远程 AI 助手</span>
          </div>
          <button
            type="button"
            onClick={() => setOpen(false)}
            className="flex h-7 w-7 items-center justify-center rounded-full hover:bg-black/5"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        {/* Messages */}
        <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3 space-y-3">
          {messages.length === 0 && !sending && (
            <div className="rounded-lg bg-gray-50 px-3 py-2 text-[13px] text-gray-500">
              描述你想在远程服务器上做什么，AI 会执行工具并返回结果
            </div>
          )}
          {messages.map((msg, i) => (
            <MessageBubble key={i} msg={msg} />
          ))}
          {(sending || currentAssistant || currentTools.length > 0) && (
            <div className="flex justify-start">
              <div className="max-w-[85%] rounded-lg bg-gray-100 px-3 py-2 text-[13px] text-gray-800">
                {currentTools.length > 0 && (
                  <div className="space-y-1 mb-1.5">
                    {currentTools.map((tc, j) => (
                      <div key={j} className="flex items-center gap-1.5 text-[12px]">
                        <Wrench className="h-3 w-3 text-blue-500" />
                        <span className="text-blue-600 font-mono truncate max-w-[200px]">
                          {tc.name}
                        </span>
                        <span className="text-gray-400 truncate max-w-[120px]">
                          {tc.summary}
                        </span>
                        {tc.status === "running" && (
                          <span className="text-yellow-500">⏳</span>
                        )}
                        {tc.status === "done" && (
                          <span className="text-green-500">✓</span>
                        )}
                      </div>
                    ))}
                  </div>
                )}
                {currentAssistant && (
                  <div className="whitespace-pre-wrap break-words">{currentAssistant}</div>
                )}
                {!currentAssistant && currentTools.length === 0 && (
                  <span className="text-gray-400">思考中…</span>
                )}
              </div>
            </div>
          )}
          <div ref={messagesEndRef} />
        </div>

        {/* Input */}
        <div className="flex shrink-0 items-center gap-2 border-t border-gray-100 px-4 py-3">
          <input
            type="text"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void handleSend();
              }
            }}
            placeholder="描述你想做什么…"
            className="flex-1 rounded-lg border border-gray-200 px-3 py-2 text-[13px] outline-none focus:border-gray-400"
          />
          <button
            type="button"
            onClick={() => void handleSend()}
            disabled={!input.trim() || sending}
            className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-black text-white transition-colors hover:bg-gray-800 disabled:bg-gray-300"
          >
            <Send className="h-4 w-4" />
          </button>
        </div>
      </div>
    </>
  );
}

function MessageBubble({ msg }: { msg: ChatMessage }) {
  return (
    <div className={`flex ${msg.role === "user" ? "justify-end" : "justify-start"}`}>
      <div
        className={`max-w-[85%] rounded-lg px-3 py-2 text-[13px] ${
          msg.role === "user"
            ? "bg-black text-white"
            : "bg-gray-100 text-gray-800"
        }`}
      >
        {msg.toolCalls && msg.toolCalls.length > 0 && (
          <div className="space-y-1 mb-1.5">
            {msg.toolCalls.map((tc, j) => (
              <div key={j} className="flex items-center gap-1.5 text-[12px]">
                <Wrench className="h-3 w-3 text-blue-500" />
                <span className="text-blue-600 font-mono truncate max-w-[200px]">
                  {tc.name}
                </span>
                <span className="text-gray-400 truncate max-w-[120px]">
                  {tc.summary}
                </span>
                <span className="text-green-500">✓</span>
              </div>
            ))}
          </div>
        )}
        {msg.text && (
          <div className="whitespace-pre-wrap break-words">{msg.text}</div>
        )}
      </div>
    </div>
  );
}
