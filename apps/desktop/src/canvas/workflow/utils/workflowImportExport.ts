import type { WorkflowNode, WorkflowEdge } from "../types";

export interface WorkflowData {
  version: number;
  mode: "creative" | "professional";
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  exportedAt: string;
}

const CURRENT_VERSION = 1;

export function exportWorkflow(
  mode: "creative" | "professional",
  nodes: WorkflowNode[],
  edges: WorkflowEdge[],
): WorkflowData {
  return {
    version: CURRENT_VERSION,
    mode,
    nodes: nodes.map((n) => ({
      ...n,
      selected: false,
      dragging: false,
    })),
    edges,
    exportedAt: new Date().toISOString(),
  };
}

export function downloadWorkflow(data: WorkflowData, filename?: string): void {
  const json = JSON.stringify(data, null, 2);
  const blob = new Blob([json], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename ?? `workflow-${Date.now()}.json`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}

export interface ImportResult {
  ok: boolean;
  error?: string;
  data?: WorkflowData;
}

export function parseWorkflowImport(json: string): ImportResult {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return { ok: false, error: "JSON 格式错误" };
  }

  if (!parsed || typeof parsed !== "object") {
    return { ok: false, error: "无效的工作流文件" };
  }

  const data = parsed as Partial<WorkflowData>;

  if (data.version !== CURRENT_VERSION) {
    return { ok: false, error: `不支持的版本（当前 ${CURRENT_VERSION}，文件 ${data.version ?? "未知"}）` };
  }

  if (data.mode !== "creative" && data.mode !== "professional") {
    return { ok: false, error: "无效的模式" };
  }

  if (!Array.isArray(data.nodes) || !Array.isArray(data.edges)) {
    return { ok: false, error: "缺少节点或边数据" };
  }

  return { ok: true, data: data as WorkflowData };
}
