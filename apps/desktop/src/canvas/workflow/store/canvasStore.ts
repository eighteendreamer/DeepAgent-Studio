import { create } from "zustand";
import type { CanvasMode, NodePickerPosition } from "../types";
import type { Viewport } from "@xyflow/react";

interface CropTarget {
  nodeId: string;
  imageUrl: string;
  name: string;
  ratio?: string;
}

interface DrawingTarget {
  nodeId: string;
  imageUrl: string;
  name: string;
  mode: "annotate" | "erase";
}

interface OutpaintTarget {
  nodeId: string;
  imageUrl: string;
  name: string;
}

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

  cropTarget: CropTarget | null;
  setCropTarget: (t: CropTarget | null) => void;

  drawingTarget: DrawingTarget | null;
  setDrawingTarget: (t: DrawingTarget | null) => void;

  outpaintTarget: OutpaintTarget | null;
  setOutpaintTarget: (t: OutpaintTarget | null) => void;

  creativeLibraryOpen: boolean;
  setCreativeLibraryOpen: (open: boolean) => void;

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

  cropTarget: null,
  setCropTarget: (t) => set({ cropTarget: t }),

  drawingTarget: null,
  setDrawingTarget: (t) => set({ drawingTarget: t }),

  outpaintTarget: null,
  setOutpaintTarget: (t) => set({ outpaintTarget: t }),

  creativeLibraryOpen: false,
  setCreativeLibraryOpen: (open) => set({ creativeLibraryOpen: open }),

  settingsOpen: false,
  openSettings: () => set({ settingsOpen: true, nodePicker: null, pendingConnection: null }),
  closeSettings: () => set({ settingsOpen: false }),
}));
