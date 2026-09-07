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

  configPanelOpen: boolean;
  setConfigPanelOpen: (open: boolean) => void;
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
  setSelectedNodeId: (id) => set({ selectedNodeId: id, configPanelOpen: id != null }),

  nodePicker: null,
  openNodePicker: (pos) => set({ nodePicker: pos }),
  closeNodePicker: () => set({ nodePicker: null }),

  configPanelOpen: false,
  setConfigPanelOpen: (open) => set({ configPanelOpen: open }),
}));
