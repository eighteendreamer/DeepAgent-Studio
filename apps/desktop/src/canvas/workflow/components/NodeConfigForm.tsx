import { useCallback } from "react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { getAllNodeDefinitions, normalizeProfessionalData } from "../utils/nodeRegistry";
import type { WorkflowNodeData, CreativeNodeData, ProfessionalNodeData } from "../types";
import { TextGenEditorPanel } from "./TextGenEditorPanel";
import { SchemaConfigForm } from "./SchemaConfigForm";
import {
  ImageGenForm,
  VideoGenForm,
  ScriptGenForm,
  ImageEditForm,
  ImageCompareForm,
  VideoStitchForm,
  GenericConfigForm,
} from "./ConfigForms";

export function NodeConfigForm({ nodeId, nodeData, nodeType }: {
  nodeId: string;
  nodeData: WorkflowNodeData;
  nodeType?: string;
}) {
  const mode = useCanvasStore((state) => state.mode);
  const data = normalizeProfessionalData(nodeData, nodeType);
  const handleUpdate = useCallback((patch: Record<string, unknown>) => {
    const store = mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
    store.updateNodeData(nodeId, patch);
  }, [mode, nodeId]);

  if (getAllNodeDefinitions().some((definition) => definition.kind === data.kind)) {
    return <SchemaConfigForm nodeId={nodeId} data={data as ProfessionalNodeData} onUpdate={handleUpdate} />;
  }

  const creativeData = data as CreativeNodeData;
  const formProps = { nodeId, onUpdate: handleUpdate };
  switch (data.kind) {
    case "text-gen":
      return <TextGenEditorPanel nodeId={nodeId} data={creativeData} onUpdate={handleUpdate} />;
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
    default:
      return <GenericConfigForm data={data} />;
  }
}
