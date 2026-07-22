import { createRoot, type Root } from "react-dom/client";
import { useEffect, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";

// A dependency-free, imperative toast/message API (antd-style):
//   message.error("..."), message.success("..."), message.info("..."),
//   message.warning("...").
// It lazily mounts a single fixed container at the top-center of the window and
// renders an animated stack of auto-dismissing notices.

type MessageType = "success" | "error" | "info" | "warning";

interface Notice {
  id: number;
  type: MessageType;
  text: string;
  duration: number;
}

const TYPE_META: Record<MessageType, { icon: IconProp; color: string }> = {
  success: { icon: ["fas", "circle-check"], color: "text-green-500" },
  error: { icon: ["fas", "circle-info"], color: "text-red-500" },
  info: { icon: ["fas", "circle-info"], color: "text-blue-500" },
  warning: { icon: ["fas", "circle-info"], color: "text-amber-500" },
};

// Module-level bridge so the imperative API can push into the mounted host.
let pushNotice: ((n: Omit<Notice, "id">) => void) | null = null;
let seq = 0;
let root: Root | null = null;

function ensureHost() {
  if (root) return;
  const el = document.createElement("div");
  el.id = "message-root";
  document.body.appendChild(el);
  root = createRoot(el);
  root.render(<MessageHost />);
}

function MessageHost() {
  const [notices, setNotices] = useState<Notice[]>([]);

  useEffect(() => {
    pushNotice = (n) => {
      const id = ++seq;
      setNotices((prev) => [...prev, { ...n, id }]);
      window.setTimeout(() => {
        setNotices((prev) => prev.filter((x) => x.id !== id));
      }, n.duration);
    };
    return () => {
      pushNotice = null;
    };
  }, []);

  return (
    <div className="fixed top-4 inset-x-0 z-[9999] flex flex-col items-center gap-2 pointer-events-none">
      {notices.map((n) => {
        const meta = TYPE_META[n.type];
        return (
          <div
            key={n.id}
            className="message-notice pointer-events-auto flex items-center gap-2.5 bg-white border border-border-theme rounded-lg shadow-[0_6px_24px_rgb(0,0,0,0.12)] px-4 py-2.5 max-w-[480px]"
          >
            <FontAwesomeIcon icon={meta.icon} className={`${meta.color} text-sm`} />
            <span className="text-[13px] text-text-base">{n.text}</span>
          </div>
        );
      })}
    </div>
  );
}

function show(type: MessageType, text: string, duration = 3000) {
  ensureHost();
  // Defer one tick so a freshly-created host has registered its pusher.
  if (pushNotice) {
    pushNotice({ type, text, duration });
  } else {
    setTimeout(() => pushNotice?.({ type, text, duration }), 0);
  }
}

export const message = {
  success: (text: string, duration?: number) => show("success", text, duration),
  error: (text: string, duration?: number) => show("error", text, duration),
  info: (text: string, duration?: number) => show("info", text, duration),
  warning: (text: string, duration?: number) => show("warning", text, duration),
};
