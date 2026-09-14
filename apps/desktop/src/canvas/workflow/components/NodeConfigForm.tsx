import { useCallback } from "react";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import type { WorkflowNodeData, CreativeNodeData, ProfessionalNodeData } from "../types";
import { TextGenEditorPanel } from "./TextGenEditorPanel";
import {
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
  EndForm,
  IfElseForm,
  IterationForm,
  QuestionClassifierForm,
  ParameterExtractorForm,
  TemplateTransformForm,
  VariableAggregatorForm,
  HumanInputForm,
  AnswerForm,
  LoopForm,
  IterationStartForm,
  LoopStartForm,
  LoopEndForm,
  AgentV2Form,
  DocumentExtractorForm,
  VariableAssignerForm,
  ListOperatorForm,
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
    case "end":
      return <EndForm data={professionalData} {...formProps} />;
    case "if-else":
      return <IfElseForm data={professionalData} {...formProps} />;
    case "iteration":
      return <IterationForm data={professionalData} {...formProps} />;
    case "question-classifier":
      return <QuestionClassifierForm data={professionalData} {...formProps} />;
    case "parameter-extractor":
      return <ParameterExtractorForm data={professionalData} {...formProps} />;
    case "template-transform":
      return <TemplateTransformForm data={professionalData} {...formProps} />;
    case "variable-aggregator":
      return <VariableAggregatorForm data={professionalData} {...formProps} />;
    case "human-input":
      return <HumanInputForm data={professionalData} {...formProps} />;
    case "answer":
      return <AnswerForm data={professionalData} {...formProps} />;
    case "loop":
      return <LoopForm data={professionalData} {...formProps} />;
    case "iteration-start":
      return <IterationStartForm data={professionalData} {...formProps} />;
    case "loop-start":
      return <LoopStartForm data={professionalData} {...formProps} />;
    case "loop-end":
      return <LoopEndForm data={professionalData} {...formProps} />;
    case "agent-v2":
      return <AgentV2Form data={professionalData} {...formProps} />;
    case "document-extractor":
      return <DocumentExtractorForm data={professionalData} {...formProps} />;
    case "variable-assigner":
      return <VariableAssignerForm data={professionalData} {...formProps} />;
    case "list-operator":
      return <ListOperatorForm data={professionalData} {...formProps} />;
    default:
      return <GenericConfigForm data={nodeData} />;
  }
}
