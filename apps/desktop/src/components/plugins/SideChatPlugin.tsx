import { useState } from "react";
import { Composer } from "../Composer";
import { useTranslation } from "react-i18next";
import { runChat } from "../../api";
import type { RuntimeEvent } from "../../api";
import type { ChatMessage } from "../../types";

/**
 * A lightweight, self-contained side chat: each submission runs an independent
 * streamed turn via the same `run_chat` backend the main chat uses, rendering
 * tokens into its own message list. (Each turn is its own quick session — this
 * is a scratch/side conversation, not the main project timeline.)
 */
export function SideChatPlugin() {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [running, setRunning] = useState(false);

  const appendAssistant = (delta: string) => {
    setMessages((prev) => {
      const next = [...prev];
      const lastIdx = next.length - 1;
      if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
        next[lastIdx] = { ...next[lastIdx], content: next[lastIdx].content + delta };
      }
      return next;
    });
  };

  const submit = () => {
    const text = value.trim();
    if (!text || running) return;
    setValue("");
    setMessages((prev) => [
      ...prev,
      { role: "user", content: text },
      { role: "assistant", content: "" },
    ]);
    setRunning(true);

    const onEvent = (event: RuntimeEvent) => {
      switch (event.type) {
        case "content_delta":
          appendAssistant(String(event.text ?? ""));
          break;
        case "tool_started":
          appendAssistant(`\n\n🔧 ${String(event.name ?? "tool")}…`);
          break;
        case "tool_completed":
          appendAssistant(event.ok ? " ✓" : " ✗");
          break;
        case "run_failed":
          setMessages((prev) => {
            const next = [...prev];
            const lastIdx = next.length - 1;
            if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
              next[lastIdx] = {
                ...next[lastIdx],
                content:
                  next[lastIdx].content ||
                  `run failed: ${String(event.reason ?? "unknown error")}`,
                tone: "error",
              };
            }
            return next;
          });
          break;
        default:
          break;
      }
    };

    runChat(text, onEvent)
      .catch((err) => {
        setMessages((prev) => {
          const next = [...prev];
          const lastIdx = next.length - 1;
          if (lastIdx >= 0 && next[lastIdx].role === "assistant") {
            next[lastIdx] = { role: "assistant", content: `error: ${String(err)}`, tone: "error" };
          }
          return next;
        });
      })
      .finally(() => setRunning(false));
  };

  return (
    <div className="w-full h-full flex flex-col bg-white">
      {/* Chat Flow */}
      <div className="flex-1 flex flex-col relative">
        <div className="flex-1 overflow-y-auto px-6 py-4 pb-32">
          {messages.length === 0 && (
            <div className="w-full max-w-4xl mx-auto text-text-secondary text-[15px] pl-2">
              {t("chatView.startConversation")}
            </div>
          )}
          {messages.map((m, i) =>
            m.role === "user" ? (
              <div key={i} className="flex flex-col items-end mb-8 w-full max-w-4xl mx-auto">
                <div className="bg-gray-100 text-text-base px-4 py-2.5 rounded-2xl rounded-tr-sm text-[15px] max-w-[80%]">
                  {m.content}
                </div>
              </div>
            ) : (
              <div key={i} className="flex flex-col items-start mb-6 w-full max-w-4xl mx-auto pl-2">
                <div
                  className={`text-[15px] leading-relaxed whitespace-pre-wrap ${
                    m.tone === "error" ? "text-red-500" : "text-text-secondary"
                  }`}
                >
                  {m.content}
                </div>
              </div>
            )
          )}
        </div>

        {/* Composer */}
        <div className="absolute bottom-6 left-0 w-full px-6 flex justify-center">
          <div className="w-full max-w-4xl">
            <Composer
              value={value}
              onChange={setValue}
              onSubmit={submit}
              placeholder={t("chatView.requestFollowUp")}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
