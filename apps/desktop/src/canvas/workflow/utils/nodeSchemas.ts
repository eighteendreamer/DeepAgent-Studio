import type { NodeConfigSchema, ProfessionalNodeKind } from "../types";

function text(title: string, defaultValue = ""): NodeConfigSchema {
  return { type: "string", title, default: defaultValue };
}

function choice(
  title: string,
  options: Record<string, string>,
  defaultValue: string,
): NodeConfigSchema {
  return {
    type: "string",
    title,
    enum: Object.keys(options),
    "x-enum-labels": Object.values(options),
    default: defaultValue,
  };
}

function variable(title: string, types?: string[]): NodeConfigSchema {
  return {
    ...text(title),
    "x-widget": "variable",
    ...(types ? { "x-variable-types": types } : {}),
  };
}

function list(title: string, items: NodeConfigSchema, defaultValue: unknown[] = []): NodeConfigSchema {
  return { type: "array", title, items, default: defaultValue };
}

function dictionary(title: string): NodeConfigSchema {
  return { type: "object", title, default: {}, additionalProperties: true, "x-widget": "json" };
}

function nodeSchema(
  title: string,
  properties: Record<string, NodeConfigSchema>,
  description?: string,
): NodeConfigSchema {
  return {
    type: "object",
    title,
    ...(description ? { description } : {}),
    properties: {
      description: { ...text("节点描述"), "x-widget": "textarea" },
      ...properties,
    },
  };
}

const VARIABLE_TYPES = {
  string: "文本",
  number: "数字",
  boolean: "布尔",
  object: "对象",
  array: "数组",
  file: "文件",
};

function variableType(types: Array<keyof typeof VARIABLE_TYPES>): NodeConfigSchema {
  return choice("类型", Object.fromEntries(types.map((type) => [type, VARIABLE_TYPES[type]])), "string");
}

const VARIABLE_DECLARATION_PROPERTIES: Record<string, NodeConfigSchema> = {
  name: { ...text("变量名"), minLength: 1, pattern: "^(?!__proto__$|constructor$|prototype$)[A-Za-z_][A-Za-z0-9_]*$", description: "使用字母、数字和下划线，不能以数字开头。" },
  type: variableType(["string", "number", "boolean", "array", "object"]),
  description: { ...text("变量描述"), "x-widget": "textarea" },
  required: { type: "boolean", title: "必填", default: true },
};

const VARIABLE_DECLARATION: NodeConfigSchema = {
  type: "object",
  title: "变量声明",
  properties: VARIABLE_DECLARATION_PROPERTIES,
  required: ["name", "type"],
};

const OUTPUT_DECLARATION: NodeConfigSchema = {
  type: "object",
  title: "输出声明",
  properties: {
    name: VARIABLE_DECLARATION_PROPERTIES.name,
    type: variableType(["string", "number", "object", "array"]),
    description: VARIABLE_DECLARATION_PROPERTIES.description,
  },
  required: ["name", "type"],
};

const VARIABLE_BINDING: NodeConfigSchema = {
  type: "object",
  title: "变量绑定",
  properties: {
    name: VARIABLE_DECLARATION_PROPERTIES.name,
    value: variable("变量值或引用"),
  },
  required: ["name", "value"],
};

const MODEL: NodeConfigSchema = { ...text("模型"), "x-widget": "model" };

const CHAT_MODEL: NodeConfigSchema = { ...MODEL, default: "deepseek-chat" };

const TOOL: NodeConfigSchema = { ...text("工具"), "x-widget": "tool" };

const RETRIEVAL_MODE = choice("检索模式", {
  semantic: "语义检索",
  keyword: "关键词检索",
  hybrid: "混合检索",
}, "semantic");

const CONDITION_ITEM: NodeConfigSchema = {
  type: "object",
  title: "条件",
  properties: {
    variable: variable("比较变量"),
    operator: choice("运算符", {
      is: "等于",
      "is-not": "不等于",
      contains: "包含",
      "not-contains": "不包含",
      "starts-with": "开头是",
      "ends-with": "结尾是",
      empty: "为空",
      "not-empty": "不为空",
      gt: "大于",
      gte: "大于等于",
      lt: "小于",
      lte: "小于等于",
    }, "is"),
    value: text("比较值"),
  },
  required: ["variable", "operator", "value"],
};

const CONDITION_GROUP: NodeConfigSchema = {
  type: "object",
  title: "条件分支",
  properties: {
    id: {
      type: "string",
      title: "分支标识",
      format: "uuid",
      readOnly: true,
      description: "新增分支使用稳定 UUID，保留已有分支标识。",
    },
    logic: choice("条件关系", { and: "全部满足", or: "任一满足" }, "and"),
    items: list("条件列表", CONDITION_ITEM, [{ variable: "", operator: "is", value: "" }]),
  },
  required: ["id", "logic", "items"],
};

export const NODE_CONFIG_SCHEMAS: Record<ProfessionalNodeKind, NodeConfigSchema> = {
  start: nodeSchema("开始", {
    inputVariables: list("输入变量", {
      ...VARIABLE_DECLARATION,
      properties: {
        ...VARIABLE_DECLARATION_PROPERTIES,
        type: variableType(["string", "number", "boolean", "object", "array", "file"]),
        default: {
          title: "默认值",
          description: "任意 JSON 值，包括文本、数字、布尔、对象、数组和 null。",
          "x-widget": "json",
        },
        options: {
          type: "array",
          title: "可选值",
          items: { type: "string", title: "选项" },
          "x-visible-when": { field: "type", value: "string" },
        },
        minLength: {
          type: "integer",
          title: "最小长度",
          minimum: 0,
          "x-visible-when": { field: "type", value: "string" },
        },
        maxLength: {
          type: "integer",
          title: "最大长度",
          minimum: 0,
          "x-visible-when": { field: "type", value: "string" },
        },
        minimum: {
          type: "number",
          title: "最小值",
          "x-visible-when": { field: "type", value: "number" },
        },
        maximum: {
          type: "number",
          title: "最大值",
          "x-visible-when": { field: "type", value: "number" },
        },
      },
    }),
  }),
  end: nodeSchema("结束", {
    outputVariables: list("输出变量", {
      type: "object",
      title: "输出变量",
      properties: {
        name: VARIABLE_DECLARATION_PROPERTIES.name,
        type: variableType(["string", "number", "object", "array"]),
        value: variable("输出值或引用"),
      },
      required: ["name", "type", "value"],
    }),
  }),
  "if-else": nodeSchema("条件分支", {
    conditions: list("分支列表", CONDITION_GROUP, [
      { id: "if", logic: "and", items: [{ variable: "", operator: "is", value: "" }] },
    ]),
  }),
  iteration: nodeSchema("迭代", {
    inputVariable: variable("输入数组变量", ["array"]),
    outputVariable: text("输出变量名"),
    parallel: { type: "boolean", title: "并行执行", default: false },
    maxConcurrency: {
      type: "integer",
      title: "最大并发数",
      default: 1,
      minimum: 1,
      maximum: 50,
      "x-visible-when": { field: "parallel", value: true },
    },
    errorHandling: choice("错误处理", {
      terminate: "终止",
      continue: "跳过并继续",
      remove: "移除异常输出",
    }, "terminate"),
    flatten: { type: "boolean", title: "扁平化输出", default: false },
  }),
  llm: nodeSchema("大语言模型", {
    llmModel: CHAT_MODEL,
    llmSystemPrompt: { ...text("系统提示词"), "x-widget": "textarea" },
    llmPrompt: { ...text("用户提示词"), "x-widget": "textarea" },
    llmTemperature: { type: "number", title: "温度", default: 0.7, minimum: 0, maximum: 2 },
    llmMaxTokens: { type: "integer", title: "最大输出令牌数", default: 4096, minimum: 1, maximum: 128000 },
  }),
  agent: nodeSchema("智能体", {
    agentStrategy: choice("策略", { "function-call": "函数调用", react: "推理与行动（ReAct）" }, "function-call"),
    agentTools: list("工具", TOOL),
  }),
  "question-classifier": nodeSchema("问题分类", {
    classifierModel: CHAT_MODEL,
    classifierInput: variable("输入查询", ["string"]),
    classifierClasses: list("分类", {
      type: "object",
      title: "分类",
      properties: {
        name: text("分类名称"),
        description: { ...text("分类描述"), "x-widget": "textarea" },
      },
      required: ["name"],
    }, [
      { name: "分类1", description: "" },
      { name: "分类2", description: "" },
    ]),
    classifierInstruction: { ...text("分类指令"), "x-widget": "textarea" },
  }),
  "parameter-extractor": nodeSchema("参数提取", {
    extractorModel: CHAT_MODEL,
    extractorInput: variable("输入变量"),
    extractorParams: list("提取参数", VARIABLE_DECLARATION, [
      { name: "", type: "string", description: "", required: true },
    ]),
    extractorInstruction: { ...text("提取指令"), "x-widget": "textarea" },
    reasoningMode: { type: "boolean", title: "推理模式", default: false },
  }),
  "knowledge-retrieval": nodeSchema("知识检索", {
    knowledgeBaseId: text("知识库标识"),
    knowledgeTopK: { type: "integer", title: "召回数量", default: 3, minimum: 1, maximum: 20 },
    queryVariable: variable("查询变量", ["string"]),
    retrievalMode: RETRIEVAL_MODE,
    scoreThreshold: { type: "number", title: "分数阈值", default: 0.5 },
    rerankModel: text("重排序模型"),
  }),
  code: nodeSchema("代码执行", {
    codeLanguage: choice("语言", { javascript: "JavaScript", python: "Python" }, "javascript"),
    codeScript: { ...text("代码"), "x-widget": "code" },
    codeInputVariables: list("输入变量", VARIABLE_BINDING),
    codeOutputVariables: list("输出声明", {
      ...OUTPUT_DECLARATION,
      properties: {
        ...OUTPUT_DECLARATION.properties,
        type: VARIABLE_DECLARATION_PROPERTIES.type,
      },
    }),
  }),
  "http-request": nodeSchema("HTTP 请求", {
    httpMethod: choice("请求方法", { GET: "GET", POST: "POST", PUT: "PUT", DELETE: "DELETE" }, "GET"),
    httpUrl: text("请求地址"),
    httpHeaders: { ...dictionary("请求头"), additionalProperties: { type: "string", title: "请求头值" } },
    httpBody: { ...text("请求体"), "x-widget": "textarea" },
    httpTimeout: { type: "integer", title: "超时时间（秒）", default: 30, minimum: 1 },
    httpRetryCount: { type: "integer", title: "重试次数", default: 0, minimum: 0 },
  }),
  "template-transform": nodeSchema("模板转换", {
    templateInputVariables: list("输入变量", VARIABLE_BINDING),
    templateScript: { ...text("Jinja 模板"), "x-widget": "code" },
  }),
  "variable-aggregator": nodeSchema("变量聚合", {
    aggregatorOutputType: variableType(["string", "number", "array", "object"]),
    aggregatorVariables: list("聚合变量", variable("变量引用")),
  }),
  tool: nodeSchema("工具调用", {
    toolId: { ...TOOL, title: "工具标识" },
    toolParams: {
      ...dictionary("工具参数"),
      description: "参数结构由所选工具声明，待工具集成后加载。",
    },
  }, "预留节点；配置元数据不代表工具调用已接入执行。"),
  "human-input": nodeSchema("人工审批", {
    humanInputFields: list("表单字段", {
      type: "object",
      title: "表单字段",
      properties: {
        name: { ...VARIABLE_DECLARATION_PROPERTIES.name, title: "字段名" },
        type: variableType(["string", "number", "boolean", "file"]),
        label: text("显示标签"),
        required: VARIABLE_DECLARATION_PROPERTIES.required,
      },
      required: ["name", "type", "label", "required"],
    }, [{ name: "", type: "string", label: "", required: true }]),
    humanInputPrompt: { ...text("提示内容"), "x-widget": "textarea" },
    timeoutSeconds: {
      type: "integer",
      title: "等待超时（秒）",
      description: "可选；填写时必须为正整数。",
      minimum: 1,
    },
  }),
  answer: nodeSchema("直接回答", {
    answerTemplate: { ...text("回答模板"), "x-widget": "textarea" },
    answerVariables: list("模板变量", VARIABLE_BINDING),
  }),
  "iteration-start": nodeSchema("迭代开始", {}, "迭代开始节点自动接收父迭代的当前项。"),
  loop: nodeSchema("循环", {
    loopVariable: variable("循环变量"),
    loopCondition: text("终止条件"),
    loopMaxIterations: { type: "integer", title: "最大循环次数", default: 100, minimum: 1, maximum: 10000 },
    errorHandling: choice("错误处理", { terminate: "终止", continue: "跳过并继续" }, "terminate"),
  }),
  "loop-start": nodeSchema("循环开始", {}, "循环开始节点自动继承父循环上下文。"),
  "loop-end": nodeSchema("循环结束", {}, "循环结束节点标记循环体终止位置。"),
  "agent-v2": nodeSchema("智能体 V2", {
    agentV2Model: CHAT_MODEL,
    agentV2Task: { ...text("任务描述"), "x-widget": "textarea" },
    agentV2Tools: list("工具", TOOL),
    agentV2Outputs: list("声明输出", OUTPUT_DECLARATION),
    agentV2Memory: { type: "boolean", title: "记忆", default: false },
  }),
  "document-extractor": nodeSchema("文档提取", {
    docExtractorFileVariable: variable("文件变量", ["file", "array"]),
    docExtractorIsArray: { type: "boolean", title: "输入为文件数组", default: false },
  }),
  "variable-assigner": nodeSchema("变量赋值", {
    assignerTarget: variable("目标变量"),
    assignerMode: choice("写入模式", {
      set: "赋值",
      increment: "自增",
      decrement: "自减",
      multiply: "乘以",
      divide: "除以",
      clear: "清空",
      "remove-first": "移除首项",
      "remove-last": "移除末项",
    }, "set"),
    assignerValue: variable("写入值或引用"),
  }),
  "list-operator": nodeSchema("列表操作", {
    listOperatorInput: variable("输入列表", ["array"]),
    listOperatorAction: choice("操作", { filter: "过滤", map: "映射提取", sort: "排序", limit: "截取" }, "filter"),
    listOperatorCondition: text("过滤或映射条件"),
    listOperatorExtractField: {
      ...text("提取字段"),
      "x-visible-when": { field: "listOperatorAction", value: "map" },
    },
    listOperatorOrderBy: {
      ...text("排序依据"),
      "x-visible-when": { field: "listOperatorAction", value: "sort" },
    },
    listOperatorLimit: { type: "integer", title: "数量限制", default: 0, minimum: 0 },
  }),
  "trigger-schedule": nodeSchema("定时触发", {
    scheduleFrequency: choice("频率", {
      minutely: "每分钟",
      hourly: "每小时",
      daily: "每天",
      weekly: "每周",
      monthly: "每月",
      custom: "自定义 Cron",
    }, "daily"),
    scheduleTime: {
      ...text("触发时间", "09:00"),
      description: "使用时:分格式；自定义 Cron 模式使用 Cron 表达式。",
    },
    scheduleTimezone: text("时区", "Asia/Shanghai"),
    scheduleCron: {
      ...text("Cron 表达式"),
      "x-visible-when": { field: "scheduleFrequency", value: "custom" },
    },
    scheduleDayOfWeek: {
      ...list("星期", { type: "string", title: "星期值", description: "周一为字符串 1。" }),
      "x-visible-when": { field: "scheduleFrequency", value: "weekly" },
    },
  }, "预留触发器；仅保存定时配置，调度执行待后端接入。"),
  "trigger-webhook": nodeSchema("Webhook 触发", {
    webhookMethod: choice("请求方法", { GET: "GET", POST: "POST", PUT: "PUT" }, "POST"),
    webhookPath: text("路径"),
    webhookHeaders: dictionary("请求头"),
    webhookAuthType: choice("鉴权方式", {
      none: "无",
      bearer: "Bearer 令牌",
      hmac: "HMAC 签名",
      basic: "Basic 认证",
    }, "none"),
    webhookAuthSecret: {
      ...text("鉴权密钥"),
      "x-widget": "password",
      description: "仅在选择鉴权方式后使用。",
    },
    webhookAsync: { type: "boolean", title: "异步模式（立即返回 202）", default: false },
  }, "预留触发器；仅保存 Webhook 配置，端点监听与鉴权待后端接入。"),
  "trigger-plugin": nodeSchema("插件触发", {
    pluginProvider: text("插件提供方"),
    pluginEvent: text("插件事件"),
    pluginParams: {
      ...dictionary("插件参数"),
      description: "参数结构由插件声明，待后端接入后动态加载。",
    },
    pluginCredentialId: text("凭据标识"),
  }, "预留触发器；插件事件订阅与执行待后端接入。"),
  datasource: nodeSchema("数据源", {
    datasourceType: choice("数据源类型", {
      api: "API",
      database: "数据库",
      storage: "对象存储",
      plugin: "插件",
    }, "api"),
    datasourcePlugin: text("插件或连接器"),
    datasourceParams: {
      ...dictionary("数据源参数"),
      description: "参数结构由数据源插件声明，待后端接入后动态加载。",
    },
    datasourceCredentialId: text("凭据标识"),
    datasourceExtensions: list("文件扩展名过滤", { type: "string", title: "文件扩展名" }),
  }, "预留数据源；仅保存连接配置，数据读取待后端接入。"),
  "knowledge-index": nodeSchema("知识库索引", {
    indexSourceVariable: variable("输入源变量"),
    indexChunkSize: { type: "integer", title: "分块大小", default: 500, minimum: 100, maximum: 4000 },
    indexChunkOverlap: { type: "integer", title: "重叠大小", default: 50, minimum: 0, maximum: 1000 },
    indexEmbeddingModel: text("嵌入模型"),
    indexRetrievalMode: RETRIEVAL_MODE,
    indexKeywords: { type: "integer", title: "关键词数量", default: 0, minimum: 0, maximum: 20 },
  }, "预留节点；索引构建待后端接入。"),
};
