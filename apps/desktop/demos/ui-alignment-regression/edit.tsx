// Local fixture: real timeline, scrolling and edit controls; no model request.
import React, { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/i18n";
import "../../src/icons";
import "../../src/styles.css";
import { ThemeProvider } from "../../src/theme/ThemeProvider";
import { Button } from "../../src/components/shadcn/button";
import { ChatTimeline } from "../../src/components/chat-timeline/ChatTimeline";
import { useTimelineScroll } from "../../src/components/chat-timeline/useTimelineScroll";
import type { ChatMessage } from "../../src/types";

function Fixture() {
  const [long, setLong] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState("");
  const scroll = useRef<HTMLDivElement>(null), content = useRef<HTMLDivElement>(null);
  const messages: ChatMessage[] = [
    { id: "edit-user", role: "user", content: long ? "长消息与换行验证。\n".repeat(30) : "你是谁",
      selectedSkills: [{ id: "fixture-skill", name: "验证技能" }], mentions: [{ kind: "plan_mode" }] },
    { id: "edit-assistant", role: "assistant", content: "我是 DeepAgent。这段回答在编辑过程中应保持可读，位置只随编辑框高度平滑变化。" },
  ];
  useTimelineScroll(scroll, content, long, 24);
  return <div className="flex h-full flex-col gap-4 p-4">
    <div className="flex flex-wrap gap-2">
      <Button onClick={() => setLong(false)}>短消息</Button>
      <Button onClick={() => setLong(true)}>长消息</Button>
      <Button onClick={() => setBusy(value => !value)}>{busy ? "解除忙碌" : "模拟忙碌"}</Button>
    </div>
    <output data-resend-result className="break-all text-xs">{result}</output>
    <div ref={scroll} data-chat-scroll className="min-h-0 flex-1 overflow-y-auto p-4" style={{ overflowAnchor: "none" }}>
      <div ref={content}>
        <ChatTimeline key={String(long)} messages={messages} busy={busy} scrollContainerRef={scroll}
          onResend={(text, skills, mentions) => setResult(JSON.stringify({ text, skills, mentions }))} />
      </div>
    </div>
  </div>;
}
const root = createRoot(document.getElementById("root")!);
import.meta.hot?.dispose(() => root.unmount());
root.render(<React.StrictMode><ThemeProvider><Fixture /></ThemeProvider></React.StrictMode>);
