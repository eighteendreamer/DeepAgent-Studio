import type { Node, Edge } from "@xyflow/react";

export type CanvasMode = "creative" | "professional";
export type NodeStatus = "idle" | "running" | "completed" | "error";

export type NodeAlignMode = "left" | "center-x" | "right" | "top" | "center-y" | "bottom";

export type CreativeNodeKind =
  | "category-picker"
  | "text-gen"
  | "image-input"
  | "image-gen"
  | "image-compare"
  | "image-edit"
  | "script-gen"
  | "video-gen"
  | "video-stitch"
  | "camera"
  | "lens"
  | "focal-length"
  | "aperture"
  | "director"
  | "creative-template"
  | "character-face"
  | "character-body"
  | "character-style"
  | "audio"
  | "storyboard-grid";

export type ProfessionalNodeKind =
  | "start"
  | "end"
  | "answer"
  | "if-else"
  | "iteration"
  | "iteration-start"
  | "loop"
  | "loop-start"
  | "loop-end"
  | "llm"
  | "agent"
  | "agent-v2"
  | "question-classifier"
  | "parameter-extractor"
  | "knowledge-retrieval"
  | "knowledge-index"
  | "document-extractor"
  | "datasource"
  | "code"
  | "http-request"
  | "template-transform"
  | "variable-aggregator"
  | "variable-assigner"
  | "list-operator"
  | "trigger-schedule"
  | "trigger-webhook"
  | "trigger-plugin"
  | "tool"
  | "human-input";

export type WorkflowNodeKind = CreativeNodeKind | ProfessionalNodeKind;

export interface NodeOutput {
  name: string;
  type: string;
  description?: string;
}

export type ExecutorKind = "passthrough" | "model" | "code" | "http" | "retrieval" | "control-flow" | "transform" | "human";

export interface NodeConfigSchema {
  type?: "string" | "number" | "integer" | "boolean" | "object" | "array";
  title?: string;
  description?: string;
  default?: unknown;
  enum?: Array<string | number | boolean>;
  properties?: Record<string, NodeConfigSchema>;
  items?: NodeConfigSchema;
  required?: string[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  minItems?: number;
  pattern?: string;
  format?: string;
  additionalProperties?: boolean | NodeConfigSchema;
  "x-widget"?: "textarea" | "code" | "variable" | "model" | "tool" | "json" | "password";
  "x-enum-labels"?: string[];
  "x-variable-types"?: string[];
  "x-visible-when"?: { field: string; value: unknown };
  readOnly?: boolean;
}

export interface NodeDefinition {
  kind: ProfessionalNodeKind;
  label: string;
  category: string;
  icon: string;
  availability: "enabled" | "reserved";
  executor: ExecutorKind;
  outputs: NodeOutput[];
  configSchema: NodeConfigSchema;
  defaultData: () => Record<string, unknown>;
}

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
  imageInputUrls?: string[];
  /** 内核提示词档案引用；系统提示词正文只留在后端档案里。 */
  promptProfileId?: string;
  promptProfileVersion?: number;
  aspectRatio?: string;
  resolution?: string;
  batchCount?: number;
  gptImage2Quality?: "low" | "medium" | "high" | "auto";
  customSize?: string;
  _storyboardLabel?: string;
  _storyboardKey?: string;
  _creativeLabel?: string;
  /** 一级模板和二级动作，用于在节点上保留用户选择的语义。 */
  creativeCategory?: string;
  creativeCategoryKey?: string;
  creativeAction?: string;
  creativeActionKey?: string;
  /** @deprecated 旧版供应商分段字段，仅用于兼容历史画布；新逻辑统一使用 videoModel。 */
  videoService?: string;
  videoModel?: string;
  videoPrompt?: string;
  videoInputUrl?: string;
  videoUrl?: string;
  videoDuration?: number;
  videoProgress?: number;
  videoTaskId?: string;
  /** 音频节点：方向由用户显式选择，内核不猜。 */
  audioOperation?: "speech_transcribe" | "speech_synthesize";
  audioModel?: string;
  /** 待转写音频的 artifact 引用。 */
  audioReference?: string;
  /** 供应商音色 id；留空则由供应商给出自己的报错。 */
  audioVoice?: string;
  audioFormat?: string;
  audioUrl?: string;
  /** 仅用于界面展示的原文件名。 */
  audioReferenceName?: string;
  leftImageUrl?: string;
  rightImageUrl?: string;
  editMode?: "crop" | "remove-bg" | "upscale" | "repaint";
  inputVideoUrls?: string[];
  result?: unknown;
  executionTime?: number;
  [key: string]: unknown;
}

export interface InputVariableDefinition {
  name: string;
  type: string;
  required?: boolean;
  description?: string;
  default?: unknown;
  options?: string[];
  minLength?: number;
  maxLength?: number;
  minimum?: number;
  maximum?: number;
}

export interface ProfessionalNodeData {
  label: string;
  description?: string;
  kind: ProfessionalNodeKind;
  status: NodeStatus;
  errorMessage?: string;
  inputVariables?: InputVariableDefinition[];
  outputMapping?: Record<string, string>;
  conditions?: Array<{
    id: string;
    logic: "and" | "or";
    items: Array<{ variable: string; operator: string; value: string }>;
  }>;
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

export interface CreativePickerOption {
  key: string;
  label: string;
  kind: CreativeNodeKind;
  icon?: string;
  description?: string;
}

export interface CreativePickerCategory {
  key: string;
  label: string;
  icon: string;
  description: string;
  options?: CreativePickerOption[];
  optionGroups?: Array<{ key: string; label: string; options: CreativePickerOption[] }>;
  action?: "create" | "upload";
  directKind?: CreativeNodeKind;
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

/**
 * 画布双击后的产品入口。它和 CREATIVE_NODE_CATEGORIES 分开，避免把
 * “用户意图菜单”误当成执行器节点注册表；后者仍用于 React Flow 的节点类型。
 */
export const CREATIVE_NODE_PICKER_CATEGORIES: CreativePickerCategory[] = [
  {
    key: "text",
    label: "文本",
    icon: "pen",
    description: "写作、解析与提示词处理",
    options: [
      { key: "write", label: "自己编写内容", kind: "text-gen", icon: "pen" },
      { key: "parse-document", label: "上传文档解析文本", kind: "text-gen", icon: "file-lines" },
      { key: "text-to-image", label: "文生图", kind: "text-gen", icon: "image" },
      { key: "text-to-video", label: "文字生视频", kind: "video-gen", icon: "video" },
      { key: "image-to-prompt", label: "图片反推提示词", kind: "text-gen", icon: "wand-magic-sparkles" },
    ],
  },
  {
    key: "image",
    label: "图片",
    icon: "image",
    description: "生成、编辑与转换图片",
    options: [
      { key: "upload-image", label: "上传图片", kind: "image-gen", icon: "upload" },
      { key: "image-to-image", label: "图生图", kind: "image-gen", icon: "wand-magic-sparkles" },
      { key: "image-to-video", label: "图生视频", kind: "video-gen", icon: "video" },
      { key: "replace-background", label: "图片换背景", kind: "image-edit", icon: "crop" },
      { key: "first-frame-to-video", label: "首帧图生视频", kind: "video-gen", icon: "film" },
    ],
  },
  {
    key: "video",
    label: "视频",
    icon: "video",
    description: "参考素材与视频生成",
    options: [
      { key: "all-reference", label: "全部参考", kind: "video-gen", icon: "layer-group" },
      { key: "image-to-video", label: "图生视频", kind: "video-gen", icon: "image" },
      { key: "first-last-frame-to-video", label: "首尾帧生视频", kind: "video-gen", icon: "film" },
    ],
  },
  {
    key: "camera",
    label: "摄像机",
    icon: "bullseye",
    description: "设置镜头与拍摄参数",
    options: [
      { key: "camera", label: "摄像机", kind: "camera", icon: "bullseye" },
      { key: "lens", label: "镜头", kind: "lens", icon: "magnifying-glass" },
      { key: "focal-length", label: "焦距", kind: "focal-length", icon: "left-right" },
      { key: "aperture", label: "光圈", kind: "aperture", icon: "circle-notch" },
    ],
  },
  {
    key: "director",
    label: "微表情导演",
    icon: "circle-user",
    description: "设计角色表情与表演",
    directKind: "director",
  },
  {
    key: "compare",
    label: "对比",
    icon: "left-right",
    description: "对比两份图片结果",
    directKind: "image-compare",
  },
  {
    key: "template",
    label: "创意库模板",
    icon: "lightbulb",
    description: "从创意库选择可复用模板",
    options: [{ key: "select-template", label: "选择创意模板", kind: "creative-template", icon: "folder-tree" }],
  },
  {
    key: "character",
    label: "角色工作室",
    icon: "circle-user",
    description: "组合角色的面部、身体与风格",
    optionGroups: [
      {
        key: "face",
        label: "面部",
        options: [
          { key: "character-type", label: "角色类型", kind: "character-face" },
          { key: "gender", label: "性别", kind: "character-face" },
          { key: "ethnicity", label: "族裔与血统", kind: "character-face" },
          { key: "hair-head", label: "发型及头部特征", kind: "character-face" },
          { key: "eye-color", label: "眼睛颜色", kind: "character-face" },
          { key: "eye-type", label: "眼睛类型", kind: "character-face" },
          { key: "eye-features", label: "眼部特征", kind: "character-face" },
          { key: "mouth-teeth", label: "嘴部与牙齿", kind: "character-face" },
          { key: "ears", label: "耳朵", kind: "character-face" },
          { key: "horns", label: "犄角", kind: "character-face" },
          { key: "skin-features", label: "皮肤特征", kind: "character-face" },
        ],
      },
      {
        key: "body",
        label: "身体",
        options: [
          { key: "skin-material", label: "皮肤材质", kind: "character-body" },
          { key: "skin-texture", label: "皮肤纹理", kind: "character-body" },
          { key: "body-shape", label: "身材", kind: "character-body" },
          { key: "right-arm", label: "右臂", kind: "character-body" },
          { key: "left-arm", label: "左臂", kind: "character-body" },
          { key: "left-leg", label: "左腿", kind: "character-body" },
          { key: "right-leg", label: "右腿", kind: "character-body" },
        ],
      },
      {
        key: "style",
        label: "风格",
        options: [
          { key: "accessories-markings", label: "配饰与标记", kind: "character-style" },
          { key: "render-style", label: "渲染风格", kind: "character-style" },
        ],
      },
    ],
  },
  {
    key: "audio",
    label: "音频",
    icon: "microphone",
    description: "导入或处理音乐与声音",
    directKind: "audio",
  },
  {
    key: "storyboard",
    label: "分镜格子",
    icon: "table",
    description: "组织镜头、节奏与分镜",
    directKind: "storyboard-grid",
  },
  {
    key: "upload",
    label: "上传",
    icon: "upload",
    description: "上传本地图片、视频或音乐",
    action: "upload",
  },
];

export const CREATIVE_NODE_KINDS: CreativeNodeKind[] = [
  ...CREATIVE_NODE_CATEGORIES.flatMap((category) => category.items.map((item) => item.kind)),
  "category-picker",
  "image-input",
  "camera",
  "lens",
  "focal-length",
  "aperture",
  "director",
  "creative-template",
  "character-face",
  "character-body",
  "character-style",
  "audio",
  "storyboard-grid",
];

export interface ProfessionalNodePickerTab {
  label: string;
  groups: Array<{
    label?: string;
    items: Array<{ kind: ProfessionalNodeKind; label: string; icon: string }>;
  }>;
}

export const PROFESSIONAL_NODE_PICKER_TABS: ProfessionalNodePickerTab[] = [
  {
    label: "节点",
    groups: [
      {
        label: "AI",
        items: [
          { kind: "llm", label: "LLM", icon: "wand-magic-sparkles" },
          { kind: "agent", label: "Agent", icon: "robot" },
          { kind: "knowledge-retrieval", label: "知识检索", icon: "book" },
          { kind: "answer", label: "输出", icon: "message-square" },
          { kind: "question-classifier", label: "问题分类器", icon: "tags" },
          { kind: "parameter-extractor", label: "参数提取器", icon: "table-columns" },
        ],
      },
      {
        label: "逻辑",
        items: [
          { kind: "if-else", label: "条件分支", icon: "code-branch" },
          { kind: "human-input", label: "人工介入", icon: "user-check" },
          { kind: "iteration", label: "迭代", icon: "rotate" },
          { kind: "loop", label: "循环", icon: "repeat" },
        ],
      },
      {
        label: "转换",
        items: [
          { kind: "code", label: "代码执行", icon: "code" },
          { kind: "template-transform", label: "模板转换", icon: "file-code" },
          { kind: "variable-aggregator", label: "变量聚合器", icon: "layer-group" },
          { kind: "document-extractor", label: "文档提取器", icon: "file-text" },
          { kind: "variable-assigner", label: "变量赋值", icon: "equal" },
          { kind: "http-request", label: "HTTP 请求", icon: "globe" },
          { kind: "list-operator", label: "列表操作", icon: "list" },
        ],
      },
    ],
  },
  {
    label: "工具",
    groups: [],
  },
  {
    label: "开始",
    groups: [
      {
        items: [
          { kind: "start", label: "用户输入", icon: "play" },
          { kind: "trigger-schedule", label: "定时触发器", icon: "clock" },
          { kind: "trigger-webhook", label: "Webhook 触发器", icon: "webhook" },
        ],
      },
    ],
  },
  {
    label: "Snippets",
    groups: [],
  },
];

export const PROFESSIONAL_NODE_CATEGORIES: ProfessionalNodeCategory[] = [
  {
    group: "流程控制",
    color: "#3B82F6",
    items: [
      { kind: "start", label: "开始", icon: "play" },
      { kind: "end", label: "结束", icon: "stop" },
      { kind: "answer", label: "直接回答", icon: "message-square" },
      { kind: "if-else", label: "条件分支", icon: "code-branch" },
      { kind: "iteration", label: "迭代", icon: "rotate" },
      { kind: "iteration-start", label: "迭代开始", icon: "log-in" },
      { kind: "loop", label: "循环", icon: "repeat" },
      { kind: "loop-start", label: "循环开始", icon: "log-in" },
      { kind: "loop-end", label: "循环结束", icon: "log-out" },
    ],
  },
  {
    group: "AI",
    color: "#8B5CF6",
    items: [
      { kind: "llm", label: "LLM", icon: "wand-magic-sparkles" },
      { kind: "agent", label: "Agent", icon: "robot" },
      { kind: "agent-v2", label: "Agent V2", icon: "bot" },
      { kind: "question-classifier", label: "问题分类", icon: "tags" },
      { kind: "parameter-extractor", label: "参数提取", icon: "table-columns" },
    ],
  },
  {
    group: "知识",
    color: "#06B6D4",
    items: [
      { kind: "knowledge-retrieval", label: "知识检索", icon: "book" },
      { kind: "knowledge-index", label: "知识库索引", icon: "database" },
      { kind: "document-extractor", label: "文档提取", icon: "file-text" },
    ],
  },
  {
    group: "数据",
    color: "#10B981",
    items: [
      { kind: "datasource", label: "数据源", icon: "cylinder" },
      { kind: "code", label: "代码执行", icon: "code" },
      { kind: "http-request", label: "HTTP 请求", icon: "globe" },
      { kind: "template-transform", label: "模板转换", icon: "file-code" },
      { kind: "variable-aggregator", label: "变量聚合", icon: "layer-group" },
      { kind: "variable-assigner", label: "变量赋值", icon: "equal" },
      { kind: "list-operator", label: "列表操作", icon: "list" },
    ],
  },
  {
    group: "触发器",
    color: "#EC4899",
    items: [
      { kind: "trigger-schedule", label: "定时触发", icon: "clock" },
      { kind: "trigger-webhook", label: "Webhook", icon: "webhook" },
      { kind: "trigger-plugin", label: "插件触发", icon: "puzzle" },
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
