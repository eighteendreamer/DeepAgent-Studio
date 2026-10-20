import { useCallback, useMemo } from "react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { getAllNodeDefinitions, normalizeProfessionalData } from "../utils/nodeRegistry";
import type { WorkflowNodeData, CreativeNodeData, ProfessionalNodeData } from "../types";
import { TextGenEditorPanel } from "./TextGenEditorPanel";
import { SchemaConfigForm } from "./SchemaConfigForm";
import { VideoEditForm } from "./VideoEditForm";
import {
  ImageGenForm,
  VideoGenForm,
  ScriptGenForm,
  ImageEditForm,
  ImageCompareForm,
  VideoStitchForm,
  AudioForm,
  GenericConfigForm,
} from "./ConfigForms";

export function NodeConfigForm({ nodeId, nodeData, nodeType }: {
  nodeId: string;
  nodeData: WorkflowNodeData;
  nodeType?: string;
}) {
  const mode = useCanvasStore((state) => state.mode);
  const creativeNodes = useCreativeStore((state) => state.nodes);
  const creativeEdges = useCreativeStore((state) => state.edges);
  const data = useMemo(() => normalizeProfessionalData(nodeData, nodeType), [nodeData, nodeType]);
  const handleUpdate = useCallback((patch: Record<string, unknown>) => {
    const store = mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
    store.updateNodeData(nodeId, patch);
  }, [mode, nodeId]);

  const videoEditData = useMemo(() => {
    if (data.kind !== "video-edit" || mode !== "creative") return data as CreativeNodeData;
    const connectedRefs = creativeEdges
      .filter((edge) => edge.target === nodeId)
      .map((edge) => creativeNodes.find((node) => node.id === edge.source)?.data)
      .map((source) => source?.videoUrl ?? (source?.videoEditStatus === "exported" ? source.videoEditOutputRef : undefined))
      .filter((reference): reference is string => typeof reference === "string" && reference.startsWith("artifact://"));
    const ownRefs = (data as CreativeNodeData).videoEditInputRefs ?? [];
    const selectedRefs = ownRefs.length ? ownRefs : connectedRefs.slice(0, 1);
    return { ...data, videoEditInputRefs: [...new Set(selectedRefs)] } as CreativeNodeData;
  }, [data, mode, creativeEdges, creativeNodes, nodeId]);

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
    case "video-edit":
      return <VideoEditForm data={videoEditData} onUpdate={handleUpdate} />;
    case "script-gen":
      return <ScriptGenForm data={creativeData} {...formProps} />;
    case "image-edit":
      return <ImageEditForm data={creativeData} {...formProps} />;
    case "image-compare":
      return <ImageCompareForm data={creativeData} {...formProps} />;
    case "video-stitch":
      return <VideoStitchForm data={creativeData} {...formProps} />;
    case "audio":
      return <AudioForm data={creativeData} {...formProps} />;
    default:
      return <GenericConfigForm data={data} />;
  }
}
