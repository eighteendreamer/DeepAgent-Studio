// Manual gate for the production answer editor and event projection. Synthetic
// IPC only; the separate Rust seam gate executes these contracts in the Kernel.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { Button } from "../src/components/shadcn/button";
import type { ProfessionalNodeData } from "../src/canvas/workflow/types";
import "../src/styles.css";

mockWindows("h182-verification");
let runId = "";
let submitted: { definition: { nodes: { id: string; config: Record<string, unknown> }[] } } | undefined;
const unexpected: string[] = [];
mockIPC((command, args) => {
  if (command === "start_workflow") {
    runId = "h182-browser";
    submitted = (args as { workflow: typeof submitted }).workflow;
    return { run_id: runId, session_id: null };
  }
  if (command === "canvas_preference_read") {
    const key = (args as { key: string }).key;
    if (key === "workspace-dirs") return {};
    if (["creative-library", "workflow-snippets", "video-edit-projects"].includes(key)) return [];
  }
  if (command === "canvas_settings_read") return { providers: [], bindings: [] };
  unexpected.push(command);
  throw new Error(`Unexpected verification IPC: ${command}`);
}, { shouldMockEvents: true });
const [{ SchemaConfigForm }, { useProfessionalStore }, { useCanvasStore }, executor, { createDefaultNodeData, getNodeConfigIssues }] = await Promise.all([
  import("../src/canvas/workflow/components/SchemaConfigForm"),
  import("../src/canvas/workflow/store/professionalStore"),
  import("../src/canvas/workflow/store/canvasStore"),
  import("../src/canvas/workflow/utils/workflowExecutor"),
  import("../src/canvas/workflow/utils/nodeRegistry"),
]);
useCanvasStore.setState({ mode: "professional" });
useProfessionalStore.setState({ nodes: [
  { id: "start", type: "professional-start", position: { x: 0, y: 0 }, data: createDefaultNodeData("start") },
  { id: "answer", type: "professional-answer", position: { x: 0, y: 0 }, data: createDefaultNodeData("answer") },
  { id: "end", type: "professional-end", position: { x: 0, y: 0 }, data: createDefaultNodeData("end") },
], edges: [{ id: "a", source: "start", target: "answer" }, { id: "b", source: "answer", target: "end" }] });

function Verification() {
  const data = useProfessionalStore((state) => state.nodes.find((node) => node.id === "answer")!.data as ProfessionalNodeData);
  const [result, setResult] = useState("");
  async function verify() {
    try {
      if (getNodeConfigIssues("answer", data).length) throw new Error("模板尚未通过生产schema校验");
      void executor.runWorkflow();
      for (let i = 0; !submitted && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10));
      const config = submitted?.definition.nodes.find((node) => node.id === "answer")?.config;
      if (config?.answerTemplate !== "你好 Eighteen" || Object.hasOwn(config, "text")) throw new Error("生产serializer未保留模板契约");
      await emit("chat://event", { run_id: runId, payload: { type: "workflow_node", event: {
        revision: "h182-browser-synthetic", node_id: "answer", scope: [], status: "completed", attempt: 1, elapsed_ms: 2,
        outputs: { answer: "你好 Eighteen" },
      } } });
      await emit("session://completed", { run_id: runId });
      if (unexpected.length) throw new Error(unexpected.join(","));
      setResult("PASS：生产编辑器 → serializer → 合成事件消费者；非真实后端验收");
    } catch (error) { setResult(String(error)); }
  }
  return <main style={{ minHeight: "100vh", background: "#101318", color: "#eee", padding: 24 }}>
    <h1>H-182 · 直接回答</h1>
    <p>先确认空模板提示，再在回答模板填写「你好 Eighteen」，最后执行合成事件验收。</p>
    <section style={{ maxWidth: 680, padding: 20, border: "1px solid #444", borderRadius: 16 }}>
      <SchemaConfigForm nodeId="answer" data={data} onUpdate={(patch) => useProfessionalStore.getState().updateNodeData("answer", patch)} />
    </section>
    <Button variant="secondary" onClick={() => void verify()}>验证已填写模板与结果展示</Button>
    <p role="status">{result || "等待操作"}</p>
    <pre>{JSON.stringify({ template: data.answerTemplate, status: data.status, result: data.result, unexpectedIPC: unexpected }, null, 2)}</pre>
  </main>;
}
createRoot(document.getElementById("root")!).render(<Verification />);
