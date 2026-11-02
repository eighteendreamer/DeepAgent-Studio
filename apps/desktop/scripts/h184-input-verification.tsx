// Manual production editor/serializer/consumer gate. Synthetic IPC only;
// workflow_audit::inputs separately executes these contracts in the real Kernel.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { Button } from "../src/components/shadcn/button";
import type { ProfessionalNodeData } from "../src/canvas/workflow/types";
import "../src/styles.css";

mockWindows("h184-verification");
type Submitted = { inputs: Record<string, unknown>; definition: { nodes: { id: string; config: Record<string, unknown> }[] } };
let submitted: Submitted | undefined;
const unexpected: string[] = [];
mockIPC((command, args) => {
  if (command === "start_workflow") {
    submitted = (args as { workflow: Submitted }).workflow;
    return { run_id: "h184-browser", session_id: null };
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
const [{ SchemaConfigForm }, { useProfessionalStore }, { useCanvasStore }, executor, { createDefaultNodeData }] = await Promise.all([
  import("../src/canvas/workflow/components/SchemaConfigForm"),
  import("../src/canvas/workflow/store/professionalStore"),
  import("../src/canvas/workflow/store/canvasStore"),
  import("../src/canvas/workflow/utils/workflowExecutor"),
  import("../src/canvas/workflow/utils/nodeRegistry"),
]);
useCanvasStore.setState({ mode: "professional" });
useProfessionalStore.setState({ nodes: [
  { id: "start", type: "professional-start", position: { x: 0, y: 0 }, data: { ...createDefaultNodeData("start"), inputVariables: [{ name: "value", type: "string", required: true, default: "BEFORE", options: ["BEFORE", "AFTER"], minLength: 5, maxLength: 6 }] } },
  { id: "end", type: "professional-end", position: { x: 0, y: 0 }, data: createDefaultNodeData("end") },
], edges: [{ id: "a", source: "start", target: "end" }] });

function Verification() {
  const data = useProfessionalStore((state) => state.nodes[0].data as ProfessionalNodeData);
  const [result, setResult] = useState("等待操作");
  async function verifyRejection() {
    submitted = undefined;
    const running = executor.runWorkflow();
    for (let i = 0; !submitted && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10));
    const error = "MissingWorkflowInput: required field value";
    await emit("session://completed", { run_id: "h184-browser", status: "failed", error });
    await running;
    const current = useProfessionalStore.getState().nodes[0].data as ProfessionalNodeData;
    setResult(current.status === "error" && current.errorMessage === error && !unexpected.length
      ? "PASS：无节点事件的准入失败仍显示错误；合成 completion，不是真实后端验收"
      : "FAIL：准入错误被静默吞掉");
  }
  async function verify() {
    try {
      submitted = undefined;
      const running = executor.runWorkflow();
      for (let i = 0; !submitted && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10));
      const request = submitted as Submitted | undefined;
      const declarations = request?.definition.nodes[0].config.inputVariables as { default?: unknown }[] | undefined;
      if (!request || Object.keys(request.inputs).length || declarations?.[0].default !== "AFTER") throw new Error("请先把默认值改为 JSON 字符串 AFTER；inputs 必须为空");
      await emit("chat://event", { run_id: "h184-browser", payload: { type: "workflow_node", event: {
        revision: "h184-browser-synthetic", node_id: "start", scope: [], status: "completed", attempt: 1, elapsed_ms: 1,
        outputs: { value: "AFTER" }, updates: { inputs: { version: 1, revision: "h184-browser-synthetic", sources: { value: "default" } } },
      } } });
      await emit("session://completed", { run_id: "h184-browser" });
      await running;
      const current = useProfessionalStore.getState().nodes[0].data as ProfessionalNodeData;
      const projected = current.result as { outputs?: { value?: unknown }; updates?: unknown } | undefined;
      if (current.status !== "completed" || projected?.outputs?.value !== "AFTER" || unexpected.length) throw new Error("事件投影或IPC边界错误");
      setResult("PASS：生产编辑器 → serializer(inputs={}) → 合成事件消费者；非真实后端验收");
    } catch (error) { setResult(String(error)); }
  }
  return <main style={{ minHeight: "100vh", background: "#101318", color: "#eee", padding: 24 }}>
    <h1>H-184～190 · 输入契约</h1>
    <p>将默认值从 JSON 字符串 BEFORE 改成 AFTER，检查类型/枚举/Unicode 长度说明后点击验证。</p>
    <section style={{ maxWidth: 680, padding: 20, border: "1px solid #444", borderRadius: 16 }}>
      <SchemaConfigForm nodeId="start" data={data} onUpdate={(patch) => useProfessionalStore.getState().updateNodeData("start", patch)} />
    </section>
    <Button variant="secondary" onClick={() => void verify()}>验证默认值与输入来源</Button>
    <Button variant="secondary" onClick={() => void verifyRejection()}>验证无节点事件的准入拒绝</Button>
    <p role="status">{result}</p>
    <pre>{JSON.stringify({ declarations: data.inputVariables, inputs: submitted?.inputs, status: data.status, result: data.result, unexpectedIPC: unexpected }, null, 2)}</pre>
  </main>;
}
createRoot(document.getElementById("root")!).render(<Verification />);
