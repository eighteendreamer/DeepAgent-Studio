import type { WorkflowNode, WorkflowEdge, NodeStatus } from "../types";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { useCanvasStore } from "../store/canvasStore";
import type { CreativeNodeData, ProfessionalNodeData } from "../types";
import type { ApprovalRequest } from "../../../types";

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

type WorkflowRunOwner = {
  controller: AbortController;
  runId: string | null;
  active: boolean;
  closed: boolean;
  eventUnlisten: (() => void) | null;
  completionUnlisten: (() => void) | null;
  approvalUnlisten: (() => void) | null;
  startBarrier: Promise<void>;
  cancellation: Promise<void> | null;
};

/** 最新画布 owner（含已结束者）；保留身份以拒绝迟到清理，不保留监听。 */
let _activeRun: WorkflowRunOwner | null = null;

function detachRunOwner(owner: WorkflowRunOwner) {
  owner.eventUnlisten?.();
  owner.eventUnlisten = null;
  owner.completionUnlisten?.();
  owner.completionUnlisten = null;
  owner.approvalUnlisten?.();
  owner.approvalUnlisten = null;
  const canvas = useCanvasStore.getState();
  if (canvas.workflowApproval?.runId === owner.runId) canvas.setWorkflowApproval(null);
}

function closeRunOwner(owner: WorkflowRunOwner) {
  owner.active = false;
  owner.closed = true;
  detachRunOwner(owner);
}

function cancelRunOwner(owner: WorkflowRunOwner): Promise<void> {
  if (!owner.runId) return Promise.resolve();
  if (!owner.cancellation) {
    const runId = owner.runId;
    owner.cancellation = (async () => {
      const core = await import("@tauri-apps/api/core");
      const ack = await core.invoke<{ accepted: boolean }>("cancel_run", { runId });
      if (!ack.accepted) console.warn("[canvas] 内核未受理取消:", runId);
    })();
  }
  return owner.cancellation;
}

function retireRunOwner(owner: WorkflowRunOwner): Promise<void> {
  if (owner.closed && !owner.controller.signal.aborted) return Promise.resolve();
  closeRunOwner(owner);
  owner.controller.abort();
  // Carry the predecessor barrier through owners replaced before they start.
  // A late start ACK is cancelled in that owner's own finalizer instead.
  return owner.startBarrier.then(() => cancelRunOwner(owner));
}

function abortError(): DOMException {
  return new DOMException("Aborted", "AbortError");
}

const UI_META_KEYS = new Set([
  "label",
  "description",
  "kind",
  "status",
  "errorMessage",
  "executionTime",
  "audioReferenceName",
  "result",
  "outputMapping",
  // 界面意图字段：已折算成内核认的 size / referenceImages，不再重复跨界。
  "aspectRatio",
  "resolution",
  "customSize",
  "batchCount",
  "imageInputUrls",
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
  "video-edit": "videoUrl",
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
  const kind = String(data.kind ?? "");
  const config: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(data)) {
    if (!UI_META_KEYS.has(key) && value !== undefined) config[key] = value;
  }
  const texts: string[] = [];
  const images: string[] = [];
  const audios: string[] = [];
  const videos: string[] = Array.isArray(data.videoEditInputRefs) ? data.videoEditInputRefs.map(String) : [];
  for (const edge of incoming) {
    const field = outputFieldByNode.get(edge.source);
    if (!field) continue;
    const reference = `{{#${edge.source}.${field}#}}`;
    if (field === "imageUrl") images.push(reference);
    else if (field === "videoUrl") {
      // The interactive editor currently edits one source at a time. A manually
      // selected/imported source takes precedence; otherwise use the first link.
      if (kind === "video-edit" && videos.length === 0) videos.push(reference);
    }
    else if (field === "audioUrl") audios.push(reference);
    else texts.push(reference);
  }
  if (texts.length) config.upstreamTexts = texts;
  // 内核只认 referenceImages：连线来的图片与面板上传的参考图合并成同一份。
  const own = Array.isArray(data.imageInputUrls)
    ? (data.imageInputUrls as unknown[]).map(String).filter((value) => value.trim())
    : [];
  const references = [...new Set([...images, ...own])];
  if (references.length) config.referenceImages = references;
  if (audios.length) config.audioInputs = audios;
  if (videos.length && kind === "video-edit") config.videoEditInputRefs = [...new Set(videos)];
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
  // 供应商作业 id 留在节点上：中断后重跑继续轮询同一个任务。
  if (typeof record.videoTaskId === "string") patch.videoTaskId = record.videoTaskId;
  if (typeof record.videoTaskFingerprint === "string") {
    patch.videoTaskFingerprint = record.videoTaskFingerprint;
  }
  if (typeof record.audioUrl === "string") patch.audioUrl = record.audioUrl;
  if (typeof record.modelId === "string") patch.usedModel = record.modelId;
  return patch;
}

/**
 * Project stable provider task identity from the typed node event itself.
 *
 * `updates` remains display-oriented and is intentionally not spread into the
 * store. Only a video generation node may turn the backend job id into the
 * `videoTaskId` that the next workflow request uses for resume.
 */
function applyNodeEventIdentity(
  mode: "creative" | "professional",
  nodeId: string,
  event: Record<string, unknown>,
): Record<string, unknown> {
  const jobId = typeof event.job_id === "string" ? event.job_id.trim() : "";
  if (!jobId) return {};
  const fingerprint =
    typeof event.resume_fingerprint === "string" ? event.resume_fingerprint.trim() : "";
  const node = getNodesAndEdges(mode).nodes.find((candidate) => candidate.id === nodeId);
  const kind = String((node?.data as { kind?: string } | undefined)?.kind ?? "");
  return kind === "video-gen"
    ? {
        videoTaskId: jobId,
        ...(fingerprint ? { videoTaskFingerprint: fingerprint } : {}),
      }
    : {};
}

async function runBackendWorkflow(
  mode: "creative" | "professional",
  nodes: WorkflowNode[],
  edges: WorkflowEdge[],
  targetNodeId: string | undefined,
  owner: WorkflowRunOwner,
): Promise<void> {
  const { signal } = owner.controller;
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
    // The canvas currently has no per-run input form. Defaults stay in the
    // declaration; the shared backend resolves them once and records their
    // source. Do not manufacture empty strings or turn defaults into provided.
    inputs: {},
    target_node_id: targetNodeId ?? null,
  };

  type BufferedEvent = {
    channel: "workflow" | "completion" | "approval";
    payload: Record<string, unknown>;
  };
  const buffered: BufferedEvent[] = [];
  // Display projection only. Item results remain scoped in durable events;
  // a last item's provider job must never become every item's resume config.
  const scopedNodes = new Map<string, { running: Set<string>; failed: boolean; error?: string }>();
  let resolveCompletion!: () => void;
  let rejectCompletion!: (reason: unknown) => void;
  const completion = new Promise<void>((resolve, reject) => {
    resolveCompletion = resolve;
    rejectCompletion = reject;
  });
  // Cancellation can arrive while listener registration/start ACK is pending.
  // Observe immediately; awaiting below still receives the original rejection.
  void completion.catch(() => {});

  const cleanup = () => {
    closeRunOwner(owner);
    signal.removeEventListener("abort", onAbort);
  };
  const onAbort = () => {
    cleanup();
    rejectCompletion(abortError());
  };
  if (signal.aborted) {
    cleanup();
    throw abortError();
  }
  signal.addEventListener("abort", onAbort, { once: true });

  const handleWorkflowPayload = (raw: Record<string, unknown>) => {
    if (!owner.active || owner.closed || _activeRun !== owner) return;
    const envelope = raw as { run_id?: string; payload?: Record<string, unknown> };
    if (!owner.runId) {
      buffered.push({ channel: "workflow", payload: raw });
      return;
    }
    if (envelope?.run_id !== owner.runId) return;
    const event = envelope?.payload ?? raw;
    if (event?.type !== "workflow_node") return;
    const inner = (event as { event?: Record<string, unknown> }).event ?? event;
    const nodeId = inner.node_id as string | undefined;
    const backendStatus = inner.status as string | undefined;
    if (!nodeId || !backendStatus) return;

    const mapped = mapBackendStatus(backendStatus);
    if (Array.isArray(inner.scope) && inner.scope.length > 0) {
      if (!inner.scope.every((part) => typeof part === "string")) return;
      const key = JSON.stringify(inner.scope);
      const state = scopedNodes.get(nodeId) ?? { running: new Set<string>(), failed: false };
      if (mapped === "running") state.running.add(key);
      else state.running.delete(key);
      if (mapped === "error") {
        state.failed = true;
        if (typeof inner.error === "string") state.error = inner.error;
      }
      scopedNodes.set(nodeId, state);
      const status = state.running.size > 0 ? "running" : state.failed ? "error" : mapped;
      setNodeStatus(mode, nodeId, status, {
        executionTime: typeof inner.elapsed_ms === "number" ? inner.elapsed_ms : undefined,
        errorMessage: state.error,
        result: inner.outputs !== undefined && inner.outputs !== null
          ? { success: mapped === "completed", outputs: inner.outputs, scope: inner.scope }
          : undefined,
      });
      return;
    }
    setNodeStatus(mode, nodeId, mapped, {
      executionTime:
        typeof inner.elapsed_ms === "number" ? inner.elapsed_ms : undefined,
      errorMessage:
        typeof inner.error === "string" ? inner.error : undefined,
      result:
        inner.outputs !== undefined && inner.outputs !== null
          ? { success: mapped === "completed", outputs: inner.outputs }
          : undefined,
      ...applyNodeEventIdentity(mode, nodeId, inner),
      ...applyNodeOutputs(mode, inner.outputs),
    });
  };

  const handleCompletionPayload = (payload: Record<string, unknown>) => {
    if (!owner.active || owner.closed || _activeRun !== owner) return;
    if (!owner.runId) {
      buffered.push({ channel: "completion", payload });
      return;
    }
    if (payload.run_id !== owner.runId) return;
    cleanup();
    if (payload.status === "failed") {
      rejectCompletion(new Error(typeof payload.error === "string" ? payload.error : "工作流执行失败"));
    } else {
      resolveCompletion();
    }
  };

  const handleApprovalPayload = (raw: Record<string, unknown>) => {
    if (!owner.active || owner.closed || _activeRun !== owner) return;
    if (!owner.runId) {
      buffered.push({ channel: "approval", payload: raw });
      return;
    }
    if (raw.run_id !== owner.runId) return;
    const request = raw.payload as ApprovalRequest | undefined;
    if (!request || typeof request.call_id !== "string" || !request.call_id) return;
    const canvas = useCanvasStore.getState();
    const view = canvas.workflowApproval;
    const requests = view?.runId === owner.runId ? view.requests : [];
    if (requests.some((pending) => pending.call_id === request.call_id)) return;
    canvas.setWorkflowApproval({
      runId: owner.runId,
      requests: [...requests, { ...request, run_id: owner.runId }],
      submitting: view?.submitting ?? false,
      error: view?.error ?? null,
    });
  };

  const eventUnlisten = await eventMod.listen<Record<string, unknown>>(
    "chat://event",
    (e) => handleWorkflowPayload(e.payload),
  );
  if (!owner.active || owner.closed || signal.aborted || _activeRun !== owner) {
    eventUnlisten();
    throw abortError();
  }
  owner.eventUnlisten = eventUnlisten;

  const completionUnlisten = await eventMod.listen<Record<string, unknown>>(
    "session://completed",
    (e) => handleCompletionPayload(e.payload),
  );
  if (!owner.active || owner.closed || signal.aborted || _activeRun !== owner) {
    completionUnlisten();
    throw abortError();
  }
  owner.completionUnlisten = completionUnlisten;

  const approvalUnlisten = await eventMod.listen<Record<string, unknown>>(
    "chat://approval",
    (e) => handleApprovalPayload(e.payload),
  );
  if (!owner.active || owner.closed || signal.aborted || _activeRun !== owner) {
    approvalUnlisten();
    throw abortError();
  }
  owner.approvalUnlisten = approvalUnlisten;

  const ack = await core.invoke<{ run_id: string; session_id: string | null }>(
    "start_workflow",
    { workflow: { ...workflow, workspace_id: null }, sessionId: null },
  );
  // Admission identity belongs to this attempt even when it was retired while
  // invoke was pending. Its finalizer must still notify the existing kernel.
  owner.runId = ack.run_id;
  if (!owner.active || owner.closed || signal.aborted || _activeRun !== owner) {
    throw abortError();
  }
  for (const event of buffered.splice(0)) {
    if (!owner.active || owner.closed) break;
    if (event.channel === "workflow") handleWorkflowPayload(event.payload);
    else if (event.channel === "completion") handleCompletionPayload(event.payload);
    else handleApprovalPayload(event.payload);
  }

  await completion;
}

export async function runWorkflow(nodeId?: string) {
  const startBarrier = _activeRun ? retireRunOwner(_activeRun) : Promise.resolve();
  const owner: WorkflowRunOwner = {
    controller: new AbortController(),
    runId: null,
    active: true,
    closed: false,
    eventUnlisten: null,
    completionUnlisten: null,
    approvalUnlisten: null,
    startBarrier,
    cancellation: null,
  };
  _activeRun = owner;

  const mode = useCanvasStore.getState().mode;
  const { nodes, edges } = getNodesAndEdges(mode);

  try {
    await owner.startBarrier;
    if (_activeRun !== owner || owner.controller.signal.aborted) throw abortError();
    resetAllStatus();
    await runBackendWorkflow(mode, nodes, edges, nodeId, owner);
  } catch (err) {
    if (_activeRun !== owner || owner.controller.signal.aborted) return;
    if (err instanceof DOMException && err.name === "AbortError") {
      return;
    }
    const msg = err instanceof Error ? err.message : String(err);
    console.error("[canvas] 工作流运行失败:", msg);
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
    // Admission can fail before any node event (whole graph or targeted run).
    // Keep one visible diagnostic without overwriting already completed nodes.
    const firstId = nodeId ?? nodes[0]?.id;
    const currentNodes = mode === "professional"
      ? useProfessionalStore.getState().nodes : useCreativeStore.getState().nodes;
    const first = currentNodes.find((node) => node.id === firstId);
    const status = (first?.data as { status?: NodeStatus } | undefined)?.status;
    if (first && (!status || status === "idle")) {
      setNodeStatus(mode, first.id, "error", { errorMessage: msg });
    }
  } finally {
    closeRunOwner(owner);
    if (owner.controller.signal.aborted) {
      try {
        await cancelRunOwner(owner);
      } catch (error) {
        console.error("[canvas] 取消运行失败:", error);
      }
    }
  }
}

/** 回答当前 owner 的当前审批；迟到点击/响应不能影响下一次运行。 */
export async function respondWorkflowApproval(
  request: ApprovalRequest,
  approved: boolean,
  scope?: string,
): Promise<boolean> {
  const owner = _activeRun;
  const view = useCanvasStore.getState().workflowApproval;
  if (!owner?.active || owner.closed || !owner.runId || !view ||
      view.runId !== owner.runId || request.run_id !== owner.runId ||
      view.requests[0] !== request || view.submitting) return false;
  useCanvasStore.getState().setWorkflowApproval({ ...view, submitting: true, error: null });
  let accepted = false;
  let error: string | null = null;
  try {
    const api = await import("../../../api");
    // Dynamic import may yield while the run is cancelled or replaced.
    if (_activeRun !== owner || !owner.active || owner.closed) return false;
    accepted = await api.resolveApproval(request.call_id, approved, scope);
    if (!accepted) error = "审批已失效，内核未受理。请核对运行状态或取消工作流。";
  } catch (cause) {
    error = `审批回答失败：${cause instanceof Error ? cause.message : String(cause)}`;
  }
  if (_activeRun === owner && owner.active && !owner.closed) {
    const current = useCanvasStore.getState().workflowApproval;
    if (current?.runId === owner.runId && current.requests[0] === request) {
      const requests = accepted ? current.requests.slice(1) : current.requests;
      useCanvasStore.getState().setWorkflowApproval(
        requests.length ? { ...current, requests, submitting: false, error } : null,
      );
    }
  }
  return accepted;
}

/**
 * 取消这次画布运行。
 *
 * 节点上的取消针对整次画布运行；先失效本地 owner，再经统一取消函数通知内核。
 * 启动 ACK 尚未返回时，由原 owner 在收到 run_id 后补发取消；取消受理不等于终态。
 */
export async function cancelWorkflow(nodeId?: string) {
  const mode = useCanvasStore.getState().mode;
  const owner = _activeRun;
  if (owner) {
    try {
      await retireRunOwner(owner);
    } catch (error) {
      console.error("[canvas] 取消运行失败:", error);
    }
    // A stop response may arrive after another run claimed the same node.
    if (_activeRun !== owner) return;
  }
  if (nodeId) setNodeStatus(mode, nodeId, "idle");
}

export function resetAllStatus() {
  const mode = useCanvasStore.getState().mode;
  const { nodes } = getNodesAndEdges(mode);
  for (const node of nodes) {
    setNodeStatus(mode, node.id, "idle");
  }
}

export function isWorkflowRunning(): boolean {
  return _activeRun?.active === true && !_activeRun.controller.signal.aborted;
}
