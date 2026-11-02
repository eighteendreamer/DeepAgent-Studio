// Manual browser gate: real canvas components/store/executor, synthetic IPC.
// No backend, filesystem, provider, settings DB or user session is accessed.
import { useState } from "react";
import { createRoot } from "react-dom/client";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import { ReactFlow, Background } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import "../src/styles.css";
import { Button } from "../src/components/shadcn/button";

mockWindows("h181-verification");
let runId = "";
let starts = 0;
let submitted: unknown;
const unexpected: string[] = [];
mockIPC((command, args) => {
  if (command === "start_workflow") {
    runId = `h181-browser-${++starts}`;
    submitted = (args as { workflow: unknown }).workflow;
    return { run_id: runId, session_id: null };
  }
  if (command === "cancel_run") return { accepted: true };
  // Module hydration reads only these explicit, empty synthetic documents.
  // A non-null receipt also prevents legacy browser-storage migration.
  if (command === "canvas_preference_read") {
    const key = (args as { key: string }).key;
    if (key === "workspace-dirs") return {};
    if (["creative-library", "workflow-snippets", "video-edit-projects"].includes(key)) return [];
  }
  if (command === "canvas_settings_read") return { providers: [], bindings: [] };
  unexpected.push(command);
  throw new Error(`Unexpected verification IPC: ${command}`);
}, { shouldMockEvents: true });

const [{ WorkflowNodeShell }, { useProfessionalStore }, { useCanvasStore }, executor, { createDefaultNodeData }] = await Promise.all([
  import("../src/canvas/workflow/components/WorkflowNodeShell"),
  import("../src/canvas/workflow/store/professionalStore"),
  import("../src/canvas/workflow/store/canvasStore"),
  import("../src/canvas/workflow/utils/workflowExecutor"),
  import("../src/canvas/workflow/utils/nodeRegistry"),
]);
useCanvasStore.setState({ mode: "professional" });
useProfessionalStore.setState({ nodes: [
  { id: "iterate", type: "verification", position: { x: 80, y: 70 }, data: { ...createDefaultNodeData("iteration"), label: "迭代容器", inputVariable: "{{#start.items#}}", parallel: true, maxConcurrency: 2 } },
  { id: "body", type: "verification", position: { x: 390, y: 70 }, data: { ...createDefaultNodeData("llm"), label: "迭代体 · 两个 item", llmPrompt: "合成输入，不请求模型" } },
  { id: "end", type: "verification", position: { x: 700, y: 70 }, data: { ...createDefaultNodeData("end"), label: "done 合流" } },
], edges: [] });
const nodeTypes = { verification: WorkflowNodeShell };

async function nodeEvent(node: string, status: string, scope: string[] = [], outputs?: unknown) {
  await emit("chat://event", { run_id: runId, payload: { type: "workflow_node", event: {
    revision: "h181-synthetic-browser", node_id: node, scope, status, attempt: 1, elapsed_ms: 12, outputs,
  } } });
}

function Verification() {
  const nodes = useProfessionalStore((state) => state.nodes);
  const [stage, setStage] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [configClean, setConfigClean] = useState<boolean | null>(null);
  async function next() {
    setBusy(true);
    try {
      if (stage === 0) {
        void executor.runWorkflow();
        for (let i = 0; starts === 0 && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10));
        if (!starts) throw new Error("start ACK missing");
        await nodeEvent("iterate", "running");
        await nodeEvent("body", "running", ["iterate", "0"]);
        await nodeEvent("body", "running", ["iterate", "1"]);
      } else if (stage === 1) {
        await nodeEvent("body", "completed", ["iterate", "1"], { text: "第二项先完成" });
        if (useProfessionalStore.getState().nodes.find((node) => node.id === "body")?.data.status !== "running") {
          throw new Error("快项错误地把仍有慢项的节点标成完成");
        }
      } else if (stage === 2) {
        await nodeEvent("body", "completed", ["iterate", "0"], { text: "第一项后完成" });
        await nodeEvent("iterate", "completed", [], { output: ["第一项后完成", "第二项先完成"] });
        await nodeEvent("end", "completed");
        await emit("session://completed", { run_id: runId });
      } else if (stage === 3) {
        void executor.runWorkflow();
        for (let i = 0; starts < 2 && i < 100; i++) await new Promise((resolve) => setTimeout(resolve, 10));
        if (starts !== 2) throw new Error("second start ACK missing");
        const wire = submitted as { definition: { nodes: { config: Record<string, unknown> }[] } };
        const clean = wire.definition.nodes.every((node) => !Object.prototype.hasOwnProperty.call(node.config, "result"));
        setConfigClean(clean);
        if (!clean) throw new Error("item 展示结果泄漏到下一次可执行配置");
        await emit("session://completed", { run_id: runId });
      }
      if (unexpected.length) throw new Error(`unexpected IPC: ${unexpected.join(",")}`);
      setStage((value) => value + 1);
    } catch (failure) { setError(String(failure)); }
    finally { setBusy(false); }
  }
  return <main style={{ height: "100vh", background: "#101318", color: "#f5f5f5" }}>
    <header style={{ padding: 24 }}>
      <h1>H-181 · 迭代作用域运行验证</h1>
      <p>真实画布组件 / store / 事件消费者；Tauri 官方 mock，仅合成数据，无真实模型或后端。</p>
      <Button variant="secondary" disabled={busy || stage >= 4} onClick={() => void next()}>
        {["启动两个 item", "完成快项，保留慢项", "完成慢项并合流", "重跑，检查配置隔离", "验证完成"][stage]}
      </Button>
      <p role="status">阶段 {stage}/4 · {error || (stage === 4 ? "PASS" : "等待操作")}</p>
    </header>
    <section style={{ height: 340 }} aria-label="真实专业画布节点">
      <ReactFlow nodes={nodes} edges={[]} nodeTypes={nodeTypes} fitView nodesDraggable={false} nodesConnectable={false}>
        <Background />
      </ReactFlow>
    </section>
    <pre id="evidence" style={{ padding: 24, fontSize: 13, whiteSpace: "pre-wrap" }}>{JSON.stringify({
      stage, error, starts, configClean, unexpectedIPC: unexpected,
      nodes: nodes.map((node) => ({ id: node.id, status: node.data.status, result: node.data.result })),
    }, null, 2)}</pre>
  </main>;
}

createRoot(document.getElementById("root")!).render(<Verification />);
