import { create } from "zustand";
import type { CanvasMode, NodePickerPosition } from "../types";
import type { Viewport } from "@xyflow/react";
import type { ApprovalRequest } from "../../../types";

/** 仅投影当前运行的审批 UI；权威审批仍由内核持有。 */
export interface WorkflowApprovalView {
  runId: string;
  requests: ApprovalRequest[];
  submitting: boolean;
  error: string | null;
}

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

/**
 * 新节点建好后要建立连线的一端。
 *
 * sources 是新节点的输入（框选批量接入、从输出桩拖出），targets 是新节点的
 * 输出（从输入桩反向拖出）。两种入口共用同一份意图，不再各存一套状态。
 */
export interface PickerWiring {
  sources: string[];
  targets: string[];
}

interface CanvasState {
  workflowApproval: WorkflowApprovalView | null;
  setWorkflowApproval: (view: WorkflowApprovalView | null) => void;
  mode: CanvasMode;
  setMode: (mode: CanvasMode) => void;

  viewport: Viewport;
  viewports: Record<CanvasMode, Viewport>;
  hydratedViewports: Record<CanvasMode, boolean>;
  viewportHydrated: boolean;
  setViewport: (vp: Viewport) => void;

  gridVisible: boolean;
  toggleGrid: () => void;

  snapToGrid: boolean;
  toggleSnap: () => void;

  selectedNodeId: string | null;
  setSelectedNodeId: (id: string | null) => void;

  nodePicker: NodePickerPosition | null;
  /** 打开节点面板时携带的接线意图：新节点建好后与这些节点建立连线。 */
  pickerWiring: PickerWiring | null;
  openNodePicker: (pos: NodePickerPosition, wiring?: PickerWiring) => void;
  closeNodePicker: () => void;

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
  workflowApproval: null,
  setWorkflowApproval: (workflowApproval) => set({ workflowApproval }),
  mode: "creative",
  setMode: (mode) => set((state) => ({
    mode,
    viewport: state.viewports[mode],
    viewportHydrated: state.hydratedViewports[mode],
  })),

  viewport: { x: 0, y: 0, zoom: 1 },
  viewports: {
    creative: { x: 0, y: 0, zoom: 1 },
    professional: { x: 0, y: 0, zoom: 1 },
  },
  hydratedViewports: { creative: false, professional: false },
  viewportHydrated: false,
  setViewport: (viewport) => set((state) => ({
    viewport,
    viewports: { ...state.viewports, [state.mode]: viewport },
  })),

  gridVisible: true,
  toggleGrid: () => set((s) => ({ gridVisible: !s.gridVisible })),

  snapToGrid: false,
  toggleSnap: () => set((s) => ({ snapToGrid: !s.snapToGrid })),

  selectedNodeId: null,
  setSelectedNodeId: (id) => set({ selectedNodeId: id }),

  nodePicker: null,
  pickerWiring: null,
  openNodePicker: (pos, wiring) => set({ nodePicker: pos, pickerWiring: wiring ?? null }),
  closeNodePicker: () => set({ nodePicker: null, pickerWiring: null }),

  cropTarget: null,
  setCropTarget: (t) => set({ cropTarget: t }),

  drawingTarget: null,
  setDrawingTarget: (t) => set({ drawingTarget: t }),

  outpaintTarget: null,
  setOutpaintTarget: (t) => set({ outpaintTarget: t }),

  creativeLibraryOpen: false,
  setCreativeLibraryOpen: (open) => set({ creativeLibraryOpen: open }),

  settingsOpen: false,
  openSettings: () => set({ settingsOpen: true, nodePicker: null, pickerWiring: null }),
  closeSettings: () => set({ settingsOpen: false }),
}));
