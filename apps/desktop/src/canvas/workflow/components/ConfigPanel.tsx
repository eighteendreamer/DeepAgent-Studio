import { useCallback } from "react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNodeData, CreativeNodeData, ProfessionalNodeData } from "../types";
import {
  TextGenForm,
  ImageGenForm,
  VideoGenForm,
  ScriptGenForm,
  ImageEditForm,
  ImageCompareForm,
  VideoStitchForm,
  LLMForm,
  CodeForm,
  HttpRequestForm,
  AgentForm,
  StartForm,
  KnowledgeForm,
  GenericConfigForm,
} from "./ConfigForms";
import { runWorkflow } from "../utils/workflowExecutor";

export function ConfigPanel() {
  const mode = useCanvasStore((s) => s.mode);
  const selectedNodeId = useCanvasStore((s) => s.selectedNodeId);
  const configPanelOpen = useCanvasStore((s) => s.configPanelOpen);
  const setConfigPanelOpen = useCanvasStore((s) => s.setConfigPanelOpen);
  const setSelectedNodeId = useCanvasStore((s) => s.setSelectedNodeId);

  const creativeNodes = useCreativeStore((s) => s.nodes);
  const professionalNodes = useProfessionalStore((s) => s.nodes);
  const nodes = mode === "creative" ? creativeNodes : professionalNodes;
  const selectedNode = nodes.find((n) => n.id === selectedNodeId);

  const handleUpdate = useCallback(
    (patch: Record<string, unknown>) => {
      if (!selectedNodeId) return;
      if (mode === "creative") {
        useCreativeStore.getState().updateNodeData(selectedNodeId, patch);
      } else {
        useProfessionalStore.getState().updateNodeData(selectedNodeId, patch);
      }
    },
    [selectedNodeId, mode],
  );

  if (!configPanelOpen || !selectedNode) return null;

  const data = selectedNode.data as WorkflowNodeData;
  const label = data.label ?? "节点";
  const kind = data.kind;

  const renderForm = () => {
    const creativeData = data as CreativeNodeData;
    const professionalData = data as ProfessionalNodeData;
    const formProps = { nodeId: selectedNode.id, onUpdate: handleUpdate };

    switch (kind) {
      case "text-gen":
        return <TextGenForm data={creativeData} {...formProps} />;
      case "image-gen":
        return <ImageGenForm data={creativeData} {...formProps} />;
      case "video-gen":
        return <VideoGenForm data={creativeData} {...formProps} />;
      case "script-gen":
        return <ScriptGenForm data={creativeData} {...formProps} />;
      case "image-edit":
        return <ImageEditForm data={creativeData} {...formProps} />;
      case "image-compare":
        return <ImageCompareForm data={creativeData} {...formProps} />;
      case "video-stitch":
        return <VideoStitchForm data={creativeData} {...formProps} />;
      case "llm":
        return <LLMForm data={professionalData} {...formProps} />;
      case "code":
        return <CodeForm data={professionalData} {...formProps} />;
      case "http-request":
        return <HttpRequestForm data={professionalData} {...formProps} />;
      case "agent":
        return <AgentForm data={professionalData} {...formProps} />;
      case "start":
        return <StartForm data={professionalData} {...formProps} />;
      case "knowledge-retrieval":
        return <KnowledgeForm data={professionalData} {...formProps} />;
      default:
        return <GenericConfigForm data={data} />;
    }
  };

  return (
    <div
      className="fixed top-0 right-0 bottom-0 z-[9996] flex flex-col"
      style={{
        width: mode === "professional" ? 360 : 400,
        background: "rgba(30,30,35,0.85)",
        border: "1px solid rgba(255,255,255,0.08)",
        borderRight: "none",
        backdropFilter: "blur(40px)",
        boxShadow: "-8px 0 40px rgba(0,0,0,0.3)",
        animation: "wfSlideIn 0.2s ease-out",
      }}
    >
      {/* Header */}
      <div
        className="flex items-center justify-between px-4 py-3"
        style={{ borderBottom: "1px solid rgba(255,255,255,0.08)" }}
      >
        <span className="text-sm font-medium" style={{ color: "rgba(248,248,248,0.9)" }}>
          {label}
        </span>
        <button
          onClick={() => {
            setConfigPanelOpen(false);
            setSelectedNodeId(null);
          }}
          className="flex h-6 w-6 items-center justify-center rounded-md transition-colors hover:bg-white/10"
          style={{ color: "rgba(248,248,248,0.5)" }}
        >
          ✕
        </button>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {renderForm()}
      </div>

      {/* Footer - Run button */}
      <div
        className="px-4 py-3"
        style={{ borderTop: "1px solid rgba(255,255,255,0.08)" }}
      >
        <button
          className="flex w-full items-center justify-center gap-2 rounded-xl py-2 text-xs font-medium transition-all duration-300 active:scale-[0.98]"
          style={{
            background: "rgba(59,130,246,0.8)",
            color: "rgb(248,248,248)",
          }}
          onClick={() => {
            if (selectedNodeId) void runWorkflow(selectedNodeId);
          }}
        >
          ▶ 运行此节点
        </button>
      </div>
    </div>
  );
}
