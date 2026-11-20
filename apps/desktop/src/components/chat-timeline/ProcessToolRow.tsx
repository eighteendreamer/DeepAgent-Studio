import { useMemo } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ImagePreviewTrigger } from "../ImagePreviewTrigger";
import { BookOpen, Bot, ChevronRight, FileCode, Folder, GitBranch, Globe, ImageIcon, ListChecks, Search, Terminal, Wrench } from "lucide-react";
import type { ToolCall } from "../../types";
import type { DagExecution } from "../../types/dag";
import { Button } from "../shadcn/button";
import { HoverInfo } from "../ui/HoverInfo";
import { CodeBlock, CodeBlockActions, CodeBlockContent, CodeBlockCopyButton, CodeBlockHeader, CodeBlockTitle } from "../shadcn/code-block";
import { CollapseBlurPanel } from "./CollapseBlurPanel";
import { useProcessDetail } from "./ProcessDisclosureContext";
import { DagExecutionRow } from "./DagExecutionRow";
import { FileChangeBlock } from "./FileChangeBlock";
import { presentTool, type ToolPresentation } from "./toolPresentation";
import { formatMs } from "./format";
import { MarkdownText } from "../MarkdownText";

const icons = { skill: Wrench, directory: Folder, read: BookOpen, change: FileCode, command: Terminal,
  search: Search, git: GitBranch, planning: ListChecks, agent: Bot, file_read: Globe };
const statuses = {
  running: { label: "执行中", className: "text-text-secondary" },
  ok: { label: "完成", className: "text-emerald-700 dark:text-emerald-400" },
  error: { label: "失败", className: "text-red-600 dark:text-red-400" },
  blocked: { label: "已阻止", className: "text-orange-700 dark:text-orange-400" },
};

export function ProcessToolRow({ tool }: { tool: ToolCall }) {
  if (tool.name === "plan_execute" && tool.output) {
    const execution = parseDagExecution(tool);
    if (execution) return <DagExecutionRow execution={execution} />;
  }
  return <StandardToolRow tool={tool} />;
}

function StandardToolRow({ tool }: { tool: ToolCall }) {
  const [open, toggle] = useProcessDetail("tool:" + tool.call_id);
  const summary = useMemo(() => presentTool(tool), [tool]);
  const { t } = useTranslation();
  const images = tool.status === "ok" ? (tool.images ?? []).flatMap(({ id, path }) => path ? [{ id, path }] : []) : [];
  const status = statuses[tool.status];
  const label = images.length ? t("composer.attachments.viewedImages", { count: images.length })
    : tool.name === "screen_capture" ? t("composer.attachments.captureImage") : summary.label;
  const Icon = images.length || tool.name === "screen_capture" ? ImageIcon : icons[summary.kind as keyof typeof icons] || Wrench;
  const canOpen = Boolean(images.length || summary.raw || summary.read || summary.command || summary.changes.length || summary.skillBody || summary.entries);
  return <div className="min-w-0">
    <Button variant="ghost" aria-expanded={open && canOpen} onClick={() => canOpen && toggle()}
      aria-label={[label, summary.target, status.label].filter(Boolean).join(" · ")}
      className="group/tool h-auto w-full min-w-0 justify-start gap-2 rounded-md px-1 py-1 text-left text-[13px] font-normal leading-5 hover:bg-hover-bg">
      <Icon size={13} className="shrink-0 text-text-secondary/80" />
      <span className="shrink-0 text-text-base">{label}</span>
      {summary.target && <HoverInfo content={summary.target}><code className="min-w-0 truncate rounded border border-border-theme bg-hover-bg/40 px-1.5 py-0.5 font-mono text-[11px] text-text-secondary">{summary.target}</code></HoverInfo>}
      {summary.kind === "read" && summary.description && <span className="shrink-0 text-[11px] text-text-secondary">{summary.description}</span>}
      <span className="ml-auto flex shrink-0 items-center gap-2 text-[11px]">
        <span className={status.className}>{status.label}</span>
        {typeof tool.durationMs === "number" && <span className="tabular-nums text-text-secondary/70">{formatMs(tool.durationMs)}</span>}
        {canOpen && <ChevronRight size={12} className={"text-text-secondary/60 transition-transform " + (open ? "rotate-90" : "")} />}
      </span>
    </Button>
    <CollapseBlurPanel open={open && canOpen} innerClassName="ml-5 mt-1 mb-2 min-w-0">
      {open && (images.length ? <div className="flex flex-wrap gap-2">
        {images.map((image, index) => <ImagePreviewTrigger
          key={`${image.id}:${index}`}
          src={convertFileSrc(image.path)}
          alt={summary.target || label}
          triggerClassName="max-w-full rounded-lg border border-border-theme"
          imageClassName="block max-h-32 w-auto object-contain"
        />)}
      </div> : <ToolDetail summary={summary} tool={tool} />)}
    </CollapseBlurPanel>
  </div>;
}

function ToolDetail({ summary, tool }: { summary: ToolPresentation; tool: ToolCall }) {
  const specialized = Boolean(summary.read || summary.command || summary.changes.length || summary.skillBody || summary.entries);
  const command = summary.command;
  return <div className="flex min-w-0 flex-col gap-2">
    {summary.skillBody && <div className="max-h-80 overflow-auto border-l border-border-theme pl-3"><MarkdownText text={summary.skillBody} className="text-[13px]" /></div>}
    {summary.entries && <CodeBlock size="sm"><CodeBlockContent aria-label="目录内容">
      {summary.entries.length ? summary.entries.map(entry => entry.name + (entry.directory ? "/" : "")).join("\n") : "空目录"}
    </CodeBlockContent></CodeBlock>}
    {summary.read && <CodeBlock size="sm">
      <CodeBlockHeader><BookOpen size={13} /><HoverInfo content={summary.target}><CodeBlockTitle>{summary.target.split(/[\\/]/).pop() || "文件内容"}</CodeBlockTitle></HoverInfo>
        <CodeBlockActions><CodeBlockCopyButton value={summary.read.text} aria-label="复制文件内容" /></CodeBlockActions></CodeBlockHeader>
      <CodeBlockContent lineNumbers startLine={summary.read.startLine}>{summary.read.text}</CodeBlockContent>
    </CodeBlock>}
    {summary.changes.map((change, index) => <FileChangeBlock key={index} change={change} path={summary.target} />)}
    {command && <CodeBlock size="sm" variant={tool.status === "error" ? "destructive" : "default"}>
      <CodeBlockHeader><Terminal size={13} /><CodeBlockTitle>终端</CodeBlockTitle>
        <CodeBlockActions><CodeBlockCopyButton value={[command.command, command.stdout, command.stderr].filter(Boolean).join("\n")} aria-label="复制终端记录" /></CodeBlockActions></CodeBlockHeader>
      <CodeBlockContent wrap>{"$ " + command.command}</CodeBlockContent>
      {command.stdout && <CodeBlockContent wrap aria-label="标准输出" className="border-t border-border-theme">{command.stdout}</CodeBlockContent>}
      {command.stderr && <CodeBlockContent wrap aria-label="标准错误" className="border-t border-red-500/20 text-red-700 dark:text-red-300">{command.stderr}</CodeBlockContent>}
    </CodeBlock>}
    {summary.matches.length > 0 ? <CodeBlock size="sm"><CodeBlockContent>{summary.matches.join("\n")}</CodeBlockContent></CodeBlock>
      : !specialized && summary.raw && <CodeBlock size="sm"><CodeBlockContent wrap>{summary.kind === "change" && summary.description ? summary.description : summary.raw}</CodeBlockContent></CodeBlock>}
  </div>;
}



function parseDagExecution(tool: ToolCall): DagExecution | null {
  try {
    const output = typeof tool.output === 'string' ? JSON.parse(tool.output) : tool.output;

    // Check if output contains DAG execution data
    if (!output || typeof output !== 'object') return null;

    // Try to extract DAG data from various possible locations
    const dagData = output.dag_execution || output.dagExecution || output.execution || output;

    if (!dagData.executionId && !dagData.execution_id) return null;
    if (!dagData.nodes || !Array.isArray(dagData.nodes)) return null;

    // Transform to our DagExecution format
    const execution: DagExecution = {
      executionId: dagData.executionId || dagData.execution_id || `exec_${Date.now()}`,
      title: dagData.title || tool.detail || 'DAG Execution',
      nodes: dagData.nodes.map((node: any) => ({
        id: node.id || node.node_id || 'unknown',
        goal: node.goal || node.description || '',
        phase: node.phase,
        role: node.role,
        status: node.status || 'pending',
        dependsOn: node.dependsOn || node.depends_on || [],
        startedAt: node.startedAt || node.started_at,
        completedAt: node.completedAt || node.completed_at,
        duration: node.duration || node.durationMs,
        summary: node.summary,
        error: node.error,
      })),
      createdAt: dagData.createdAt || dagData.created_at || Date.now(),
      updatedAt: dagData.updatedAt || dagData.updated_at || Date.now(),
    };

    return execution;
  } catch (error) {
    return null;
  }
}
