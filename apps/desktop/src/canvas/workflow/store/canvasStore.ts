import { create } from "zustand";
import type { CanvasMode, NodePickerPosition } from "../types";
import type { Viewport } from "@xyflow/react";

interface CanvasState {
  mode: CanvasMode;
  setMode: (mode: CanvasMode) => void;

  viewport: Viewport;
  setViewport: (vp: Viewport) => void;

  gridVisible: boolean;
  toggleGrid: () => void;

  snapToGrid: boolean;
  toggleSnap: () => void;

  selectedNodeId: string | null;
  setSelectedNodeId: (id: string | null) => void;

  nodePicker: NodePickerPosition | null;
  openNodePicker: (pos: NodePickerPosition) => void;
  closeNodePicker: () => void;

  pendingConnection: { nodeId: string; handleType: "source" | "target" } | null;
  setPendingConnection: (c: { nodeId: string; handleType: "source" | "target" } | null) => void;

  settingsOpen: boolean;
  openSettings: () => void;
  closeSettings: () => void;
}

export const useCanvasStore = create<CanvasState>((set) => ({
  mode: "creative",
  setMode: (mode) => set({ mode }),

  viewport: { x: 0, y: 0, zoom: 1 },
  setViewport: (viewport) => set({ viewport }),

  gridVisible: true,
  toggleGrid: () => set((s) => ({ gridVisible: !s.gridVisible })),

  snapToGrid: false,
  toggleSnap: () => set((s) => ({ snapToGrid: !s.snapToGrid })),

  selectedNodeId: null,
  setSelectedNodeId: (id) => set({ selectedNodeId: id }),

  nodePicker: null,
  openNodePicker: (pos) => set({ nodePicker: pos }),
  closeNodePicker: () => set({ nodePicker: null, pendingConnection: null }),

  pendingConnection: null,
  setPendingConnection: (c) => set({ pendingConnection: c }),

  settingsOpen: false,
  openSettings: () => set({ settingsOpen: true }),
  closeSettings: () => set({ settingsOpen: false }),
}));
