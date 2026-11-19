import type { ToolCall } from "../../types";

type RecordValue = Record<string, unknown>;
export type FileChange = { before: string; after: string; startLine?: number; scope: string };
export type ToolPresentation = {
  kind: string;
  label: string;
  target: string;
  description: string;
  raw: string;
  matches: string[];
  skillBody?: string;
  entries?: { name: string; directory: boolean }[];
  read?: { text: string; startLine: number; truncated: boolean };
  command?: { command: string; stdout: string; stderr: string; exitCode?: number };
  changes: FileChange[];
};

function record(value: unknown): RecordValue {
  if (typeof value === "string") {
    try { return record(JSON.parse(value)); } catch { return {}; }
  }
  return value && typeof value === "object" && !Array.isArray(value) ? value as RecordValue : {};
}
const string = (value: unknown): string => typeof value === "string" ? value : "";
const number = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) ? value : undefined;
const compact = (value: string) => value.replace(/\s+/g, " ").trim().slice(0, 180);
function rawText(value: unknown): string {
  if (value == null) return "";
  if (typeof value === "string") {
    try { return JSON.stringify(JSON.parse(value), null, 2); } catch { return value; }
  }
  return JSON.stringify(value, null, 2);
}

/** One presentation projection for both section titles and individual tools.
 * Raw tool output remains available, but never becomes a navigation title. */
export function presentTool(tool: ToolCall): ToolPresentation {
  const args = record(tool.args);
  const data = record(tool.output ?? tool.detail);
  const meta = record(tool.meta);
  const name = tool.name.toLowerCase();
  const path = tool.filePath || string(data.path) || string(args.path) || string(args.file_path) || string(meta.path);
  const kind = name === "skill" ? "skill" : name === "list_dir" || name === "glob" ? "directory"
    : name === "read_file" ? "read" : ["write_file", "edit_file", "multi_edit"].includes(name) ? "change"
    : ["bash", "shell"].includes(name) || tool.toolKind === "command_execution" ? "command"
    : tool.toolKind || "tool";
  const labels: Record<string, string> = {
    skill: "使用技能", directory: "查看目录", read: "读取文件", change: "修改文件",
    command: "运行命令", search: "搜索", git: "检查 Git", planning: "更新计划", agent: "委派任务",
  };
  const result: ToolPresentation = {
    kind, label: labels[kind] || tool.name, target: compact(path), description: "",
    raw: rawText(tool.output ?? tool.detail ?? args), changes: [],
    matches: Array.isArray(data.matches) ? data.matches.filter((v): v is string => typeof v === "string") : [],
  };
  if (kind === "skill") {
    result.target = compact(string(data.name) || string(data.id) || string(args.name) || string(args.skill));
    result.skillBody = string(data.body) || undefined;
  } else if (name === "list_dir" && Array.isArray(data.entries)) {
    result.entries = data.entries.map(record).filter(entry => typeof entry.name === "string")
      .map(entry => ({ name: string(entry.name), directory: entry.dir === true }));
  } else if (kind === "read") {
    if (data.unchanged_stub === true) result.description = "文件未变化，复用已读取的内容";
    else if (typeof data.content === "string") {
      const startLine = Math.max(1, number(data.offset) ?? number(args.offset) ?? 1);
      const lines = data.content.split("\n");
      // read_file uses cat -n prefixes. Only remove them when every number
      // matches the actual returned range, preserving tabs inside the source.
      const numbered = lines.every((line, i) => Number(/^\s*(\d+)\t/.exec(line)?.[1]) === startLine + i);
      result.read = { text: numbered ? lines.map(line => line.replace(/^\s*\d+\t/, "")).join("\n") : data.content,
        startLine, truncated: data.truncated === true };
      result.description = `第 ${startLine}–${startLine + Math.max(0, lines.length - 1)} 行${data.truncated ? " · 部分内容" : ""}`;
    }
  } else if (kind === "command") {
    const command = string(data.command) || string(args.command) || string(args.cmd) || string(meta.command);
    result.target = compact(command);
    result.command = { command, stdout: string(data.stdout), stderr: string(data.stderr) || string(data.error), exitCode: number(data.exit_code) };
    if (!result.command.stdout && !result.command.stderr && !Object.keys(data).length && tool.output != null) {
      result.command.stdout = rawText(tool.output);
    }
  } else if (kind === "change" && tool.status === "ok") {
    const preview = record(record(meta.client_metadata).file_change);
    if (preview.version === 1 && typeof preview.before === "string" && typeof preview.after === "string") {
      result.changes = [{ before: preview.before, after: preview.after, startLine: number(preview.start_line), scope: "本次修改" }];
    } else if (preview.omitted === true) {
      result.description = "修改已完成，变更过大，未保存代码预览";
    } else {
      const edits = name === "multi_edit" && Array.isArray(args.edits) ? args.edits.map(record) : [args];
      result.changes = edits.flatMap(edit => typeof edit.old === "string" && typeof edit.new === "string"
        ? [{ before: edit.old, after: edit.new, scope: "替换片段" }] : []);
      result.description = result.changes.length ? "历史记录 · 仅保留替换片段" : "修改已完成，此记录没有保存修改前内容";
    }
  }
  if (!result.target) result.target = compact(string(args.pattern) || string(args.query) || string(args.url));
  if (result.matches.length) result.description = `${result.matches.length} 个匹配`;
  return result;
}

/** Protect the existing quadratic diff service, including old unbounded args. */
export function canPreviewChange(change: FileChange): boolean {
  return change.before.length + change.after.length <= 128 * 1024 &&
    change.before.split("\n").length <= 1200 && change.after.split("\n").length <= 1200;
}
