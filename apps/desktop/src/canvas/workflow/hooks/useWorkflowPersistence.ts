import { useEffect, useRef } from "react";
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
  try {
    const raw = localStorage.getItem(LEGACY_STORAGE_KEYS[mode]);
    if (!raw) return null;
    return normalizeGraph(JSON.parse(raw));
  } catch {
    return null;
  }
}

function forgetLegacyGraph(mode: CanvasMode) {
  try {
    localStorage.removeItem(LEGACY_STORAGE_KEYS[mode]);
  } catch {
    // 存储不可用时无需处理：数据库已经是唯一读取来源
  }
}

export function useWorkflowPersistence() {
  const initialized = useRef(false);

  useEffect(() => {
    if (initialized.current || !isTauriRuntime()) return;
    initialized.current = true;

    let disposed = false;
    let unsubscriptions: Array<() => void> = [];
    const timers = new Map<CanvasMode, ReturnType<typeof setTimeout>>();
    /** 每个模式最近一次已知落库内容，用于跳过无变化的自动保存。 */
    const saved = new Map<CanvasMode, string>();

    const persist = async (mode: CanvasMode) => {
      const graph = graphOf(mode);
      const serialized = JSON.stringify(graph);
      if (serialized === saved.get(mode)) return;
      try {
        await invoke("canvas_workflow_state_write", {
          mode,
          workspaceId: null,
          graph,
        });
        saved.set(mode, serialized);
      } catch (error) {
        console.error(`[canvas] 保存 ${mode} 画布失败:`, error);
      }
    };

    const schedulePersist = (mode: CanvasMode) => {
      const pending = timers.get(mode);
      if (pending) clearTimeout(pending);
      timers.set(
        mode,
        setTimeout(() => {
          timers.delete(mode);
          void persist(mode);
        }, PERSIST_DEBOUNCE_MS),
      );
    };

    void (async () => {
      for (const mode of CANVAS_MODES) {
        try {
          const stored = normalizeGraph(
            await invoke<unknown>("canvas_workflow_state_read", {
              mode,
              workspaceId: null,
            }),
          );
          const migrated = stored ?? readLegacyGraph(mode);
          if (!migrated) {
            forgetLegacyGraph(mode);
            continue;
          }
          if (!stored) {
            await invoke("canvas_workflow_state_write", {
              mode,
              workspaceId: null,
              graph: migrated,
            });
          }
          forgetLegacyGraph(mode);
          if (disposed) return;
          applyGraph(mode, migrated);
          saved.set(mode, JSON.stringify(migrated));
        } catch (error) {
          console.error(`[canvas] 读取 ${mode} 画布失败:`, error);
        }
      }
      if (disposed) return;
      unsubscriptions = [
        useCreativeStore.subscribe(() => schedulePersist("creative")),
        useProfessionalStore.subscribe(() => schedulePersist("professional")),
      ];
    })();

    return () => {
      disposed = true;
      unsubscriptions.forEach((unsubscribe) => unsubscribe());
      timers.forEach((timer) => clearTimeout(timer));
    };
  }, []);
}
