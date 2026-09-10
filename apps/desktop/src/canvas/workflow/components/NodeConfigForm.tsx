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

export function NodeConfigForm({ nodeId, nodeData }: { nodeId: string; nodeData: WorkflowNodeData }) {
  const mode = useCanvasStore((s) => s.mode);

  const handleUpdate = useCallback(
    (patch: Record<string, unknown>) => {
      if (mode === "creative") {
        useCreativeStore.getState().updateNodeData(nodeId, patch);
      } else {
        useProfessionalStore.getState().updateNodeData(nodeId, patch);
      }
    },
    [mode, nodeId],
  );

  const creativeData = nodeData as CreativeNodeData;
  const professionalData = nodeData as ProfessionalNodeData;
  const formProps = { nodeId, onUpdate: handleUpdate };

  switch (nodeData.kind) {
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
      return <GenericConfigForm data={nodeData} />;
  }
}
