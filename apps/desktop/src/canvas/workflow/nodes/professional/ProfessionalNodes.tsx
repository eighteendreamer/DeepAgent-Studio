import type { ProfessionalNodeData } from "../../types";
import { getNodeOutputs } from "../../utils/nodeRegistry";

interface Props {
  data: ProfessionalNodeData;
}

export function StartContent({ data }: Props) {
  const vars = data.inputVariables ?? [];
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
        {vars.length > 0 ? `${vars.length} 个输入变量` : "定义输入变量"}
      </span>
      {vars.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {vars.slice(0, 4).map((v) => (
            <span key={v.name} className="rounded px-1 py-0.5 text-[9px]" style={{ background: "rgba(59,130,246,0.15)", color: "rgba(59,130,246,0.8)" }}>
              {v.name}
            </span>
          ))}
          {vars.length > 4 && (
            <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.3)" }}>+{vars.length - 4}</span>
          )}
        </div>
      )}
    </div>
  );
}

export function EndContent({ data }: Props) {
  const count = getNodeOutputs("end", data).length;
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {count > 0 ? `${count} 个输出` : "定义输出映射"}
    </span>
  );
}

export function IfElseContent({ data }: Props) {
  const conds = data.conditions ?? [];
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {conds.length > 0 ? `${conds.length} 个条件` : "设置分支条件"}
    </span>
  );
}

export function IterationContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      遍历数组元素
    </span>
  );
}

export function LLMContent({ data }: Props) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1.5">
        <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(139,92,246,0.2)", color: "rgba(139,92,246,0.9)" }}>
          {data.llmModel ?? "deepseek-chat"}
        </span>
        {data.llmTemperature != null && (
          <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
            T={data.llmTemperature}
          </span>
        )}
      </div>
      {data.llmPrompt && (
        <div className="text-[10px] truncate" style={{ color: "rgba(248,248,248,0.5)" }}>
          {data.llmPrompt}
        </div>
      )}
    </div>
  );
}

export function AgentContent({ data }: Props) {
  return (
    <div className="flex items-center gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(139,92,246,0.2)", color: "rgba(139,92,246,0.9)" }}>
        {data.agentStrategy === "react" ? "ReAct" : "Function Call"}
      </span>
      {(data.agentTools?.length ?? 0) > 0 && (
        <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
          {data.agentTools!.length} 工具
        </span>
      )}
    </div>
  );
}

export function QuestionClassifierContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      LLM 分类路由
    </span>
  );
}

export function ParameterExtractorContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      结构化参数提取
    </span>
  );
}

export function KnowledgeRetrievalContent({ data }: Props) {
  return (
    <div className="flex items-center gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(6,182,212,0.2)", color: "rgba(6,182,212,0.9)" }}>
        RAG
      </span>
      {data.knowledgeTopK && (
        <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
          Top {data.knowledgeTopK}
        </span>
      )}
    </div>
  );
}

export function CodeContent({ data }: Props) {
  const lang = data.codeLanguage ?? "javascript";
  return (
    <div className="flex flex-col gap-1">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium self-start" style={{ background: "rgba(16,185,129,0.2)", color: "rgba(16,185,129,0.9)" }}>
        {lang === "python" ? "Python" : "JavaScript"}
      </span>
      {data.codeScript && (
        <div className="text-[10px] font-mono truncate" style={{ color: "rgba(248,248,248,0.5)" }}>
          {data.codeScript.split("\n")[0]}
        </div>
      )}
    </div>
  );
}

export function HttpRequestContent({ data }: Props) {
  const method = data.httpMethod ?? "GET";
  const colors: Record<string, string> = { GET: "#22c55e", POST: "#3b82f6", PUT: "#f59e0b", DELETE: "#ef4444" };
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1.5">
        <span className="rounded px-1 py-0.5 text-[9px] font-bold" style={{ background: `${colors[method]}22`, color: colors[method] }}>
          {method}
        </span>
      </div>
      {data.httpUrl && (
        <div className="text-[10px] truncate" style={{ color: "rgba(248,248,248,0.5)" }}>
          {data.httpUrl}
        </div>
      )}
    </div>
  );
}

export function TemplateTransformContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      Jinja2 模板渲染
    </span>
  );
}

export function VariableAggregatorContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      合并多路输出
    </span>
  );
}

export function ToolContent({ data }: Props) {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {data.toolId ? `工具: ${data.toolId}` : "选择工具"}
    </span>
  );
}

export function HumanInputContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      暂停等待人工确认
    </span>
  );
}

export function AnswerContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      直接输出回答内容
    </span>
  );
}

export function LoopContent({ data }: Props) {
  const max = data.loopMaxIterations;
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {max ? `最大 ${max} 次循环` : "设置循环条件"}
    </span>
  );
}

export function IterationStartContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      当前迭代项入口
    </span>
  );
}

export function LoopStartContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      循环体入口
    </span>
  );
}

export function LoopEndContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      循环体结束
    </span>
  );
}

export function AgentV2Content({ data }: Props) {
  const outputs = (data.agentV2Outputs ?? []) as Array<{ name: string }>;
  return (
    <div className="flex items-center gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(139,92,246,0.2)", color: "rgba(139,92,246,0.9)" }}>
        V2
      </span>
      {outputs.length > 0 && (
        <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
          {outputs.length} 输出
        </span>
      )}
    </div>
  );
}

export function DocumentExtractorContent() {
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      提取文档文本内容
    </span>
  );
}

export function VariableAssignerContent({ data }: Props) {
  const target = data.assignerTarget;
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {target ? `${target} = ...` : "设置变量值"}
    </span>
  );
}

export function ListOperatorContent({ data }: Props) {
  const action = data.listOperatorAction;
  const labels: Record<string, string> = { filter: "过滤", map: "映射", sort: "排序", limit: "截取" };
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {labels[(action as string) ?? "filter"] ?? "列表操作"}
    </span>
  );
}

export function TriggerScheduleContent({ data }: Props) {
  const freq = data.scheduleFrequency;
  const labels: Record<string, string> = { minutely: "每分钟", hourly: "每小时", daily: "每天", weekly: "每周", monthly: "每月", custom: "Cron" };
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {labels[(freq as string) ?? "daily"] ?? "定时触发"}
    </span>
  );
}

export function TriggerWebhookContent({ data }: Props) {
  const method = (data.webhookMethod as string) ?? "POST";
  const colors: Record<string, string> = { GET: "#22c55e", POST: "#3b82f6", PUT: "#f59e0b" };
  return (
    <div className="flex items-center gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-bold" style={{ background: `${colors[method]}22`, color: colors[method] }}>
        {method}
      </span>
      {(data.webhookPath as string) && (
        <span className="text-[10px] truncate" style={{ color: "rgba(248,248,248,0.5)" }}>
          {data.webhookPath as string}
        </span>
      )}
    </div>
  );
}

export function TriggerPluginContent({ data }: Props) {
  const provider = data.pluginProvider;
  return (
    <span className="text-[10px]" style={{ color: "rgba(248,248,248,0.35)" }}>
      {provider ? `插件: ${provider}` : "选择插件"}
    </span>
  );
}

export function DatasourceContent({ data }: Props) {
  const type = data.datasourceType;
  const labels: Record<string, string> = { api: "API", database: "数据库", storage: "存储", plugin: "插件" };
  return (
    <div className="flex items-center gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(16,185,129,0.2)", color: "rgba(16,185,129,0.9)" }}>
        {labels[(type as string) ?? "api"] ?? "数据源"}
      </span>
      {(data.datasourcePlugin as string) && (
        <span className="text-[9px] truncate" style={{ color: "rgba(248,248,248,0.4)" }}>
          {data.datasourcePlugin as string}
        </span>
      )}
    </div>
  );
}

export function KnowledgeIndexContent({ data }: Props) {
  const mode = data.indexRetrievalMode;
  const labels: Record<string, string> = { semantic: "语义", keyword: "关键词", hybrid: "混合" };
  return (
    <div className="flex items-center gap-1.5">
      <span className="rounded px-1 py-0.5 text-[9px] font-medium" style={{ background: "rgba(6,182,212,0.2)", color: "rgba(6,182,212,0.9)" }}>
        索引
      </span>
      <span className="text-[9px]" style={{ color: "rgba(248,248,248,0.35)" }}>
        {labels[(mode as string) ?? "semantic"] ?? "语义"} · {(data.indexChunkSize as number) ?? 500} 块
      </span>
    </div>
  );
}
