// Manual gate for the production aggregator editor and event projection. Synthetic
// IPC only; the separate Rust seam gate executes these contracts in the Kernel.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { Button } from "../src/components/shadcn/button";
import type { ProfessionalNodeData } from "../src/canvas/workflow/types";
import "../src/styles.css";

mockWindows("h183-verification");
let runId = "";
let submitted: { definition: { nodes: { id: string; config: Record<string, unknown> }[] } } | undefined;
const unexpected: string[] = [];
mockIPC((command, args) => {
  if (command === "start_workflow") {
    runId = "h183-browser";
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
  { id: "start", type: "professional-start", position: { x: 0, y: 0 }, data: { ...createDefaultNodeData("start"), inputVariables: [{ name: "first", type: "string", default: "FIRST" }, { name: "second", type: "string", default: "SECOND" }] } },
  { id: "aggregate", type: "professional-variable-aggregator", position: { x: 0, y: 0 }, data: { ...createDefaultNodeData("variable-aggregator"), aggregatorOutputType: "array" } },
  { id: "end", type: "professional-end", position: { x: 0, y: 0 }, data: createDefaultNodeData("end") },
], edges: [{ id: "a", source: "start", target: "aggregate" }, { id: "b", source: "aggregate", target: "end" }] });

function Verification() {
  const data = useProfessionalStore((state) => state.nodes.find((node) => node.id === "aggregate")!.data as ProfessionalNodeData);
  const [result, setResult] = useState("");
  async function verify() {
    try {
      if (getNodeConfigIssues("variable-aggregator", data).length) throw new Error("绑定尚未通过生产schema校验");
      const running = executor.runWorkflow();
      for (let i = 0; !submitted && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10));
      const config = submitted?.definition.nodes.find((node) => node.id === "aggregate")?.config;
      if (config?.aggregatorOutputType !== "array" || JSON.stringify(config.aggregatorVariables) !== JSON.stringify(["{{#start.first#}}", "{{#start.second#}}"] ) || Object.hasOwn(config, "variables")) throw new Error("生产serializer未保留绑定契约");
      await emit("chat://event", { run_id: runId, payload: { type: "workflow_node", event: {
        revision: "h183-browser-synthetic", node_id: "aggregate", scope: [], status: "completed", attempt: 1, elapsed_ms: 2,
        outputs: { output: ["FIRST", "SECOND"] },
        updates: { aggregation: { version: 1, outputType: "array", configuredCount: 2, selectedCount: 2, selectedSources: [["start", "first"], ["start", "second"]], skippedSources: [] } },
      } } });
      await emit("session://completed", { run_id: runId });
      await running;
      const current = useProfessionalStore.getState().nodes.find((node) => node.id === "aggregate")!.data as ProfessionalNodeData;
      const currentResult = current.result as { outputs?: unknown } | undefined;
      if (JSON.stringify(currentResult?.outputs) !== JSON.stringify({ output: ["FIRST", "SECOND"] }) || current.status !== "completed") throw new Error("生产事件消费者未保留数组结果");
      if (unexpected.length) throw new Error(unexpected.join(","));
      setResult("PASS：生产编辑器 → serializer → 合成事件消费者；非真实后端验收");
    } catch (error) { setResult(String(error)); }
  }
  return <main style={{ minHeight: "100vh", background: "#101318", color: "#eee", padding: 24 }}>
    <h1>H-183 · 变量聚合</h1>
    <p>先确认空绑定提示；添加两项聚合变量，分别选择 first / second，再验证合成事件。</p>
    <section style={{ maxWidth: 680, padding: 20, border: "1px solid #444", borderRadius: 16 }}>
      <SchemaConfigForm nodeId="aggregate" data={data} onUpdate={(patch) => useProfessionalStore.getState().updateNodeData("aggregate", patch)} />
    </section>
    <Button variant="secondary" onClick={() => void verify()}>验证绑定与数组结果展示</Button>
    <p role="status">{result || "等待操作"}</p>
    <pre>{JSON.stringify({ bindings: data.aggregatorVariables, status: data.status, result: data.result, unexpectedIPC: unexpected }, null, 2)}</pre>
  </main>;
}
createRoot(document.getElementById("root")!).render(<Verification />);
