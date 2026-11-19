// Local UI fixture: renders the production timeline, without model requests.
import React, { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import "../../src/i18n";
import "../../src/icons";
import "../../src/styles.css";
import { MessageTurn } from "../../src/components/chat-timeline/MessageTurn";
import { ChatTimeline } from "../../src/components/chat-timeline/ChatTimeline";
import { Button } from "../../src/components/shadcn/button";
import type { Turn } from "../../src/components/chat-timeline/timelineTypes";
import type { ToolCall } from "../../src/types";

const tools: ToolCall[] = [
  { call_id: "skill", name: "skill", args: '{"name":"using-superpowers"}', status: "ok", durationMs: 11,
    output: { id: "using-superpowers", name: "using-superpowers", body: "技能的详细说明保留在详情内，不作为步骤标题。" } },
  { call_id: "read", name: "read_file", args: '{"path":"src/settings.ts","offset":118}', status: "ok", durationMs: 6,
    output: { path: "src/settings.ts", offset: 118, content: "   118\texport const settings = {\n   119\t  title: 'DeepAgent Studio',\n   120\t  theme: 'system',\n   121\t};", truncated: false } },
  { call_id: "edit", name: "edit_file", args: '{"path":"src/settings.ts","old":"system","new":"light"}', status: "ok", durationMs: 23,
    meta: { client_metadata: { file_change: { version: 1, before: "export const settings = {\n  title: 'DeepAgent Studio',\n  theme: 'system',\n};", after: "export const settings = {\n  title: 'DeepAgent Studio',\n  theme: 'light',\n};", start_line: 118 } } }, output: { path: "src/settings.ts", replacements: 1 } },
  { call_id: "build", name: "bash", args: '{"command":"pnpm build"}', status: "ok", durationMs: 1840,
    output: { command: "pnpm build", stdout: "✓ TypeScript 检查通过\n✓ 142 modules transformed.\n✓ built in 1.84s", stderr: "", exit_code: 0 } },
  { call_id: "error", name: "bash", args: '{"command":"pnpm test --filter missing"}', status: "error", durationMs: 83,
    output: JSON.stringify({ command: "pnpm test --filter missing", stdout: "", stderr: "No tests found for filter: missing", exit_code: 1 }) },
  { call_id: "directory", name: "list_dir", args: '{"path":"src"}', status: "ok", output: { path: "src", entries: [{ name: "settings.ts" }, { name: "App.tsx" }] } },
];
function Preview() {
  const [dark, setDark] = useState(false);
  const [processing, setProcessing] = useState(true);
  const [single, setSingle] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    const values = dark ? ["#17181a", "#eceef1", "#9da4b0", "#303238", "#1e2023", "#272a2f"]
      : ["#ffffff", "#20242b", "#727984", "#e5e7eb", "#ffffff", "#f4f5f7"];
    ["bg", "fg", "text-secondary", "border", "elevated", "hover"].forEach((token, i) => document.documentElement.style.setProperty("--theme-" + token, values[i]));
  }, [dark]);
  const turn = useMemo<Turn>(() => ({ activity: { id: "preview-" + revision, phase: processing ? "working" : "settled", generation: 0 },
    runMs: 22400,
    blocks: [
      { kind: "reasoning", id: "reasoning", text: "先检查现有设置的读取路径，确认修改范围。保留工作区中已有的改动，只调整默认主题。" },
      ...tools.slice(0, single ? 1 : tools.length).map(tool => ({ kind: "tool" as const, id: tool.call_id, tool })),
      { kind: "assistant", id: "answer", text: "已调整默认主题。文件读取、代码变更和命令输出都保留在上方的深度探索中。", source: { role: "assistant", content: "" } },
    ] }), [processing, revision, single]);
  return <div style={{ height: "100%", overflow: "auto", padding: "24px" }}>
    <div style={{ maxWidth: 900, margin: "0 auto" }}>
      <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8, marginBottom: 32 }}>
        <span style={{ fontSize: 12, marginRight: "auto" }}>生产组件验证 · 本地示例数据</span>
        <Button variant="ghost" size="sm" onClick={() => setDark(!dark)}>{dark ? "切换浅色" : "切换深色"}</Button>
        <Button variant="ghost" size="sm" onClick={() => { setSingle(!single); setRevision(revision + 1); }}>{single ? "查看多条记录" : "查看单条记录"}</Button>
        <Button variant="ghost" size="sm" onClick={() => { setWaiting(!waiting); setProcessing(true); setRevision(revision + 1); }}>{waiting ? "查看执行过程" : "等待首条输出"}</Button>
        <Button variant="ghost" size="sm" onClick={() => setProcessing(!processing)}>{processing ? "结束回合" : "运行回合"}</Button>
        <Button variant="ghost" size="sm" onClick={() => { setRevision(revision + 1); setProcessing(true); }}>重置</Button>
      </div>
      {waiting ? <ChatTimeline messages={[
        { id: "waiting-user", role: "user", content: "你好", turn: { id: "waiting-" + revision, phase: processing ? "working" : "settled", generation: 0 } },
        { id: "waiting-assistant", role: "assistant", content: "", parts: [] },
      ]} busy={processing} onResend={() => {}} /> : <MessageTurn turn={turn} processing={processing} busy={false} onResend={() => {}} />}
    </div>
  </div>;
}
createRoot(document.getElementById("root")!).render(<React.StrictMode><Preview /></React.StrictMode>);
