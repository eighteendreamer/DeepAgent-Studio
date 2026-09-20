import type { WorkflowNode, WorkflowEdge, NodeStatus } from "../types";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { useCanvasStore } from "../store/canvasStore";
import type { CreativeNodeData, ProfessionalNodeData } from "../types";

/**
 * Workflow execution for both canvas modes.
 *
 * Professional and creative graphs compile into the same version-1
 * `WorkflowDefinition` and run through one backend chain (`start_workflow` →
 * kernel → `chat://event`). Creative nodes express their inputs through edges,
 * so the edges are translated into the reference syntax the kernel's value
 * resolver already understands (`{{#nodeId.output#}}`) rather than inventing a
 * second data-flow mechanism.
 */

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
  "audioReferenceName",
  "result",
  "inputVariables",
  "outputMapping",
  "conditions",
]);

/** 创作节点种类 → 该节点对外输出的字段（与内核 output_contract 对齐）。 */
const CREATIVE_OUTPUT_FIELD: Record<string, string> = {
  "text-gen": "text",
  "script-gen": "text",
  director: "text",
  "creative-template": "text",
  "image-gen": "imageUrl",
  "image-edit": "imageUrl",
  "image-input": "imageUrl",
  "video-gen": "videoUrl",
  "video-stitch": "videoUrl",
  audio: "audioUrl",
};

/**
 * 创作节点的对外输出字段。
 *
 * 音频节点两种方向输出不同东西：合成给出音频制品引用，转写给出文本，
 * 所以只能按节点自己的方向决定，不能在表里写死。
 */
function creativeOutputField(node: WorkflowNode): string | undefined {
  const data = node.data as { kind?: string; audioOperation?: string };
  const kind = String(data.kind ?? "");
  if (kind === "audio") {
    return data.audioOperation === "speech_synthesize" ? "audioUrl" : "text";
  }
  return CREATIVE_OUTPUT_FIELD[kind];
}

/** 创作节点之间靠连线传递数据，这里把入边翻译成内核引用。 */
function creativeConfig(
  node: WorkflowNode,
  incoming: WorkflowEdge[],
  outputFieldByNode: Map<string, string>,
): Record<string, unknown> {
  const data = node.data as Record<string, unknown>;
  const config: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(data)) {
    if (!UI_META_KEYS.has(key) && value !== undefined) config[key] = value;
  }
  const texts: string[] = [];
  const images: string[] = [];
  const audios: string[] = [];
  for (const edge of incoming) {
    const field = outputFieldByNode.get(edge.source);
    if (!field) continue;
    const reference = `{{#${edge.source}.${field}#}}`;
    if (field === "imageUrl") images.push(reference);
    else if (field === "videoUrl") continue;
    else if (field === "audioUrl") audios.push(reference);
    else texts.push(reference);
  }
  if (texts.length) config.upstreamTexts = texts;
  if (images.length) config.referenceImages = images;
  if (audios.length) config.audioInputs = audios;
  return config;
}

function serializeNodes(nodes: WorkflowNode[], edges: WorkflowEdge[], mode: "creative" | "professional") {
  const outputFieldByNode = new Map<string, string>();
  if (mode === "creative") {
    for (const node of nodes) {
      const field = creativeOutputField(node);
      if (field) outputFieldByNode.set(node.id, field);
    }
  }
  return nodes.map((node) => {
    const kind = String((node.data as { kind?: string }).kind ?? "");
    const config =
      mode === "creative"
        ? creativeConfig(
            node,
            edges.filter((edge) => edge.target === node.id),
            outputFieldByNode,
          )
        : (() => {
            const data = node.data as Record<string, unknown>;
            const plain: Record<string, unknown> = {};
            for (const [key, value] of Object.entries(data)) {
              if (!UI_META_KEYS.has(key) && value !== undefined) plain[key] = value;
            }
            return plain;
          })();
    return { id: node.id, kind, config };
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

/** 内核节点输出回写到画布节点上，界面按现有字段读取。 */
function applyNodeOutputs(
  mode: "creative" | "professional",
  outputs: unknown,
): Record<string, unknown> {
  if (!outputs || typeof outputs !== "object") return {};
  const record = outputs as Record<string, unknown>;
  const patch: Record<string, unknown> = {};
  const text =
    typeof record.text === "string"
      ? record.text
      : typeof record.answer === "string"
        ? record.answer
        : undefined;
  if (text !== undefined) {
    if (mode === "creative") patch.output = text;
    else patch.result = text;
  }
  if (typeof record.imageUrl === "string") patch.imageUrl = record.imageUrl;
  if (typeof record.videoUrl === "string") patch.videoUrl = record.videoUrl;
  if (typeof record.audioUrl === "string") patch.audioUrl = record.audioUrl;
  if (typeof record.modelId === "string") patch.usedModel = record.modelId;
  return patch;
}

async function runBackendWorkflow(
  mode: "creative" | "professional",
  nodes: WorkflowNode[],
  edges: WorkflowEdge[],
  targetNodeId: string | undefined,
  signal: AbortSignal,
): Promise<void> {
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
  if (!w.__TAURI_INTERNALS__) {
    throw new Error("当前窗口不是 Tauri 运行时，无法调用内核执行链");
  }

  const core = await import("@tauri-apps/api/core");
  const eventMod = await import("@tauri-apps/api/event");

  const workflow = {
    definition: {
      version: 1,
      nodes: serializeNodes(nodes, edges, mode),
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
        if (envelope?.run_id && envelope.run_id !== ack.run_id) return;
        const event =
          envelope?.run_id && envelope?.payload ? envelope.payload : raw;
        if (event?.type !== "workflow_node") return;
        const inner = (event as { event?: Record<string, unknown> }).event ?? event;
        const nodeId = inner.node_id as string | undefined;
        const backendStatus = inner.status as string | undefined;
        if (!nodeId || !backendStatus) return;

        const mapped = mapBackendStatus(backendStatus);
        setNodeStatus(mode, nodeId, mapped, {
          executionTime:
            typeof inner.elapsed_ms === "number" ? inner.elapsed_ms : undefined,
          errorMessage:
            typeof inner.error === "string" ? inner.error : undefined,
          result:
            inner.outputs !== undefined && inner.outputs !== null
              ? { success: mapped === "completed", outputs: inner.outputs }
              : undefined,
          ...applyNodeOutputs(mode, inner.outputs),
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

  try {
    resetAllStatus();
    await runBackendWorkflow(mode, nodes, edges, nodeId, signal);
  } catch (err) {
    if (err instanceof DOMException && err.name === "AbortError") {
      return;
    }
    const msg = err instanceof Error ? err.message : String(err);
    const affected = nodeId ? [nodeId] : nodes.map((node) => node.id);
    for (const id of affected) {
      const current =
        mode === "professional"
          ? useProfessionalStore.getState().nodes.find((n) => n.id === id)
          : useCreativeStore.getState().nodes.find((n) => n.id === id);
      if (current && (current.data as { status?: NodeStatus }).status === "running") {
        setNodeStatus(mode, id, "error", { errorMessage: msg });
      }
    }
    if (nodeId === undefined) {
      // 整图提交失败时（例如缺少 Tauri 运行时），把原因落到第一个节点，避免静默无反馈。
      const first = nodes[0];
      if (first && !(first.data as { status?: NodeStatus }).status) {
        setNodeStatus(mode, first.id, "error", { errorMessage: msg });
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
