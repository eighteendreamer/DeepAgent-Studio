import type { WorkflowNode, WorkflowEdge, NodeStatus } from "../types";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { useCanvasStore } from "../store/canvasStore";
import type { CreativeNodeData, ProfessionalNodeData } from "../types";

let _abortController: AbortController | null = null;

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

export async function runWorkflow(nodeId?: string) {
  if (_abortController) {
    _abortController.abort();
  }
  _abortController = new AbortController();
  const { signal } = _abortController;

  const mode = useCanvasStore.getState().mode;
  const { nodes, edges } = getNodesAndEdges(mode);

  let executionOrder: string[];
  if (nodeId) {
    executionOrder = [nodeId];
  } else {
    executionOrder = topologicalSort(nodes, edges);
  }

  const nodeMap = new Map(nodes.map((n) => [n.id, n]));

  try {
    for (const id of executionOrder) {
      const node = nodeMap.get(id);
      if (!node) continue;

      if (signal.aborted) break;

      setNodeStatus(mode, id, "running");

      try {
        await sleep(getExecutionDelay(node), signal);
        setNodeStatus(mode, id, "completed", {
          executionTime: getExecutionDelay(node),
          result: { success: true, timestamp: Date.now() },
        });
      } catch {
        setNodeStatus(mode, id, "error", {
          errorMessage: "执行被取消",
        });
        break;
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
