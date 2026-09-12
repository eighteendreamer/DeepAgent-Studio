import { memo, useEffect, useRef, useState } from "react";
import { Handle, NodeToolbar, Position, type NodeProps } from "@xyflow/react";
import type {
  WorkflowNodeData,
  NodeStatus,
  CreativeNodeData,
  ProfessionalNodeData,
  CreativeNodeKind,
  ProfessionalNodeKind,
} from "../types";
import { useCanvasStore } from "../store/canvasStore";
import { useCreativeStore } from "../store/creativeStore";
import { useProfessionalStore } from "../store/professionalStore";
import { runWorkflow } from "../utils/workflowExecutor";
import { NodeConfigForm } from "./NodeConfigForm";
import { NodeFloatingToolbar } from "./NodeFloatingToolbar";
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

const EDIT_PANEL_STYLE: React.CSSProperties = {
  width: 500,
  maxHeight: 380,
  overflowY: "auto",
  background: "rgba(24,24,27,0.92)",
  border: "1px solid rgba(255,255,255,0.08)",
  borderRadius: 12,
  boxShadow: "0 24px 64px rgba(0,0,0,0.4)",
  backdropFilter: "blur(40px)",
  WebkitBackdropFilter: "blur(40px)",
  padding: 12,
};

// asset 协议等跨域 URL 会让 <a download> 失效并直接导航打开，必须先转成同源 blob URL
const triggerFileDownload = async (url: string, filename: string) => {
  let href = url;
  if (!url.startsWith("data:") && !url.startsWith("blob:")) {
    try {
      const blob = await fetch(url).then((r) => {
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        return r.blob();
      });
      href = URL.createObjectURL(blob);
    } catch (err) {
      console.error("[WorkflowNodeShell] 下载前拉取资源失败，回退直链:", url, err);
    }
  }
  const a = document.createElement("a");
  a.href = href;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  if (href !== url) setTimeout(() => URL.revokeObjectURL(href), 1000);
};

function WorkflowNodeShellInner({ id, data, selected }: NodeProps) {
  const nodeData = data as unknown as WorkflowNodeData;
  const nodeStatus = nodeData.status ?? "idle";
  const nodeLabel = nodeData.label ?? "节点";
  const mode = useCanvasStore((s) => s.mode);

  // 行内重命名态（Penguin _renameMode 语义：Enter/失焦提交、Esc 取消、空值回落默认名）
  const [renaming, setRenaming] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");
  const titleInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!renaming) return;
    const t1 = setTimeout(() => titleInputRef.current?.focus(), 30);
    const t2 = setTimeout(() => titleInputRef.current?.select(), 60);
    return () => {
      clearTimeout(t1);
      clearTimeout(t2);
    };
  }, [renaming]);

  const commitRename = (commit: boolean) => {
    const next = titleDraft.trim();
    if (commit) {
      const store = mode === "creative" ? useCreativeStore.getState() : useProfessionalStore.getState();
      store.updateNodeData(id, { label: next || "节点" } as never);
    }
    setRenaming(false);
  };

  const handleRun = () => {
    void runWorkflow(id);
  };

  const handleDelete = () => {
    if (mode === "creative") useCreativeStore.getState().removeNode(id);
    else useProfessionalStore.getState().removeNode(id);
    useCanvasStore.getState().setSelectedNodeId(null);
  };

  const handleRename = () => {
    setTitleDraft(nodeData.label ?? "");
    setRenaming(true);
  };

  const handleDuplicate = () => {
    // mode 决定 store 与节点数据同源，按 mode 分支窄化类型（同 CanvasApp 粘贴链路）
    if (mode === "creative") {
      const s = useCreativeStore.getState();
      const node = s.nodes.find((n) => n.id === id);
      if (!node) return;
      const newId = s.addNodeAt(
        nodeData.kind as CreativeNodeKind,
        node.position.x + 40,
        node.position.y + 40,
        node.data as Partial<CreativeNodeData>,
      );
      s.setSelectedIds([newId]);
      useCanvasStore.getState().setSelectedNodeId(newId);
      return;
    }
    const s = useProfessionalStore.getState();
    const node = s.nodes.find((n) => n.id === id);
    if (!node) return;
    const newId = s.addNodeAt(
      nodeData.kind as ProfessionalNodeKind,
      node.position.x + 40,
      node.position.y + 40,
      node.data as Partial<ProfessionalNodeData>,
    );
    s.setSelectedIds([newId]);
    useCanvasStore.getState().setSelectedNodeId(newId);
  };

  const handleDownload = () => {
    const kind = nodeData.kind;
    const creativeData = nodeData as CreativeNodeData;

    if (kind === "image-gen" || kind === "image-edit" || kind === "image-compare") {
      const url = creativeData.imageUrl;
      if (!url) return;
      void triggerFileDownload(url, `${nodeLabel}-${Date.now()}.png`);
      return;
    }

    if (kind === "video-gen" || kind === "video-stitch") {
      const url = creativeData.videoUrl;
      if (!url) return;
      void triggerFileDownload(url, `${nodeLabel}-${Date.now()}.mp4`);
      return;
    }

    if (kind !== "text-gen" && kind !== "script-gen") return;
    const prompt = creativeData.prompt ?? "";
    const md = `# ${nodeLabel}\n\n${prompt}`;
    const blob = new Blob([md], { type: "text/markdown;charset=utf-8" });
    const blobUrl = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = blobUrl;
    a.download = `${nodeLabel}-${Date.now()}.md`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(blobUrl);
  };

  const handleAnnotate = () => {
    const imageUrl = (nodeData as CreativeNodeData).imageUrl;
    if (!imageUrl) return;
    const { setDrawingTarget } = useCanvasStore.getState();
    setDrawingTarget({ nodeId: id, imageUrl, name: nodeLabel, mode: "annotate" });
  };

  const handleErase = () => {
    const imageUrl = (nodeData as CreativeNodeData).imageUrl;
    if (!imageUrl) return;
    const { setDrawingTarget } = useCanvasStore.getState();
    setDrawingTarget({ nodeId: id, imageUrl, name: nodeLabel, mode: "erase" });
  };

  const handleCreativeLibrary = () => {
    const { setCreativeLibraryOpen } = useCanvasStore.getState();
    setCreativeLibraryOpen(true);
  };

  const handleStoryboardPreset = (key: string) => {
    const storyboardLabels: Record<string, string> = {
      "plot-4": "4宫格剧情推演",
      "multi-cam-9": "9宫格多机位",
      "continuous-25": "25宫格连贯分镜",
      "char-3view": "角色3视图",
      "char-4view": "角色4视图",
      "char-design-sheet": "角色设定图",
      "emoji-grid-9": "9宫格表情包",
      "scene-after-3": "画面推演·3秒后",
      "scene-before-5": "画面回推·5秒前",
      "depth-parallax": "深度视差",
    };
    const label = storyboardLabels[key] ?? key;
    useCreativeStore.getState().updateNodeData(id, {
      imagePrompt: `[${label}] 请基于当前图片生成分镜变体`,
      status: "idle",
    });
  };

  const handleCrop = () => {
    const imageUrl = (nodeData as CreativeNodeData).imageUrl;
    if (!imageUrl) return;
    const { setCropTarget } = useCanvasStore.getState();
    setCropTarget({ nodeId: id, imageUrl, name: nodeLabel });
  };

  const handleSaveAsset = (categoryKey: string) => {
    const imageUrl = (nodeData as CreativeNodeData).imageUrl;
    if (!imageUrl) return;
    try {
      const raw = localStorage.getItem("canvas-creative-library");
      const library: Array<{ id: string; name: string; imageUrl: string; category: string; createdAt: string }> = raw ? JSON.parse(raw) : [];
      library.unshift({
        id: `asset-${Date.now()}`,
        name: nodeLabel,
        imageUrl,
        category: categoryKey,
        createdAt: new Date().toISOString(),
      });
      localStorage.setItem("canvas-creative-library", JSON.stringify(library));
    } catch { /* storage full or corrupt */ }
  };

  return (
    <>
      {/* Floating toolbar (single-selected only) — Penguin-Magic 全量工具栏，生成类按钮待后端接入 */}
      <NodeToolbar position={Position.Top} offset={34}>
        <NodeFloatingToolbar
          kind={nodeData.kind}
          onRun={handleRun}
          onDelete={handleDelete}
          onRename={handleRename}
          onDuplicate={handleDuplicate}
          onDownload={handleDownload}
          onCrop={handleCrop}
          onAnnotate={handleAnnotate}
          onErase={handleErase}
          onCreativeLibrary={handleCreativeLibrary}
          onSaveAsset={handleSaveAsset}
          onStoryboardPreset={handleStoryboardPreset}
        />
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
        {renaming ? (
          <input
            ref={titleInputRef}
            value={titleDraft}
            placeholder="Name"
            onChange={(e) => setTitleDraft(e.target.value)}
            onMouseDown={(e) => e.stopPropagation()}
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Enter") {
                e.preventDefault();
                commitRename(true);
              }
              if (e.key === "Escape") {
                e.preventDefault();
                commitRename(false);
              }
            }}
            onBlur={() => commitRename(true)}
            className="outline-none bg-transparent text-xs font-semibold truncate"
            style={{
              color: "rgba(255,255,255,0.85)",
              borderBottom: "1px solid #3b82f6",
              paddingBottom: 1,
              maxWidth: 160,
            }}
          />
        ) : (
          <span className="text-xs font-semibold" style={{ letterSpacing: "0.3px" }}>
            {nodeLabel}
          </span>
        )}
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
