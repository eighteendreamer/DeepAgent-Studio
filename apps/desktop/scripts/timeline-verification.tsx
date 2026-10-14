/// <reference types="vite/client" />
// Local verification fixture. No model requests, persistence or production routes.
import React, { useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import "../src/i18n";
import "../src/icons";
import "../src/styles.css";
import { Button } from "../src/components/shadcn/button";
import { ChatTimeline } from "../src/components/chat-timeline/ChatTimeline";
import { ProcessStoreContext } from "../src/components/chat-timeline/ProcessDisclosureContext";
import { advanceTurnActivity, ProcessDisclosureStore, type TurnActivity } from "../src/components/chat-timeline/processDisclosure";
import { useTimelineScroll } from "../src/components/chat-timeline/useTimelineScroll";
import type { ChatMessage } from "../src/types";

function Fixture() {
  const [store] = useState(() => new ProcessDisclosureStore());
  const [activity, setActivity] = useState<TurnActivity>({ id: "fixture-live", phase: "working", generation: 0 });
  const [count, setCount] = useState(1);
  const [mounted, setMounted] = useState(true);
  const [tokens, setTokens] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const container = useRef<HTMLDivElement>(null);
  const content = useRef<HTMLDivElement>(null);
  const update = (phase: TurnActivity["phase"]) => setActivity(previous => advanceTurnActivity(previous, phase)!);
  const settle = (label: string) => setActivity(previous => ({ ...advanceTurnActivity(previous, "settled")!, finalAnswer: `本轮${label}，执行过程统一收起。` }));
  const messages: ChatMessage[] = [];
  for (let index = 0; index < count - 1; index++) {
    const turn: TurnActivity = { id: `history-${index}`, phase: "settled", generation: 0 };
    messages.push({ id: `u-${index}`, role: "user", content: `历史问题 ${index}`, turn });
    messages.push({ id: `a-${index}`, role: "assistant", content: `历史答案 ${index}`, turn, parts: [
      { kind: "reasoning", text: "历史思考详情" },
      { kind: "tool", tool: { name: "shell", call_id: `history-tool-${index}`, args: "pwd", status: "error", detail: "失败后已结束" } },
      { kind: "text", text: `历史答案 ${index}\n\n` + "用于检查滚动锚点和虚拟列表。".repeat(20) },
    ] });
  }
  messages.push({ id: "live-user", role: "user", content: "验证执行过程自动收起", turn: activity });
  messages.push({ id: "live-assistant", role: "assistant", content: "", turn: activity, runMs: activity.phase === "settled" ? 8800 : undefined, parts: [
    { kind: "reasoning", text: "执行期间的思考。\n\n[详情内焦点](#focus)" },
    { kind: "tool", tool: { name: "shell", call_id: "fixture-tool", args: "pwd", status: "error", detail: "工具失败，但这不应阻止结束后收起" } },
    ...(activity.phase !== "working" ? [{ kind: "text" as const, text: activity.finalAnswer ?? `总结正在输出 ${"追加文本。".repeat(tokens)}` }] : []),
  ] });
  useTimelineScroll(container, content, `${activity.generation}:${tokens}:${count}:${mounted}`, 24);
  return <ProcessStoreContext.Provider value={store}>
    <div className="mx-auto max-w-5xl p-5">
      <h1 className="mb-3 text-lg font-semibold">执行过程折叠验证 · 固定场景</h1>
      <div className="mb-3 flex flex-wrap gap-2">
        <Button onClick={() => update("answering")}>输出总结</Button>
        <Button onClick={() => update("working")}>继续执行</Button>
        {["成功", "失败", "取消", "阻塞", "预算耗尽"].map(label => <Button key={label} variant="outline" onClick={() => settle(label)}>{label}结束</Button>)}
        <Button variant="outline" onClick={() => { store.setOpen("fixture-live", false); setActivity({ id: "fixture-live-" + Date.now(), phase: "working", generation: 0 }); }}>新一轮</Button>
        {[48, 49, 52].map(value => <Button key={value} variant="outline" onClick={() => setCount(value)}>{value}轮</Button>)}
        <Button variant="outline" onClick={() => setTokens(value => value + 30)}>追加文本</Button>
        <Button variant="outline" onClick={() => setMounted(value => !value)}>切换视图</Button>
        <Button variant="outline" onClick={() => { setTimeout(() => settle("失败"), 1500); }}>稍后结束</Button>
      </div>
      <p className="mb-2 text-sm">阶段：{activity.phase} · 轮数：{count} · 滚动位置：{Math.round(scrollTop)}</p>
      <div ref={container} onScroll={() => setScrollTop(container.current?.scrollTop ?? 0)} className="overflow-y-auto rounded border p-4" style={{ height: 520, overflowAnchor: "none", paddingBottom: 24 }}>
        <div ref={content}>{mounted && <ChatTimeline messages={messages} busy={activity.phase !== "settled"} scrollContainerRef={container} onResend={() => {}} />}</div>
      </div>
    </div>
  </ProcessStoreContext.Provider>;
}
const verificationRoot = createRoot(document.getElementById("root")!);
import.meta.hot?.dispose(() => verificationRoot.unmount());
verificationRoot.render(<React.StrictMode><Fixture /></React.StrictMode>);
