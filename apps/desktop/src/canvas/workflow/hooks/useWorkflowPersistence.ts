import { useEffect, useRef } from "react";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNode, WorkflowEdge } from "../types";
import type { Viewport } from "@xyflow/react";

interface PersistedState {
  nodes: WorkflowNode[];
  edges: WorkflowEdge[];
  viewport?: Viewport;
}

const STORAGE_PREFIX = "workflow-canvas";

function loadState(mode: string): PersistedState | null {
  try {
    const raw = localStorage.getItem(`${STORAGE_PREFIX}-${mode}`);
    if (!raw) return null;
    return JSON.parse(raw) as PersistedState;
  } catch {
    return null;
  }
}

function saveState(mode: string, state: PersistedState) {
  try {
    localStorage.setItem(`${STORAGE_PREFIX}-${mode}`, JSON.stringify(state));
  } catch {
    // storage full or unavailable
  }
}

export function useWorkflowPersistence() {
  const initialized = useRef(false);

  useEffect(() => {
    if (initialized.current) return;
    initialized.current = true;

    const creativeState = loadState("creative");
    if (creativeState) {
      useCreativeStore.setState({
        nodes: creativeState.nodes,
        edges: creativeState.edges,
      });
    }

    const proState = loadState("professional");
    if (proState) {
      useProfessionalStore.setState({
        nodes: proState.nodes,
        edges: proState.edges,
      });
    }
  }, []);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    const unsubCreative = useCreativeStore.subscribe((state) => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        saveState("creative", { nodes: state.nodes, edges: state.edges });
      }, 500);
    });

    const unsubPro = useProfessionalStore.subscribe((state) => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        saveState("professional", { nodes: state.nodes, edges: state.edges });
      }, 500);
    });

    return () => {
      unsubCreative();
      unsubPro();
      if (timer) clearTimeout(timer);
    };
  }, []);
}
