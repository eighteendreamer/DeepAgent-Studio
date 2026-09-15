import type { WorkflowNode, WorkflowEdge, NodeStatus } from "../types";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { useCanvasStore } from "../store/canvasStore";
import type { CreativeNodeData, ProfessionalNodeData } from "../types";

let _abortController: AbortController | null = null;
let _eventUnlisten: (() => void) | null = null;
let _completionUnlisten: (() => void) | null = null;

const UI_META_KEYS = new Set([
  "label",
  "description",
  "kind",
  "status",
  "errorMessage",
  "executionTime",
  "result",
  "inputVariables",
  "outputMapping",
  "conditions",
]);

function serializeNodes(nodes: WorkflowNode[]) {
  return nodes.map((n) => {
    const data = n.data as Record<string, unknown>;
    const config: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(data)) {
      if (!UI_META_KEYS.has(key) && value !== undefined) {
        config[key] = value;
      }
    }
    return { id: n.id, kind: String(data.kind ?? ""), config };
  });
}

function serializeEdges(edges: WorkflowEdge[]) {
  return edges
    .filter((e) => e.source && e.target)
    .map((e) => ({
      id: e.id ?? `${e.source}-${e.target}`,
      source: e.source!,
      target: e.target!,
      source_handle: e.sourceHandle ?? null,
      target_handle: e.targetHandle ?? null,
    }));
}

function collectInputs(nodes: WorkflowNode[]): Record<string, unknown> {
  const startNode = nodes.find(
    (n) => (n.data as { kind?: string }).kind === "start",
  );
  if (!startNode) return {};
  const data = startNode.data as ProfessionalNodeData;
  const inputs: Record<string, unknown> = {};
  for (const v of data.inputVariables ?? []) {
    inputs[v.name] = v.default ?? "";
  }
  return inputs;
}

function mapBackendStatus(backend: string): NodeStatus {
  switch (backend) {
    case "running":
      return "running";
    case "completed":
      return "completed";
    case "failed":
    case "cancelled":
      return "error";
    default:
      return "idle";
  }
}

function topologicalSort(nodes: WorkflowNode[], edges: WorkflowEdge[]): string[] {
  const adj = new Map<string, string[]>();
  const inDegree = new Map<string, number>();

  for (const node of nodes) {
    adj.set(node.id, []);
    inDegree.set(node.id, 0);
  }
  for (const edge of edges) {
    if (edge.source && edge.target) {
      adj.get(edge.source)?.push(edge.target);
      inDegree.set(edge.target, (inDegree.get(edge.target) ?? 0) + 1);
    }
  }

  const queue: string[] = [];
  for (const [id, deg] of inDegree) {
    if (deg === 0) queue.push(id);
  }

  const result: string[] = [];
  while (queue.length > 0) {
    const id = queue.shift()!;
    result.push(id);
    for (const neighbor of adj.get(id) ?? []) {
      const newDeg = (inDegree.get(neighbor) ?? 1) - 1;
      inDegree.set(neighbor, newDeg);
      if (newDeg === 0) queue.push(neighbor);
    }
  }

  return result;
}

function getExecutionDelay(node: WorkflowNode): number {
  const kind = (node.data as { kind: string }).kind;
  switch (kind) {
    case "start":
    case "end":
      return 300;
    case "if-else":
    case "iteration":
    case "iteration-start":
    case "loop":
    case "loop-start":
    case "loop-end":
    case "variable-aggregator":
    case "variable-assigner":
    case "list-operator":
    case "template-transform":
    case "answer":
    case "document-extractor":
      return 500;
    case "code":
    case "http-request":
      return 1200;
    case "datasource":
      return 1200;
    case "knowledge-index":
      return 1500;
    case "trigger-schedule":
    case "trigger-webhook":
    case "trigger-plugin":
      return 300;
    case "llm":
    case "agent":
    case "agent-v2":
    case "text-gen":
    case "script-gen":
      return 2000;
    case "image-gen":
    case "image-edit":
    case "image-compare":
      return 2500;
    case "video-gen":
      return 4000;
    case "video-stitch":
      return 3000;
    default:
      return 1000;
  }
}

function setNodeStatus(
  mode: "creative" | "professional",
  nodeId: string,
  status: NodeStatus,
  extra?: Record<string, unknown>,
) {
  const patch = { status, ...extra };
  if (mode === "creative") {
    useCreativeStore.getState().updateNodeData(nodeId, patch as Partial<CreativeNodeData>);
  } else {
    useProfessionalStore.getState().updateNodeData(nodeId, patch as Partial<ProfessionalNodeData>);
  }
}

function getNodesAndEdges(mode: "creative" | "professional") {
  if (mode === "creative") {
    const s = useCreativeStore.getState();
    return { nodes: s.nodes, edges: s.edges };
  }
  const s = useProfessionalStore.getState();
  return { nodes: s.nodes, edges: s.edges };
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      reject(new DOMException("Aborted", "AbortError"));
    });
  });
}

async function runProfessionalBackend(
  nodes: WorkflowNode[],
  edges: WorkflowEdge[],
  targetNodeId: string | undefined,
  signal: AbortSignal,
): Promise<void> {
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
  if (!w.__TAURI_INTERNALS__) {
    throw new Error("Tauri runtime not available — use creative mode preview");
  }

  const core = await import("@tauri-apps/api/core");
  const eventMod = await import("@tauri-apps/api/event");

  const workflow = {
    definition: {
      version: 1,
      nodes: serializeNodes(nodes),
      edges: serializeEdges(edges),
    },
    inputs: collectInputs(nodes),
    target_node_id: targetNodeId ?? null,
  };

  const ack = await core.invoke<{ run_id: string; session_id: string | null }>(
    "start_workflow",
    { workflow, sessionId: null },
  );

  return new Promise<void>((resolve, reject) => {
    const cleanup = () => {
      _eventUnlisten?.();
      _eventUnlisten = null;
      _completionUnlisten?.();
      _completionUnlisten = null;
    };

    const onAbort = () => {
      cleanup();
      reject(new DOMException("Aborted", "AbortError"));
    };
    if (signal.aborted) {
      cleanup();
      reject(new DOMException("Aborted", "AbortError"));
      return;
    }
    signal.addEventListener("abort", onAbort, { once: true });

    eventMod
      .listen<Record<string, unknown>>("chat://event", (e) => {
        const raw = e.payload;
        const envelope = raw as { run_id?: string; payload?: Record<string, unknown> };
        const event =
          envelope?.run_id && envelope?.payload ? envelope.payload : raw;
        if (event?.type !== "workflow_node") return;
        const inner = (event as { event?: Record<string, unknown> }).event ?? event;
        const nodeId = inner.node_id as string | undefined;
        const backendStatus = inner.status as string | undefined;
        if (!nodeId || !backendStatus) return;

        const mapped = mapBackendStatus(backendStatus);
        setNodeStatus("professional", nodeId, mapped, {
          executionTime:
            typeof inner.elapsed_ms === "number" ? inner.elapsed_ms : undefined,
          errorMessage:
            typeof inner.error === "string" ? inner.error : undefined,
          result:
            inner.outputs !== undefined && inner.outputs !== null
              ? { success: mapped === "completed", outputs: inner.outputs }
              : undefined,
        });
      })
      .then((unlisten) => {
        _eventUnlisten = unlisten;
      });

    eventMod
      .listen<Record<string, unknown>>("session://completed", () => {
        cleanup();
        signal.removeEventListener("abort", onAbort);
        resolve();
      })
      .then((unlisten) => {
        _completionUnlisten = unlisten;
      });

    void ack;
  });
}

async function runCreativeSimulation(
  nodes: WorkflowNode[],
  signal: AbortSignal,
  nodeId?: string,
): Promise<void> {
  let executionOrder: string[];
  if (nodeId) {
    executionOrder = [nodeId];
  } else {
    executionOrder = topologicalSort(nodes, []);
  }

  const nodeMap = new Map(nodes.map((n) => [n.id, n]));

  for (const id of executionOrder) {
    const node = nodeMap.get(id);
    if (!node) continue;
    if (signal.aborted) break;

    setNodeStatus("creative", id, "running");
    try {
      await sleep(getExecutionDelay(node), signal);
      setNodeStatus("creative", id, "completed", {
        executionTime: getExecutionDelay(node),
        result: { success: true, timestamp: Date.now() },
      });
    } catch {
      setNodeStatus("creative", id, "error", {
        errorMessage: "执行被取消",
      });
      break;
    }
  }
}

export async function runWorkflow(nodeId?: string) {
  if (_abortController) {
    _abortController.abort();
  }
  _abortController = new AbortController();
  const { signal } = _abortController;

  const mode = useCanvasStore.getState().mode;
  const { nodes, edges } = getNodesAndEdges(mode);

  try {
    if (mode === "professional") {
      resetAllStatus();
      await runProfessionalBackend(nodes, edges, nodeId, signal);
    } else {
      await runCreativeSimulation(nodes, signal, nodeId);
    }
  } catch (err) {
    if (err instanceof DOMException && err.name === "AbortError") {
      return;
    }
    const msg = err instanceof Error ? err.message : String(err);
    const allIds = nodeId ? [nodeId] : topologicalSort(nodes, edges);
    for (const id of allIds) {
      const current =
        mode === "professional"
          ? useProfessionalStore.getState().nodes.find((n) => n.id === id)
          : useCreativeStore.getState().nodes.find((n) => n.id === id);
      if (current && (current.data as { status?: NodeStatus }).status === "running") {
        setNodeStatus(mode, id, "error", { errorMessage: msg });
      }
    }
  } finally {
    _abortController = null;
  }
}

export function stopWorkflow() {
  if (_abortController) {
    _abortController.abort();
    _abortController = null;
  }
  _eventUnlisten?.();
  _eventUnlisten = null;
  _completionUnlisten?.();
  _completionUnlisten = null;
}

export function resetAllStatus() {
  const mode = useCanvasStore.getState().mode;
  const { nodes } = getNodesAndEdges(mode);
  for (const node of nodes) {
    setNodeStatus(mode, node.id, "idle");
  }
}

export function isWorkflowRunning(): boolean {
  return _abortController != null;
}
