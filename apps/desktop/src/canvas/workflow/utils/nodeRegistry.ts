import type { NodeDefinition, ProfessionalNodeKind } from "./types";

const registry = new Map<ProfessionalNodeKind, NodeDefinition>();

function register(def: NodeDefinition) {
  registry.set(def.kind, def);
}

register({
  kind: "start",
  label: "开始",
  category: "流程控制",
  icon: "play",
  availability: "enabled",
  executor: "passthrough",
  outputs: [
    { name: "input_variables", type: "object", description: "用户输入的变量集合" },
  ],
  defaultData: () => ({ label: "开始", inputVariables: [] }),
});

register({
  kind: "end",
  label: "结束",
  category: "流程控制",
  icon: "stop",
  availability: "enabled",
  executor: "passthrough",
  outputs: [],
  defaultData: () => ({ label: "结束", outputVariables: [] }),
});

register({
  kind: "if-else",
  label: "条件分支",
  category: "流程控制",
  icon: "code-branch",
  availability: "enabled",
  executor: "control-flow",
  outputs: [],
  defaultData: () => ({
    label: "条件分支",
    conditions: [
      {
        id: "if",
        logic: "and",
        items: [{ variable: "", operator: "is", value: "" }],
      },
    ],
  }),
});

register({
  kind: "iteration",
  label: "迭代",
  category: "流程控制",
  icon: "rotate",
  availability: "enabled",
  executor: "control-flow",
  outputs: [{ name: "output", type: "array", description: "每次迭代的输出数组" }],
  defaultData: () => ({
    label: "迭代",
    inputVariable: "",
    outputVariable: "",
    parallel: false,
    maxConcurrency: 1,
    errorHandling: "terminate",
    flatten: false,
  }),
});

register({
  kind: "llm",
  label: "LLM",
  category: "AI",
  icon: "wand-magic-sparkles",
  availability: "enabled",
  executor: "model",
  outputs: [
    { name: "text", type: "string", description: "模型输出文本" },
    { name: "usage", type: "object", description: "Token 用量" },
  ],
  defaultData: () => ({
    label: "LLM",
    llmModel: "deepseek-chat",
    llmSystemPrompt: "",
    llmPrompt: "",
    llmTemperature: 0.7,
    llmMaxTokens: 4096,
  }),
});

register({
  kind: "agent",
  label: "Agent",
  category: "AI",
  icon: "robot",
  availability: "enabled",
  executor: "model",
  outputs: [
    { name: "text", type: "string", description: "Agent 输出文本" },
    { name: "usage", type: "object", description: "Token 用量" },
  ],
  defaultData: () => ({
    label: "Agent",
    agentStrategy: "function-call",
    agentTools: [],
  }),
});

register({
  kind: "question-classifier",
  label: "问题分类",
  category: "AI",
  icon: "tags",
  availability: "enabled",
  executor: "model",
  outputs: [
    { name: "class_name", type: "string", description: "分类名称" },
    { name: "class_label", type: "string", description: "分类标签" },
    { name: "usage", type: "object", description: "Token 用量" },
  ],
  defaultData: () => ({
    label: "问题分类",
    classifierModel: "deepseek-chat",
    classifierInput: "",
    classifierClasses: [
      { name: "分类1", description: "" },
      { name: "分类2", description: "" },
    ],
    classifierInstruction: "",
  }),
});

register({
  kind: "parameter-extractor",
  label: "参数提取",
  category: "AI",
  icon: "table-columns",
  availability: "enabled",
  executor: "model",
  outputs: [
    { name: "__is_success", type: "number", description: "提取是否成功" },
    { name: "__reason", type: "string", description: "失败原因" },
    { name: "__usage", type: "object", description: "Token 用量" },
  ],
  defaultData: () => ({
    label: "参数提取",
    extractorModel: "deepseek-chat",
    extractorInput: "",
    extractorParams: [{ name: "", type: "string", description: "", required: true }],
    extractorInstruction: "",
    reasoningMode: false,
  }),
});

register({
  kind: "knowledge-retrieval",
  label: "知识检索",
  category: "知识",
  icon: "book",
  availability: "enabled",
  executor: "retrieval",
  outputs: [
    { name: "documents", type: "array", description: "检索到的文档列表" },
    { name: "content", type: "string", description: "合并后的文本内容" },
  ],
  defaultData: () => ({
    label: "知识检索",
    knowledgeBaseId: "",
    knowledgeTopK: 3,
    queryVariable: "",
    retrievalMode: "semantic",
    scoreThreshold: 0.5,
    rerankModel: "",
  }),
});

register({
  kind: "code",
  label: "代码执行",
  category: "数据",
  icon: "code",
  availability: "enabled",
  executor: "code",
  outputs: [],
  defaultData: () => ({
    label: "代码执行",
    codeLanguage: "javascript",
    codeScript: "",
    codeInputVariables: [],
    codeOutputVariables: [],
  }),
});

register({
  kind: "http-request",
  label: "HTTP 请求",
  category: "数据",
  icon: "globe",
  availability: "enabled",
  executor: "http",
  outputs: [
    { name: "body", type: "string", description: "响应体" },
    { name: "status_code", type: "number", description: "HTTP 状态码" },
    { name: "headers", type: "object", description: "响应头" },
  ],
  defaultData: () => ({
    label: "HTTP 请求",
    httpMethod: "GET",
    httpUrl: "",
    httpHeaders: {},
    httpBody: "",
    httpTimeout: 30,
    httpRetryCount: 0,
  }),
});

register({
  kind: "template-transform",
  label: "模板转换",
  category: "数据",
  icon: "file-code",
  availability: "enabled",
  executor: "transform",
  outputs: [{ name: "output", type: "string", description: "渲染后的文本" }],
  defaultData: () => ({
    label: "模板转换",
    templateInputVariables: [],
    templateScript: "",
  }),
});

register({
  kind: "variable-aggregator",
  label: "变量聚合",
  category: "数据",
  icon: "layer-group",
  availability: "enabled",
  executor: "transform",
  outputs: [{ name: "output", type: "string", description: "聚合后的结果" }],
  defaultData: () => ({
    label: "变量聚合",
    aggregatorOutputType: "string",
    aggregatorVariables: [],
  }),
});

register({
  kind: "tool",
  label: "工具调用",
  category: "工具",
  icon: "wrench",
  availability: "reserved",
  executor: "http",
  outputs: [
    { name: "text", type: "string", description: "工具输出文本" },
    { name: "json", type: "object", description: "工具输出 JSON" },
  ],
  defaultData: () => ({ label: "工具调用", toolId: "", toolParams: {} }),
});

register({
  kind: "human-input",
  label: "人工审批",
  category: "工具",
  icon: "user-check",
  availability: "enabled",
  executor: "human",
  outputs: [
    { name: "action", type: "string", description: "用户操作结果" },
  ],
  defaultData: () => ({
    label: "人工审批",
    humanInputFields: [{ name: "", type: "string", label: "", required: true }],
    humanInputPrompt: "",
  }),
});

export function getNodeDefinition(kind: ProfessionalNodeKind): NodeDefinition {
  const def = registry.get(kind);
  if (!def) throw new Error(`Unknown professional node kind: ${kind}`);
  return def;
}

export function createDefaultNodeData(kind: ProfessionalNodeKind): Record<string, unknown> {
  return getNodeDefinition(kind).defaultData();
}

export function getNodeOutputs(kind: ProfessionalNodeKind) {
  return getNodeDefinition(kind).outputs;
}

export function getAllNodeDefinitions(): NodeDefinition[] {
  return Array.from(registry.values());
}
