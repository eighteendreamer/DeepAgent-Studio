import type { Node, Edge } from "@xyflow/react";

export type CanvasMode = "creative" | "professional";
export type NodeStatus = "idle" | "running" | "completed" | "error";

export type CreativeNodeKind =
  | "text-gen"
  | "image-gen"
  | "image-compare"
  | "image-edit"
  | "script-gen"
  | "video-gen"
  | "video-stitch";

export type ProfessionalNodeKind =
  | "start"
  | "end"
  | "if-else"
  | "iteration"
  | "llm"
  | "agent"
  | "question-classifier"
  | "parameter-extractor"
  | "knowledge-retrieval"
  | "code"
  | "http-request"
  | "template-transform"
  | "variable-aggregator"
  | "tool"
  | "human-input";

export type WorkflowNodeKind = CreativeNodeKind | ProfessionalNodeKind;

export interface CreativeNodeData {
  label: string;
  kind: CreativeNodeKind;
  status: NodeStatus;
  errorMessage?: string;
  prompt?: string;
  model?: string;
  output?: string;
  imageModel?: string;
  imagePrompt?: string;
  imageUrl?: string;
  aspectRatio?: string;
  resolution?: string;
  videoService?: "sora" | "veo" | "kling";
  videoModel?: string;
  videoPrompt?: string;
  videoInputUrl?: string;
  videoUrl?: string;
  videoDuration?: number;
  videoProgress?: number;
  videoTaskId?: string;
  leftImageUrl?: string;
  rightImageUrl?: string;
  editMode?: "crop" | "remove-bg" | "upscale" | "repaint";
  inputVideoUrls?: string[];
  result?: unknown;
  executionTime?: number;
  [key: string]: unknown;
}

export interface ProfessionalNodeData {
  label: string;
  description?: string;
  kind: ProfessionalNodeKind;
  status: NodeStatus;
  errorMessage?: string;
  inputVariables?: Array<{ name: string; type: string; required?: boolean }>;
  outputMapping?: Record<string, string>;
  conditions?: Array<{ variable: string; operator: string; value: string }>;
  llmModel?: string;
  llmPrompt?: string;
  llmSystemPrompt?: string;
  llmTemperature?: number;
  llmMaxTokens?: number;
  agentTools?: string[];
  agentStrategy?: "function-call" | "react";
  codeLanguage?: "javascript" | "python";
  codeScript?: string;
  httpMethod?: "GET" | "POST" | "PUT" | "DELETE";
  httpUrl?: string;
  httpHeaders?: Record<string, string>;
  httpBody?: string;
  knowledgeBaseId?: string;
  knowledgeTopK?: number;
  toolId?: string;
  toolParams?: Record<string, unknown>;
  result?: unknown;
  executionTime?: number;
  [key: string]: unknown;
}

export type WorkflowNodeData = CreativeNodeData | ProfessionalNodeData;

export type WorkflowNode = Node<WorkflowNodeData, string>;
export type WorkflowEdge = Edge<{ dataType?: string }>;

export interface NodePickerPosition {
  x: number;
  y: number;
  worldX: number;
  worldY: number;
}

export interface CreativeNodeCategory {
  group: string;
  items: Array<{ kind: CreativeNodeKind; label: string; icon: string }>;
}

export interface ProfessionalNodeCategory {
  group: string;
  color: string;
  items: Array<{ kind: ProfessionalNodeKind; label: string; icon: string }>;
}

export const CREATIVE_NODE_CATEGORIES: CreativeNodeCategory[] = [
  {
    group: "图文创作",
    items: [
      { kind: "text-gen", label: "文本生成", icon: "pen" },
      { kind: "image-gen", label: "图片生成", icon: "image" },
      { kind: "image-compare", label: "图片对比", icon: "left-right" },
      { kind: "image-edit", label: "图片编辑", icon: "wand-magic-sparkles" },
    ],
  },
  {
    group: "视频创作",
    items: [
      { kind: "script-gen", label: "脚本生成", icon: "file-lines" },
      { kind: "video-gen", label: "视频生成", icon: "video" },
      { kind: "video-stitch", label: "视频拼接", icon: "film" },
    ],
  },
];

export const PROFESSIONAL_NODE_CATEGORIES: ProfessionalNodeCategory[] = [
  {
    group: "流程控制",
    color: "#3B82F6",
    items: [
      { kind: "start", label: "开始", icon: "play" },
      { kind: "end", label: "结束", icon: "stop" },
      { kind: "if-else", label: "条件分支", icon: "code-branch" },
      { kind: "iteration", label: "迭代", icon: "rotate" },
    ],
  },
  {
    group: "AI",
    color: "#8B5CF6",
    items: [
      { kind: "llm", label: "LLM", icon: "wand-magic-sparkles" },
      { kind: "agent", label: "Agent", icon: "robot" },
      { kind: "question-classifier", label: "问题分类", icon: "tags" },
      { kind: "parameter-extractor", label: "参数提取", icon: "table-columns" },
    ],
  },
  {
    group: "知识",
    color: "#06B6D4",
    items: [{ kind: "knowledge-retrieval", label: "知识检索", icon: "book" }],
  },
  {
    group: "数据",
    color: "#10B981",
    items: [
      { kind: "code", label: "代码执行", icon: "code" },
      { kind: "http-request", label: "HTTP 请求", icon: "globe" },
      { kind: "template-transform", label: "模板转换", icon: "file-code" },
      { kind: "variable-aggregator", label: "变量聚合", icon: "layer-group" },
    ],
  },
  {
    group: "工具",
    color: "#F59E0B",
    items: [
      { kind: "tool", label: "工具调用", icon: "wrench" },
      { kind: "human-input", label: "人工审批", icon: "user-check" },
    ],
  },
];
