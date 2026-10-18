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

type CanvasMode = "creative" | "professional";

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
  timer?: ReturnType<typeof setTimeout>;
  writing?: Promise<void>;
}

// 与两个 Zustand store 一样属于当前画布窗口，React 重新挂载不会重读旧快照。
// 每个模式只建立一个订阅；数据库仍是唯一持久化来源。
const slots = new Map<CanvasMode, PersistenceSlot>();
let hydration: Promise<void> | null = null;

function persist(mode: CanvasMode): Promise<void> {
  const slot = slots.get(mode);
  if (!slot) return Promise.resolve();
  if (slot.timer) clearTimeout(slot.timer);
  slot.timer = undefined;
  if (slot.writing) return slot.writing;
  slot.writing = (async () => {
    // 写入期间仍可编辑；完成后继续保存最新快照，旧写入不能覆盖新写入。
    while (true) {
      const serialized = JSON.stringify(graphOf(mode));
      if (serialized === slot.saved) return;
      await invoke("canvas_workflow_state_write", {
        mode,
        workspaceId: null,
        graph: JSON.parse(serialized),
      });
      slot.saved = serialized;
    }
  })().finally(() => { slot.writing = undefined; });
  return slot.writing;
}

function subscribe(mode: CanvasMode) {
  const store = mode === "creative" ? useCreativeStore : useProfessionalStore;
  store.subscribe((state, previous) => {
    if (state.nodes === previous.nodes && state.edges === previous.edges) return;
    const slot = slots.get(mode)!;
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
      const stored = normalizeGraph(value);
      if (value != null && !stored) throw new Error(`${mode} 画布数据无效`);
      const graph = stored ?? readLegacyGraph(mode);
      if (graph && !stored) {
        await invoke("canvas_workflow_state_write", { mode, workspaceId: null, graph });
      }
      // 迁移成功才删除旧键。失败不能把唯一副本丢掉。
      forgetLegacyGraph(mode);
      if (graph) applyGraph(mode, graph);
      slots.set(mode, { saved: JSON.stringify(graphOf(mode)) });
      subscribe(mode);
    }
  })().catch((error) => {
    hydration = null;
    throw error;
  });
  return hydration;
}

/** 取消防抖并等待所有最新快照落库；失败向调用方传播，不能提前关闭窗口。 */
export async function flushWorkflowPersistence(): Promise<void> {
  // 初始化失败的模式没有开放编辑，不允许将空画布写回覆盖原记录。
  await hydration?.catch(() => undefined);
  await Promise.all(CANVAS_MODES.map(persist));
}
