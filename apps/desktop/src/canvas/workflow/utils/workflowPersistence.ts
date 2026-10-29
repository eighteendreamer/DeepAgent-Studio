import { invoke } from "@tauri-apps/api/core";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNode, WorkflowEdge } from "../types";

/**
 * Canvas graph persistence.
 *
 * The kernel database owns where a graph lives; the node shapes inside it stay
 * a frontend concern. `localStorage` is only read once, to move graphs drawn
 * before this change into the database, and those keys are then dropped.
 */

export type CanvasMode = "creative" | "professional";

interface CanvasGraph {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
}

const CANVAS_MODES: CanvasMode[] = ["creative", "professional"];

/** Keys used by the pre-database canvas. */
const LEGACY_STORAGE_KEYS: Record<CanvasMode, string> = {
  creative: "workflow-canvas-creative",
  professional: "workflow-canvas-professional",
};

/** 画布持续编辑，落库要合并连续变更。 */
const PERSIST_DEBOUNCE_MS = 500;

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function graphOf(mode: CanvasMode): CanvasGraph {
  const state =
    mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
  return { nodes: state.nodes, edges: state.edges };
}

function applyGraph(mode: CanvasMode, graph: CanvasGraph) {
  if (mode === "creative") {
    useCreativeStore.setState({ nodes: graph.nodes, edges: graph.edges });
    return;
  }
  const store = useProfessionalStore.getState();
  store.setNodes(graph.nodes);
  store.setEdges(graph.edges);
}

/** 旧版本 id 计数器重载后可能写入同 id 节点，恢复时按 id 去重（保留首个）。 */
function dedupeNodes(nodes: WorkflowNode[]): WorkflowNode[] {
  const seen = new Set<string>();
  return nodes.filter((node) => (seen.has(node.id) ? false : seen.add(node.id)));
}

function normalizeGraph(value: unknown): CanvasGraph | null {
  if (!value || typeof value !== "object") return null;
  const graph = value as Partial<CanvasGraph>;
  if (!Array.isArray(graph.nodes)) return null;
  return {
    nodes: dedupeNodes(graph.nodes),
    edges: Array.isArray(graph.edges) ? graph.edges : [],
  };
}

function readLegacyGraph(mode: CanvasMode): CanvasGraph | null {
  const raw = localStorage.getItem(LEGACY_STORAGE_KEYS[mode]);
  if (!raw) return null;
  const graph = normalizeGraph(JSON.parse(raw));
  if (!graph) throw new Error(`${mode} 旧画布数据无效`);
  return graph;
}

function forgetLegacyGraph(mode: CanvasMode) {
  try {
    localStorage.removeItem(LEGACY_STORAGE_KEYS[mode]);
  } catch {
    // 存储不可用时无需处理：数据库已经是唯一读取来源
  }
}

interface PersistenceSlot {
  saved: string;
  revision: string | null;
  error?: PersistenceError;
  recovering?: Promise<void>;
  backedUp?: { graph: string; id: string };
  timer?: ReturnType<typeof setTimeout>;
  writing?: Promise<void>;
}

class PersistenceError extends Error {
  constructor(message: string, readonly conflict = false) { super(message); }
}

export interface WorkflowPersistenceProblem {
  mode: CanvasMode;
  message: string;
  conflict: boolean;
  recovering: boolean;
}

function failure(cause: unknown): PersistenceError {
  if (cause instanceof PersistenceError) return cause;
  const wire = cause as { schemaVersion?: number; code?: string; message?: string } | null;
  const conflict = wire?.schemaVersion === 1 && wire.code === "revision_conflict";
  return new PersistenceError(
    conflict ? "画布已被其他窗口更新，本地编辑尚未保存。" : (wire?.message ?? String(cause)),
    conflict,
  );
}

function snapshot(value: unknown): { graph: CanvasGraph | null; revision: string | null } {
  const state = value as { schemaVersion?: number; revision?: unknown; graph?: unknown } | null;
  if (state?.schemaVersion !== 1) throw new Error("画布存储协议版本不兼容");
  const graph = normalizeGraph(state.graph);
  if (state.graph === null && state.revision === null) return { graph: null, revision: null };
  if (!graph || typeof state.revision !== "string" || !state.revision) throw new Error("画布数据无效");
  return { graph, revision: state.revision };
}

function receipt(value: unknown): string {
  const saved = value as { schemaVersion?: number; revision?: unknown } | null;
  if (saved?.schemaVersion !== 1 || typeof saved.revision !== "string" || !saved.revision) {
    throw new Error("画布保存回执无效；本地编辑仍保留，请核对后重试");
  }
  return saved.revision;
}

// 与两个 Zustand store 一样属于当前画布窗口，React 重新挂载不会重读旧快照。
// 每个模式只建立一个订阅；数据库仍是唯一持久化来源。
const slots = new Map<CanvasMode, PersistenceSlot>();
let hydration: Promise<void> | null = null;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((listener) => listener());

export function workflowPersistenceProblems(): WorkflowPersistenceProblem[] {
  return [...slots].flatMap(([mode, slot]) => slot.error ? [{
    mode, message: slot.error.message, conflict: slot.error.conflict, recovering: !!slot.recovering,
  }] : []);
}

export function subscribeWorkflowPersistence(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function persist(mode: CanvasMode): Promise<void> {
  const slot = slots.get(mode);
  if (!slot) return Promise.resolve();
  if (slot.timer) clearTimeout(slot.timer);
  slot.timer = undefined;
  if (slot.recovering) return slot.recovering;
  if (slot.error?.conflict) return Promise.reject(slot.error);
  if (slot.writing) return slot.writing;
  slot.writing = (async () => {
    // 写入期间仍可编辑；完成后继续保存最新快照，旧写入不能覆盖新写入。
    while (true) {
      const serialized = JSON.stringify(graphOf(mode));
      // A lost response may have committed a different graph. Returning to the
      // old local snapshot is not proof that the pending error was reconciled.
      if (serialized === slot.saved && !slot.error) return;
      const response = await invoke<unknown>("canvas_workflow_state_write", {
        mode,
        workspaceId: null,
        graph: JSON.parse(serialized),
        expectedRevision: slot.revision,
      });
      slot.revision = receipt(response);
      slot.saved = serialized;
      slot.error = undefined;
      notify();
    }
  })().catch((cause) => {
    slot.error = failure(cause);
    notify();
    throw slot.error;
  }).finally(() => { slot.writing = undefined; });
  return slot.writing;
}

function subscribe(mode: CanvasMode) {
  const store = mode === "creative" ? useCreativeStore : useProfessionalStore;
  store.subscribe((state, previous) => {
    if (state.nodes === previous.nodes && state.edges === previous.edges) return;
    const slot = slots.get(mode)!;
    if (slot.error?.conflict || slot.recovering) return;
    if (slot.timer) clearTimeout(slot.timer);
    slot.timer = setTimeout(() => {
      void persist(mode).catch((error) => console.error(`[canvas] 保存 ${mode} 画布失败:`, error));
    }, PERSIST_DEBOUNCE_MS);
  });
}

/** 恢复完成后才能挂载编辑器。读取失败保留旧数据，并允许显式重试。 */
export function whenWorkflowPersistenceReady(): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  if (hydration) return hydration;
  hydration = (async () => {
    for (const mode of CANVAS_MODES) {
      if (slots.has(mode)) continue;
      const value = await invoke<unknown>("canvas_workflow_state_read", {
        mode, workspaceId: null,
      });
      const state = snapshot(value);
      const stored = state.graph;
      let revision = state.revision;
      const graph = stored ?? readLegacyGraph(mode);
      if (graph && !stored) {
        revision = receipt(await invoke("canvas_workflow_state_write", {
          mode, workspaceId: null, graph, expectedRevision: revision,
        }));
        // Only remove the source this window actually committed. A competing
        // migration must not make a retry discard this window's legacy copy.
        forgetLegacyGraph(mode);
      }
      // 迁移成功才删除旧键。失败不能把唯一副本丢掉。
      if (graph) applyGraph(mode, graph);
      slots.set(mode, { saved: JSON.stringify(graphOf(mode)), revision });
      subscribe(mode);
    }
  })().catch((error) => {
    hydration = null;
    throw error;
  });
  return hydration;
}

/** Explicit conflict recovery. Preserve the local graph in the existing library
 * before adopting a fresh snapshot; never turn a conflict token into a blind retry. */
export function preserveAndReloadWorkflow(mode: CanvasMode): Promise<void> {
  const slot = slots.get(mode);
  if (!slot?.error?.conflict) return Promise.reject(new Error("画布当前没有待处理的版本冲突"));
  if (slot.recovering) return slot.recovering;
  const writing = slot.writing;
  slot.recovering = (async () => {
    await writing?.catch(() => undefined);
    const local = JSON.stringify(graphOf(mode));
    if (slot.backedUp?.graph === local) {
      const kept = await invoke<{ graph?: unknown } | null>("canvas_workflow_load", {
        id: slot.backedUp.id, workspaceId: null,
      });
      if (!kept || JSON.stringify(kept.graph) !== local) slot.backedUp = undefined;
    }
    if (slot.backedUp?.graph !== local) {
      const copy = await invoke<{ id?: unknown } | null>("canvas_workflow_save", {
        id: null, name: `${mode === "creative" ? "创意" : "专业"}画布冲突副本 ${new Date().toISOString()}`,
        mode, workspaceId: null, graph: JSON.parse(local),
      });
      if (typeof copy?.id !== "string" || !copy.id) throw new Error("本地副本保存回执无效，尚未载入其他版本");
      slot.backedUp = { graph: local, id: copy.id };
    }
    const latest = snapshot(await invoke("canvas_workflow_state_read", { mode, workspaceId: null }));
    if (JSON.stringify(graphOf(mode)) !== local) {
      throw new Error("保留副本期间画布仍在变化；本地编辑未丢弃，请停止编辑或运行后重试");
    }
    applyGraph(mode, latest.graph ?? { nodes: [], edges: [] });
    slot.saved = JSON.stringify(graphOf(mode));
    slot.revision = latest.revision;
    slot.error = undefined;
    slot.backedUp = undefined;
  })().catch((cause) => {
    slot.error = new PersistenceError(`冲突恢复失败：${failure(cause).message}`, true);
    throw slot.error;
  }).finally(() => { slot.recovering = undefined; notify(); });
  notify();
  return slot.recovering;
}

export function retryWorkflowPersistence(mode: CanvasMode): Promise<void> {
  return persist(mode);
}

/** 取消防抖并等待所有最新快照落库；失败向调用方传播，不能提前关闭窗口。 */
export async function flushWorkflowPersistence(): Promise<void> {
  // 初始化失败的模式没有开放编辑，不允许将空画布写回覆盖原记录。
  await hydration?.catch(() => undefined);
  await Promise.all(CANVAS_MODES.map(persist));
}
