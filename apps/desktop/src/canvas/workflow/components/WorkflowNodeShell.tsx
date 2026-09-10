import { memo } from "react";
import { Handle, NodeToolbar, Position, type NodeProps } from "@xyflow/react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { faPlay, faTrashCan } from "@fortawesome/free-solid-svg-icons";
import type { WorkflowNodeData, NodeStatus, CreativeNodeData, ProfessionalNodeData } from "../types";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { runWorkflow } from "../utils/workflowExecutor";
import { NodeConfigForm } from "./NodeConfigForm";
import { TextGenContent } from "../nodes/creative/TextGenNode";
import { ImageGenContent } from "../nodes/creative/ImageGenNode";
import { ImageCompareContent } from "../nodes/creative/ImageCompareContent";
import { ImageEditContent } from "../nodes/creative/ImageEditContent";
import { ScriptGenContent } from "../nodes/creative/ScriptGenContent";
import { VideoGenContent } from "../nodes/creative/VideoGenContent";
import { VideoStitchContent } from "../nodes/creative/VideoStitchContent";
import {
  StartContent,
  EndContent,
  IfElseContent,
  IterationContent,
  LLMContent,
  AgentContent,
  QuestionClassifierContent,
  ParameterExtractorContent,
  KnowledgeRetrievalContent,
  CodeContent,
  HttpRequestContent,
  TemplateTransformContent,
  VariableAggregatorContent,
  ToolContent,
  HumanInputContent,
} from "../nodes/professional/ProfessionalNodes";

const STATUS_BORDER: Record<NodeStatus, string> = {
  idle: "1px solid rgba(255,255,255,0.1)",
  running: "1.5px dashed rgba(59,130,246,0.7)",
  completed: "1px solid rgba(34,197,94,0.7)",
  error: "1px solid rgba(239,68,68,0.7)",
};

const STATUS_SHADOW: Record<NodeStatus, string> = {
  idle: "none",
  running: "0 0 12px rgba(59,130,246,0.25)",
  completed: "0 0 8px rgba(34,197,94,0.15)",
  error: "0 0 12px rgba(239,68,68,0.25)",
};

const STATUS_LABEL: Record<NodeStatus, string> = {
  idle: "",
  running: "运行中...",
  completed: "已完成",
  error: "失败",
};

const STATUS_DOT_COLOR: Record<NodeStatus, string> = {
  idle: "rgba(255,255,255,0.3)",
  running: "#3b82f6",
  completed: "#22c55e",
  error: "#ef4444",
};

function renderContent(nodeData: WorkflowNodeData) {
  const kind = nodeData.kind;
  const creativeData = nodeData as CreativeNodeData;
  const professionalData = nodeData as ProfessionalNodeData;

  switch (kind) {
    case "text-gen":
      return <TextGenContent data={creativeData} />;
    case "image-gen":
      return <ImageGenContent data={creativeData} />;
    case "image-compare":
      return <ImageCompareContent data={creativeData} />;
    case "image-edit":
      return <ImageEditContent data={creativeData} />;
    case "script-gen":
      return <ScriptGenContent data={creativeData} />;
    case "video-gen":
      return <VideoGenContent data={creativeData} />;
    case "video-stitch":
      return <VideoStitchContent data={creativeData} />;
    case "start":
      return <StartContent data={professionalData} />;
    case "end":
      return <EndContent data={professionalData} />;
    case "if-else":
      return <IfElseContent data={professionalData} />;
    case "iteration":
      return <IterationContent />;
    case "llm":
      return <LLMContent data={professionalData} />;
    case "agent":
      return <AgentContent data={professionalData} />;
    case "question-classifier":
      return <QuestionClassifierContent />;
    case "parameter-extractor":
      return <ParameterExtractorContent />;
    case "knowledge-retrieval":
      return <KnowledgeRetrievalContent data={professionalData} />;
    case "code":
      return <CodeContent data={professionalData} />;
    case "http-request":
      return <HttpRequestContent data={professionalData} />;
    case "template-transform":
      return <TemplateTransformContent />;
    case "variable-aggregator":
      return <VariableAggregatorContent />;
    case "tool":
      return <ToolContent data={professionalData} />;
    case "human-input":
      return <HumanInputContent />;
    default:
      return null;
  }
}

const TOOLBAR_PILL_STYLE: React.CSSProperties = {
  background: "rgba(76,80,82,0.55)",
  border: "1px solid rgba(255,255,255,0.08)",
  boxShadow: "0 6px 24px rgba(0,0,0,0.24)",
  backdropFilter: "blur(40px)",
  WebkitBackdropFilter: "blur(40px)",
  borderRadius: 10,
};

const EDIT_PANEL_STYLE: React.CSSProperties = {
  width: 340,
  maxHeight: 340,
  overflowY: "auto",
  background: "#101010",
  border: "1px solid rgba(255,255,255,0.1)",
  borderRadius: 16,
  boxShadow: "0 14px 44px rgba(0,0,0,0.4)",
  padding: 12,
};

const TOOLBAR_BTN_CLASS =
  "flex h-6 w-6 items-center justify-center rounded-md transition-colors duration-200 hover:bg-white/10 active:scale-95";

function WorkflowNodeShellInner({ id, data, selected }: NodeProps) {
  const nodeData = data as unknown as WorkflowNodeData;
  const nodeStatus = nodeData.status ?? "idle";
  const nodeLabel = nodeData.label ?? "节点";
  const mode = useCanvasStore((s) => s.mode);

  const handleRun = () => {
    void runWorkflow(id);
  };

  const handleDelete = () => {
    if (mode === "creative") useCreativeStore.getState().removeNode(id);
    else useProfessionalStore.getState().removeNode(id);
    useCanvasStore.getState().setSelectedNodeId(null);
  };

  return (
    <>
      {/* Floating toolbar (single-selected only) */}
      <NodeToolbar position={Position.Top} offset={34}>
        <div className="flex items-center gap-0.5 px-1" style={{ ...TOOLBAR_PILL_STYLE, height: 32 }}>
          <button className={TOOLBAR_BTN_CLASS} onClick={handleRun} title="运行此节点">
            <FontAwesomeIcon icon={faPlay} style={{ fontSize: 11, color: "rgba(248,248,248,0.7)" }} />
          </button>
          <button className={TOOLBAR_BTN_CLASS} onClick={handleDelete} title="删除节点">
            <FontAwesomeIcon icon={faTrashCan} style={{ fontSize: 11, color: "rgba(248,248,248,0.7)" }} />
          </button>
        </div>
      </NodeToolbar>

      {/* Floating edit panel below node (single-selected only) */}
      <NodeToolbar position={Position.Bottom} offset={12}>
        <div style={EDIT_PANEL_STYLE}>
          <NodeConfigForm nodeId={id} nodeData={nodeData} />
        </div>
      </NodeToolbar>

      <div
      className="relative select-none"
      style={{
        width: 240,
        borderRadius: 16,
        background: "#101010",
        border: STATUS_BORDER[nodeStatus],
        boxShadow: selected
          ? "0 0 0 3px rgba(139,124,247,0.35), 0 14px 44px rgba(0,0,0,0.4)"
          : STATUS_SHADOW[nodeStatus],
        backdropFilter: "blur(12px)",
        transition: "border-color 0.3s, box-shadow 0.3s, transform 0.15s ease",
      }}
    >
      {/* Selection glow halo — outer ring */}
      {selected && (
        <>
          <div
            className="absolute pointer-events-none"
            style={{
              inset: -3,
              borderRadius: 19,
              background:
                "conic-gradient(from var(--glow-angle, 0deg), rgba(0,0,0,0) 0%, #8b7cf7 4%, #a78bfa 10%, rgba(0,0,0,0) 22%, rgba(0,0,0,0) 45%, #ff7ec7 55%, #ff9ecf 62%, rgba(0,0,0,0) 75%)",
              WebkitMask: "linear-gradient(#fff 0 0) content-box, linear-gradient(#fff 0 0)",
              WebkitMaskComposite: "xor",
              maskComposite: "exclude",
              padding: 3,
              opacity: 0.65,
              animation: "wfGlowRotate 4s linear infinite",
            }}
          />
          <div
            className="absolute pointer-events-none"
            style={{
              inset: -2,
              borderRadius: 18,
              background:
                "conic-gradient(from var(--glow-angle, 0deg), rgba(0,0,0,0) 0%, #c8bfff 3%, #e0d9ff 8%, rgba(0,0,0,0) 14%, rgba(0,0,0,0) 48%, #ffc0e0 54%, #ffd0e8 60%, rgba(0,0,0,0) 68%)",
              WebkitMask: "linear-gradient(#fff 0 0) content-box, linear-gradient(#fff 0 0)",
              WebkitMaskComposite: "xor",
              maskComposite: "exclude",
              padding: 2,
              opacity: 0.75,
              animation: "wfGlowRotate 4s linear infinite",
            }}
          />
        </>
      )}

      {/* Title above node */}
      <div
        className="absolute left-0 flex items-center gap-1.5 select-none"
        style={{ top: -22, color: "rgba(255,255,255,0.8)" }}
      >
        <span className="text-xs font-semibold" style={{ letterSpacing: "0.3px" }}>
          {nodeLabel}
        </span>
      </div>

      {/* Card content */}
      <div className="px-3 py-2.5">
        {/* Node content by kind */}
        {renderContent(nodeData)}

        {/* Status line */}
        <div className="mt-2 flex items-center gap-1.5">
          <div
            className="rounded-full"
            style={{
              width: 6,
              height: 6,
              background: STATUS_DOT_COLOR[nodeStatus],
              boxShadow: nodeStatus === "running" ? "0 0 6px rgba(59,130,246,0.5)" : "none",
              animation: nodeStatus === "running" ? "pulse 1.5s ease-in-out infinite" : "none",
            }}
          />
          <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.45)" }}>
            {STATUS_LABEL[nodeStatus] || "就绪"}
          </span>
        </div>
      </div>

      {/* Connection handles */}
      <Handle
        type="target"
        position={Position.Left}
        style={{ left: -4 }}
      />
      <Handle
        type="source"
        position={Position.Right}
        style={{ right: -4 }}
      />

      {/* Running progress bar */}
      {nodeStatus === "running" && "videoProgress" in nodeData && nodeData.videoProgress != null && (
        <div className="absolute bottom-0 left-0 right-0 h-0.5 rounded-b-xl overflow-hidden">
          <div
            className="h-full transition-all duration-300"
            style={{
              width: `${nodeData.videoProgress}%`,
              background: "linear-gradient(90deg, #3b82f6, #8b5cf6)",
            }}
          />
        </div>
      )}
    </div>
    </>
  );
}

export const WorkflowNodeShell = memo(WorkflowNodeShellInner);
