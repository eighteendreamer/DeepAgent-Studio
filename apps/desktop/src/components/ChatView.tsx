import { useState, useEffect, useMemo, useRef, useCallback } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { convertFileSrc } from "@tauri-apps/api/core";
import type {
  ChatMessage,
  ComposerAttachment,
  ComposerMention,
  ComposerSkillSelection,
  ContextUsageSnapshot,
  ToolCall,
  TimelineEntry,
  ApprovalRequest,
  ProjectMapStatus,
} from "../types";
import { Composer } from "./Composer";
import { ApprovalDialog } from "./ApprovalDialog";
import { RenameSessionDialog } from "./RenameSessionDialog";
import { EnvironmentInfoMenu } from "./EnvironmentInfoMenu";
import type { OutputItem } from "./EnvironmentInfoMenu";
import { ProjectMapStatusBadge } from "./project-map/ProjectMapPanel";
import { ToolLauncherPanel } from "./ToolLauncherPanel";
import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import { Button as AriaButton } from "react-aria-components";
import {
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuSub,
  DropdownMenuSubContent,
} from "./ui/DropdownMenu";
import { message as toast } from "./message";
import { useTranslation } from "react-i18next";
import {
  OPEN_AUTOMATION_EVENT,
  previewReadDataUrl,
  projectMapStatus,
  SEND_TO_CHAT_EVENT,
} from "../api";
import { useGitStatus } from "../hooks/useGitStatus";
import { GitWorkbench } from "./git/GitWorkbench";
import {
  createPluginTab,
  PLUGIN_TOOL_CARDS,
  renderPluginTab,
  type PluginConnectionSummary,
  type PluginTab,
  type PluginToolCard,
} from "./plugins/pluginRegistry";
import { RightSidebarWorkbench } from "./RightSidebarWorkbench";
import { ChatTimeline } from "./chat-timeline/ChatTimeline";
import { usePanelPresence } from "../hooks/usePanelPresence";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";
const PROJECT_MAP_TAB_ID = "project-map";

interface Props {
  /** Active session id for per-session UI persistence. */
  sessionId?: string | null;
  /** Active chat identity, including pending runs before a real session id exists. */
  sessionKey?: string | null;
  messages: ChatMessage[];
  onSend: (
    text: string,
    attachments?: ComposerAttachment[],
    selectedSkills?: ComposerSkillSelection[],
    mentions?: ComposerMention[],
    displayText?: string,
  ) => void;
  /** Fork the current session into a new branch from its latest point. */
  onFork?: () => void;
  /** Rewind the current session to a timeline sequence (destructive). */
  onRewind?: (toSeq: number) => void;
  /** Export the current session transcript. */
  onExport?: (format: "markdown" | "json") => void;
  /** Copy the current session transcript. */
  onCopy?: () => void;
  /** Rename the current session. */
  onRename?: (title: string) => void | Promise<void>;
  /** Open the current session in a new window. */
  onOpenInNewWindow?: () => void;
  /** Pin or unpin the current session. */
  onPin?: () => void;
  /** Archive the current session. */
  onArchive?: () => void;
  /** Whether the current session is pinned. */
  pinned?: boolean;
  /** Current session display title. */
  title?: string | null;
  /** The session timeline, used to offer rewind anchors. */
  timeline?: TimelineEntry[];
  /** Head-of-queue tool-approval request to show floating above the composer. */
  approval?: ApprovalRequest | null;
  /** Total queued approvals (including the current one). */
  approvalQueueCount?: number;
  /** Resolve the current approval (allow / deny). */
  onApprovalDecision?: (req: ApprovalRequest, approved: boolean) => void;
  /** True while a run is streaming - disables the composer send button. */
  busy?: boolean;
  /** Stop the in-flight run (manual cancel). */
  onStop?: () => void;
  /** Current session is in read-only Plan mode. */
  planMode?: boolean;
  /** Explicitly selected project path. Empty means the chat is not project-bound. */
  activeProjectPath?: string | null;
  /** Incremented by parent when another UI surface asks to open the project map. */
  projectMapOpenSignal?: number;
  /** Latest context usage snapshot for the active session/run. */
  contextUsage?: ContextUsageSnapshot | null;
}

type OfficeContextView = {
  type: string;
  title?: string;
  meta: Array<{ label: string; value: string }>;
  body?: string;
  prompt: string;
};

/// Count the number of lines in `s`, treating an empty string as 0 lines and
/// not double-counting a trailing newline (so `"a\nb\n"` -> 2, `"a"` -> 1, `""` -> 0).
/// Used by [`computeChatChanges`] to score `edit_file` / `multi_edit` /
/// `write_file` tool calls into a `+N -M` summary.
function countLines(s: string): number {
  if (!s) return 0;
  let n = 0;
  let lastWasNewline = true;
  for (const ch of s) {
    if (lastWasNewline) n++;
    lastWasNewline = ch === "\n";
  }
  return n;
}

/// Aggregate `+lines / -lines` across all `write_file` / `edit_file` /
/// `multi_edit` tool calls in the conversation.
///
/// This is the **agent-driven** changeset (what this conversation has
/// actually written to disk via tool calls), not git's working-tree diff.
/// We surface it in the env panel because:
/// - In a freshly-opened project that isn't yet a git repo, `git diff
///   --shortstat HEAD` returns nothing -> the panel used to show `+0 -0`
///   regardless of how much the agent had touched the workspace.
/// - The "what did this conversation do" framing is more directly useful
///   for an AI-driven UX than "what's uncommitted in your filesystem"
///   (which the user can always inspect via the terminal).
///
/// Mirrors `claudecode/restored-src/src/components/StructuredDiff.tsx`'s
/// per-edit `+/-` accounting, summed across the session.
///
/// Tool wire-format (from `crates/deepagent-builtins/src/file_tools.rs`):
///   - `write_file` -> `{ path, content }`
///   - `edit_file`  -> `{ path, old, new, replace_all? }`
///   - `multi_edit` -> `{ path, edits: [{ old, new, replace_all? }, ...] }`
///
/// Notes / known undercounting:
///   - `write_file` deletions are 0 because the agent doesn't ship the prior
///     file content; an overwrite of a long file shows only additions.
///   - Failed (`status !== "ok"`) tool calls are skipped - they didn't
///     materialize a change.
function computeChatChanges(messages: ChatMessage[]): {
  additions: number;
  deletions: number;
} {
  const allTools: ToolCall[] = [];
  for (const msg of messages) {
    if (msg.tools) allTools.push(...msg.tools);
    for (const part of msg.parts ?? []) {
      if (part.kind === "tool") allTools.push(part.tool);
    }
  }

  let additions = 0;
  let deletions = 0;

  for (const tool of allTools) {
    if (tool.status !== "ok") continue;
    if (!tool.args) continue;
    let parsed: unknown;
    try {
      parsed = JSON.parse(tool.args);
    } catch {
      continue;
    }
    if (!parsed || typeof parsed !== "object") continue;
    const args = parsed as { [k: string]: unknown };

    if (tool.name === "write_file") {
      const content = typeof args.content === "string" ? args.content : "";
      additions += countLines(content);
      // Deletions undercount: prior file content isn't carried on the wire.
    } else if (tool.name === "edit_file") {
      const oldStr = typeof args.old === "string" ? args.old : "";
      const newStr = typeof args.new === "string" ? args.new : "";
      additions += countLines(newStr);
      deletions += countLines(oldStr);
    } else if (tool.name === "multi_edit") {
      const edits = Array.isArray(args.edits) ? (args.edits as unknown[]) : [];
      for (const e of edits) {
        if (!e || typeof e !== "object") continue;
        const edit = e as { [k: string]: unknown };
        const oldStr = typeof edit.old === "string" ? edit.old : "";
        const newStr = typeof edit.new === "string" ? edit.new : "";
        additions += countLines(newStr);
        deletions += countLines(oldStr);
      }
    }
  }

  return { additions, deletions };
}

/// Image-file extension matcher: covers the formats a model can plausibly
/// generate or reference. Anchored to the end (or to ?/# query/fragment) so
/// it doesn't false-positive on names that happen to contain `.png` mid-path.
const IMAGE_EXT_PATTERN = /\.(png|jpe?g|gif|webp|svg|bmp|ico|tiff?|avif)(?:[?#]|$)/i;

/// Tools that materialize a file on disk. We pull the `path` field from
/// `tool.args` (always full JSON; `tool.detail` is truncated to <=200 chars
/// upstream and isn't reliable for parsing).
const FILE_GENERATING_TOOLS: ReadonlySet<string> = new Set([
  "write_file",
  "edit_file",
  "multi_edit",
]);

function parseFieldFromArgs(argsJson: string | undefined, field: string): string | null {
  if (!argsJson) return null;
  try {
    const obj = JSON.parse(argsJson);
    const v = obj?.[field];
    return typeof v === "string" ? v : null;
  } catch {
    return null;
  }
}

function parseTodoSnapshotFromArgs(argsJson: string | undefined): OutputItem | null {
  if (!argsJson) return null;
  try {
    const obj = JSON.parse(argsJson);
    if (!Array.isArray(obj?.todos)) return null;
    let pending = 0;
    let inProgress = 0;
    let completed = 0;
    for (const t of obj.todos) {
      const status = String(t?.status ?? "");
      if (status === "pending") pending++;
      else if (status === "in_progress" || status === "in-progress" || status === "inprogress") {
        inProgress++;
      } else if (status === "completed" || status === "complete" || status === "done") {
        completed++;
      }
    }
    const total = obj.todos.length;
    const parts: string[] = [];
    if (completed > 0) parts.push(`已完成 ${completed}`);
    if (inProgress > 0) parts.push(`进行中 ${inProgress}`);
    if (pending > 0) parts.push(`待办 ${pending}`);
    const label = `Todo · ${total}${parts.length ? " · " + parts.join(" · ") : ""}`;
    return { kind: "todo", label, total, pending, inProgress, completed };
  } catch {
    return null;
  }
}
function collectOutputItems(messages: ChatMessage[]): OutputItem[] {
  // Walk every tool call once (legacy `tools` array + ordered `parts`).
  const allTools: ToolCall[] = [];
  for (const message of messages) {
    if (message.tools) allTools.push(...message.tools);
    for (const part of message.parts ?? []) {
      if (part.kind === "tool") allTools.push(part.tool);
    }
  }

  const seen = new Set<string>();
  const items: OutputItem[] = [];
  let latestTodo: OutputItem | null = null;

  // Phase 1: file/image generation + todo snapshots from tool calls.
  for (const tool of allTools) {
    // Only count completed-OK tools - running calls aren't done, errored
    // calls didn't materialize their effect.
    if (tool.status !== "ok") continue;

    if (FILE_GENERATING_TOOLS.has(tool.name)) {
      const path = parseFieldFromArgs(tool.args, "path");
      if (!path) continue;
      const isImage = IMAGE_EXT_PATTERN.test(path);
      const key = `${isImage ? "image" : "file"}:${path}`;
      if (seen.has(key)) continue;
      seen.add(key);
      if (isImage) {
        items.push({ kind: "image", label: path, source: "tool" });
      } else {
        const action: "write" | "edit" = tool.name === "write_file" ? "write" : "edit";
        items.push({ kind: "file", label: path, action });
      }
    }

    if (tool.name === "todo_write") {
      const snap = parseTodoSnapshotFromArgs(tool.args);
      // Always overwrite - only the latest snapshot is shown.
      if (snap) latestTodo = snap;
    }
  }

  // Phase 2: URL extraction from message text. URLs ending in image
  // extensions get classified as images so the UI uses the picture icon.
  const chunks: string[] = [];
  for (const message of messages) {
    if (message.content) chunks.push(message.content);
    for (const part of message.parts ?? []) {
      if (part.kind === "text" || part.kind === "reasoning") chunks.push(part.text);
      if (part.kind === "tool") {
        if (part.tool.detail) chunks.push(part.tool.detail);
        if (part.tool.args) chunks.push(part.tool.args);
      }
    }
    for (const tool of message.tools ?? []) {
      if (tool.detail) chunks.push(tool.detail);
      if (tool.args) chunks.push(tool.args);
    }
  }
  const text = chunks.join("\n");
  const urlPattern =
    /\b(?:https?:\/\/)?(?:localhost|127\.0\.0\.1)(?::\d+)?(?:\/[^\s<>"'`)]*)?|\bhttps?:\/\/[^\s<>"'`)]+/gi;
  for (const match of text.matchAll(urlPattern)) {
    const cleaned = match[0].replace(/[),.;\]}"']+$/, "");
    if (!cleaned) continue;
    if (IMAGE_EXT_PATTERN.test(cleaned)) {
      const key = `image:${cleaned}`;
      if (seen.has(key)) continue;
      seen.add(key);
      items.push({ kind: "image", label: cleaned, source: "url" });
    } else {
      const key = `url:${cleaned}`;
      if (seen.has(key)) continue;
      seen.add(key);
      items.push({ kind: "url", label: cleaned });
    }
  }

  // Phase 3: todo lands first - it's the most actionable summary.
  if (latestTodo) items.unshift(latestTodo);

  // Cap at 15 - enough to surface a turn's worth of output without crowding
  // the floating panel.
  return items.slice(0, 15);
}

const OFFICE_FIELD_LABELS = [
  "类型",
  "当前预览文件",
  "文件类型",
  "路径",
  "已导出",
  "内容",
  "提取内容摘要",
];

function parseOfficeContextMessage(content: string): OfficeContextView | null {
  const match = content.match(/<office-context>\s*([\s\S]*?)\s*<\/office-context>\s*([\s\S]*)/);
  if (!match) return null;

  const fields = parseOfficeFields(match[1].trim());
  const type = fields.get("类型") || fields.get("文件类型") || "办公上下文";
  const title = fields.get("当前预览文件");
  const body = fields.get("内容") || fields.get("提取内容摘要");
  const meta = Array.from(fields.entries())
    .filter(([label]) => !["类型", "当前预览文件", "内容", "提取内容摘要"].includes(label))
    .map(([label, value]) => ({ label, value }));

  return {
    type,
    title,
    meta,
    body,
    prompt: match[2].trim(),
  };
}

function parseOfficeFields(context: string): Map<string, string> {
  const compactFields = parseCompactOfficeFields(context);
  if (!context.includes("\n") && compactFields.size > 0) {
    return compactFields;
  }

  const fields = new Map<string, string>();
  const lines = context.replace(/\r\n/g, "\n").split("\n");
  let current: string | null = null;

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    const label = OFFICE_FIELD_LABELS.find((candidate) => line.startsWith(`${candidate}:`));
    if (label) {
      current = label;
      fields.set(label, line.slice(label.length + 1).trim());
      continue;
    }
    if (current) {
      const prev = fields.get(current) ?? "";
      fields.set(current, prev ? `${prev}\n${line}` : line.trim());
    }
  }

  if (fields.size > 0) return fields;

  // Also tolerate a compact one-line payload copied from an old plain bubble.
  return compactFields;
}

function parseCompactOfficeFields(context: string): Map<string, string> {
  const fields = new Map<string, string>();
  const hits: Array<{ label: string; start: number; valueStart: number }> = [];

  for (const label of OFFICE_FIELD_LABELS) {
    const needle = `${label}:`;
    let start = context.indexOf(needle);
    while (start >= 0) {
      hits.push({ label, start, valueStart: start + needle.length });
      start = context.indexOf(needle, start + needle.length);
    }
  }

  hits.sort((a, b) => a.start - b.start);
  for (let i = 0; i < hits.length; i++) {
    const hit = hits[i];
    const end = hits[i + 1]?.start ?? context.length;
    const value = context.slice(hit.valueStart, end).trim();
    if (value) fields.set(hit.label, value);
  }

  return fields;
}

function copyText(text: string) {
  if (navigator.clipboard?.writeText) {
    navigator.clipboard
      .writeText(text)
      .then(() => toast.success("已复制"))
      .catch(() => fallbackCopyText(text));
    return;
  }
  fallbackCopyText(text);
}

function fallbackCopyText(text: string) {
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.appendChild(textarea);
  textarea.select();
  try {
    document.execCommand("copy");
    toast.success("已复制");
  } catch {
    toast.error("复制失败");
  } finally {
    document.body.removeChild(textarea);
  }
}

function stripAttachmentContext(content: string): string {
  return content.replace(/\n*<attachments>[\s\S]*?<\/attachments>\s*/g, "").trim();
}

function stripSkillContext(content: string): string {
  return content.replace(/\n*<skill-context\b[^>]*>[\s\S]*?<\/skill-context>\s*/g, "").trim();
}

function stripMentionContext(content: string): string {
  return content.replace(/\n*<context-items>[\s\S]*?<\/context-items>\s*/g, "").trim();
}

function parseSkillContexts(content: string): ComposerSkillSelection[] {
  const skills: ComposerSkillSelection[] = [];
  const contextRegex = /<skill-context\b([^>]*)>/g;
  let match: RegExpExecArray | null;
  while ((match = contextRegex.exec(content)) !== null) {
    const attrs = parseAttachmentAttrs(match[1]);
    const id = attrs.id?.trim();
    const name = attrs.name?.trim();
    if (id && name) skills.push({ id, name });
  }
  return skills;
}

function unescapeXmlText(value: string): string {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

function parseMentionContexts(content: string): ComposerMention[] {
  const match = content.match(/<context-items>\s*([\s\S]*?)\s*<\/context-items>/);
  if (!match) return [];
  const mentions: ComposerMention[] = [];
  const itemRegex = /<context-item\b([^>]*)>([\s\S]*?)<\/context-item>/g;
  let item: RegExpExecArray | null;
  while ((item = itemRegex.exec(match[1])) !== null) {
    const attrs = parseAttachmentAttrs(item[1]);
    const kind = attrs.kind;
    if (kind === "plan_mode") {
      mentions.push({ kind: "plan_mode" });
      continue;
    }
    if (kind === "goal") {
      const text = unescapeXmlText(item[2].trim());
      if (text) mentions.push({ kind: "goal", text });
      continue;
    }
    if (kind === "file" || kind === "folder") {
      const relPath = unescapeXmlText(attrs.path ?? "");
      const path = unescapeXmlText(attrs.absolute_path ?? relPath);
      if (!relPath && !path) continue;
      const normalized = relPath || path.replace(/\\/g, "/");
      const name = normalized.split(/[\\/]/).filter(Boolean).pop() ?? normalized;
      mentions.push({
        kind,
        path,
        relPath: normalized,
        name,
        isDir: kind === "folder",
      });
    }
  }
  return mentions;
}

function parseAttachmentContext(content: string): ComposerAttachment[] {
  const match = content.match(/<attachments>\s*([\s\S]*?)\s*<\/attachments>/);
  if (!match) return [];
  const attachments: ComposerAttachment[] = [];
  const blockRegex = /<attachment\s+([^>]*)>([\s\S]*?)<\/attachment>/g;
  let block: RegExpExecArray | null;
  while ((block = blockRegex.exec(match[1])) !== null) {
    const attrs = parseAttachmentAttrs(block[1]);
    const body = block[2];
    const path = body.match(/^path:\s*(.+)$/m)?.[1]?.trim();
    const sizeText = body.match(/^size:\s*(\d+)\s+bytes$/m)?.[1];
    const kind = normalizeAttachmentKind(attrs.kind, attrs.type);
    attachments.push({
      id: `parsed-${attrs.index ?? attachments.length + 1}-${attrs.name ?? "attachment"}`,
      kind,
      name: attrs.name ?? (kind === "image" ? "image" : "attachment"),
      mime: attrs.type ?? (kind === "image" ? "image/*" : "application/octet-stream"),
      size: sizeText ? Number(sizeText) || 0 : 0,
      source: "paste",
      originalPath: path,
      localPath: path,
      status: "ready",
    });
  }
  return attachments;
}

function parseAttachmentAttrs(raw: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  const attrRegex = /(\w+)="([^"]*)"/g;
  let match: RegExpExecArray | null;
  while ((match = attrRegex.exec(raw)) !== null) {
    attrs[match[1]] = match[2];
  }
  return attrs;
}

function normalizeAttachmentKind(
  kind?: string,
  mime?: string
): ComposerAttachment["kind"] {
  if (kind === "image" || kind === "text" || kind === "file") return kind;
  if (mime?.startsWith("image/")) return "image";
  if (mime?.startsWith("text/")) return "text";
  return "file";
}

function attachmentLabel(item: ComposerAttachment): string {
  if (item.status === "processing") return item.kind === "image" ? "识别中" : "处理中";
  if (item.status === "error") return item.error ?? "处理失败";
  if (item.kind === "image") return item.mime || "图片";
  if (item.kind === "text") return "文本";
  return item.mime || "文件";
}

function formatAttachmentSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function attachmentPath(item: ComposerAttachment): string | null {
  const path = item.originalPath ?? item.localPath;
  return path?.trim() || null;
}

function UserAttachmentCard({ item }: { item: ComposerAttachment }) {
  const [loadedSrc, setLoadedSrc] = useState<string | null>(() => item.dataUrl ?? null);
  const [imageFailed, setImageFailed] = useState(false);
  const path = attachmentPath(item);

  useEffect(() => {
    let cancelled = false;
    setImageFailed(false);

    if (item.kind !== "image") {
      setLoadedSrc(null);
      return () => {
        cancelled = true;
      };
    }

    if (item.dataUrl) {
      setLoadedSrc(item.dataUrl);
      return () => {
        cancelled = true;
      };
    }

    if (!path) {
      setLoadedSrc(null);
      return () => {
        cancelled = true;
      };
    }

    setLoadedSrc(null);
    previewReadDataUrl(path)
      .then((dataUrl) => {
        if (!cancelled && dataUrl) setLoadedSrc(dataUrl);
      })
      .catch(() => {
        const assetSrc = convertFileSrc(path);
        if (!cancelled) setLoadedSrc(assetSrc);
      });

    return () => {
      cancelled = true;
    };
  }, [item.kind, item.dataUrl, path]);

  const imageSrc = item.kind === "image" && !imageFailed ? loadedSrc : null;

  return (
    <div
      className="group/attachment flex min-h-14 max-w-[220px] items-center gap-2 overflow-hidden rounded-xl border border-border-theme bg-elevated-bg px-2 py-2 shadow-sm"
      title={item.originalPath ?? item.localPath ?? item.name}
    >
      {imageSrc ? (
        <img
          src={imageSrc}
          alt={item.name}
          className="h-12 w-12 shrink-0 rounded-lg border border-border-theme object-cover"
          onError={() => setImageFailed(true)}
        />
      ) : (
        <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-lg border border-border-theme bg-sidebar-bg text-text-secondary">
          <FontAwesomeIcon
            icon={["fas", item.kind === "image" ? "image" : item.kind === "text" ? "file-lines" : "file"]}
            className="text-[15px]"
          />
        </div>
      )}
      <div className="min-w-0 flex-1 text-left">
        <div className="truncate text-[12px] font-medium text-text-base">{item.name}</div>
        <div className="truncate text-[11px] text-text-secondary">
          {attachmentLabel(item)}
          {formatAttachmentSize(item.size) ? ` · ${formatAttachmentSize(item.size)}` : ""}
        </div>
      </div>
    </div>
  );
}

function UserAttachments({ attachments }: { attachments: ComposerAttachment[] }) {
  if (attachments.length === 0) return null;
  return (
    <div className="mb-2 flex max-w-[80%] flex-wrap justify-end gap-2">
      {attachments.map((item) => (
        <UserAttachmentCard key={item.id} item={item} />
      ))}
    </div>
  );
}

export function UserContextChips({
  skills,
  mentions,
}: {
  skills: ComposerSkillSelection[];
  mentions: ComposerMention[];
}) {
  if (skills.length === 0 && mentions.length === 0) return null;
  const mentionIcon = (mention: ComposerMention) => {
    if (mention.kind === "folder") return "folder";
    if (mention.kind === "file") return "file";
    if (mention.kind === "goal") return "bullseye";
    return "list-check";
  };
  const mentionLabel = (mention: ComposerMention) => {
    if (mention.kind === "goal") return "目标";
    if (mention.kind === "plan_mode") return "计划模式";
    return mention.relPath || mention.name;
  };
  const mentionTitle = (mention: ComposerMention) => {
    if (mention.kind === "goal") return mention.text;
    if (mention.kind === "plan_mode") return "开启计划模式";
    return mention.path;
  };

  return (
    <div className="mb-2 flex max-w-[80%] flex-wrap justify-end gap-1.5">
      {skills.map((skill) => (
        <span
          key={`skill-${skill.id}`}
          className="inline-flex max-w-[220px] items-center rounded-md border border-primary/20 bg-primary/10 px-2 py-1 text-[13px] font-medium text-primary"
          title={`${skill.name} (${skill.id})`}
        >
          <FontAwesomeIcon icon={["fas", "cube"]} className="mr-1.5 text-[11px]" />
          <span className="truncate">{skill.name || skill.id}</span>
        </span>
      ))}
      {mentions.map((mention, index) => (
        <span
          key={`mention-${index}-${mentionLabel(mention)}`}
          className="inline-flex max-w-[260px] items-center rounded-md border border-border-theme bg-sidebar-bg px-2 py-1 text-[13px] font-medium text-text-base"
          title={mentionTitle(mention)}
        >
          <FontAwesomeIcon icon={["fas", mentionIcon(mention) as any]} className="mr-1.5 text-[11px] text-text-secondary" />
          <span className="truncate">{mentionLabel(mention)}</span>
        </span>
      ))}
    </div>
  );
}

const USER_SKILL_MARKER = "\uE000";
const USER_MENTION_MARKER = "\uE001";

function UserInlineContent({
  content,
  skills,
  mentions,
}: {
  content: string;
  skills: ComposerSkillSelection[];
  mentions: ComposerMention[];
}) {
  const mentionIcon = (mention: ComposerMention) => {
    if (mention.kind === "folder") return "folder";
    if (mention.kind === "file") return "file";
    if (mention.kind === "goal") return "bullseye";
    return "list-check";
  };
  const mentionLabel = (mention: ComposerMention) => {
    if (mention.kind === "goal") return "目标";
    if (mention.kind === "plan_mode") return "计划模式";
    return mention.relPath || mention.name;
  };
  const mentionTitle = (mention: ComposerMention) => {
    if (mention.kind === "goal") return mention.text;
    if (mention.kind === "plan_mode") return "开启计划模式";
    return mention.path;
  };
  const renderSkillChip = (skill: ComposerSkillSelection, key: string) => (
    <span
      key={key}
      className="mx-0.5 inline-flex max-w-[220px] translate-y-[2px] items-center rounded-md border border-primary/20 bg-primary/10 px-1.5 py-0.5 text-[13px] font-medium leading-none text-primary"
      title={`${skill.name} (${skill.id})`}
    >
      <FontAwesomeIcon icon={["fas", "cube"]} className="mr-1 text-[11px]" />
      <span className="truncate">{skill.name || skill.id}</span>
    </span>
  );
  const renderMentionChip = (mention: ComposerMention, key: string) => (
    <span
      key={key}
      className="mx-0.5 inline-flex max-w-[260px] translate-y-[2px] items-center rounded-md border border-border-theme bg-sidebar-bg px-1.5 py-0.5 text-[13px] font-medium leading-none text-text-base"
      title={mentionTitle(mention)}
    >
      <FontAwesomeIcon icon={["fas", mentionIcon(mention) as any]} className="mr-1 text-[11px] text-text-secondary" />
      <span className="truncate">{mentionLabel(mention)}</span>
    </span>
  );

  const markerRegex = /[\uE000\uE001]/g;
  const hasMarkers = markerRegex.test(content);
  markerRegex.lastIndex = 0;
  if (!hasMarkers) {
    return (
      <>
        {skills.map((skill) => renderSkillChip(skill, `skill-prefix-${skill.id}`))}
        {mentions.map((mention, index) => renderMentionChip(mention, `mention-prefix-${index}`))}
        {content}
      </>
    );
  }

  const nodes: ReactNode[] = [];
  let offset = 0;
  let skillIndex = 0;
  let mentionIndex = 0;
  let index = 0;
  let match: RegExpExecArray | null;
  while ((match = markerRegex.exec(content)) !== null) {
    const text = content.slice(offset, match.index);
    if (text) nodes.push(<span key={`text-${index}`}>{text}</span>);
    if (match[0] === USER_SKILL_MARKER) {
      const skill = skills[skillIndex];
      if (skill) nodes.push(renderSkillChip(skill, `skill-${index}`));
      skillIndex += 1;
    } else if (match[0] === USER_MENTION_MARKER) {
      const mention = mentions[mentionIndex];
      if (mention) nodes.push(renderMentionChip(mention, `mention-${index}`));
      mentionIndex += 1;
    }
    offset = match.index + 1;
    index += 1;
  }
  const tail = content.slice(offset);
  if (tail) nodes.push(<span key="text-tail">{tail}</span>);
  return <>{nodes}</>;
}

export function UserTurn({
  message,
  busy,
  onResend,
}: {
  message: ChatMessage;
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
}) {
  const skills = message.selectedSkills?.length ? message.selectedSkills : parseSkillContexts(message.content);
  const mentions = message.mentions?.length ? message.mentions : parseMentionContexts(message.content);
  const parsedAttachments = parseAttachmentContext(message.content);
  const attachments = message.attachments?.length ? message.attachments : parsedAttachments;
  const visibleContent = stripMentionContext(stripAttachmentContext(stripSkillContext(message.content)));
  const hasVisibleMessage = Boolean(visibleContent || skills.length > 0 || mentions.length > 0);
  const office = parseOfficeContextMessage(visibleContent);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(visibleContent);

  const submitEdit = () => {
    const text = draft.trim();
    if (!text || busy) return;
    setEditing(false);
    onResend(text, skills, mentions);
  };

  return (
    <div className="flex flex-col items-end mb-8 w-full max-w-4xl mx-auto group">
      {editing ? (
        <div className="w-full max-w-[80%] rounded-2xl rounded-tr-sm bg-sidebar-bg p-3">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            className="min-h-[120px] w-full resize-y rounded-xl border border-border-theme bg-elevated-bg px-3 py-2 text-[14px] leading-relaxed text-text-base outline-none focus:border-primary/60"
            autoFocus
          />
          <div className="mt-2 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                setDraft(visibleContent);
                setEditing(false);
              }}
              className="rounded-lg px-3 py-1.5 text-[12px] text-text-secondary hover:bg-hover-bg"
            >
              取消
            </button>
            <button
              type="button"
              onClick={submitEdit}
              disabled={busy || !draft.trim()}
              className="rounded-lg bg-primary px-3 py-1.5 text-[12px] font-medium text-white hover:opacity-90 disabled:opacity-50"
            >
              重发
            </button>
          </div>
        </div>
      ) : (
        <>
          <UserAttachments attachments={attachments} />
          {hasVisibleMessage && (
            <div className="bg-sidebar-bg text-text-base px-4 py-3 rounded-2xl rounded-tr-sm text-[15px] max-w-[80%]">
              {office && !message.selectedSkills?.length && !message.mentions?.length ? (
                <OfficeContextBubble office={office} />
              ) : (
                <UserInlineContent content={visibleContent} skills={skills} mentions={mentions} />
              )}
            </div>
          )}
        </>
      )}
      {!editing && (
        <div className="flex text-text-secondary mt-2 space-x-3 text-sm opacity-0 group-hover:opacity-100 transition-opacity w-full justify-end">
          <button
            type="button"
            title="复制"
            aria-label="复制"
            onClick={() => copyText(visibleContent)}
            className="hover:text-text-base"
          >
            <FontAwesomeIcon icon={["far", "copy"]} />
          </button>
          <button
            type="button"
            title="编辑后重发"
            aria-label="编辑后重发"
            onClick={() => setEditing(true)}
            disabled={busy || !visibleContent}
            className="hover:text-text-base disabled:opacity-40"
          >
            <FontAwesomeIcon icon={["fas", "pen"]} />
          </button>
        </div>
      )}
    </div>
  );
}

function OfficeContextBubble({ office }: { office: OfficeContextView }) {
  return (
    <div className="min-w-0">
      <div className="mb-2 rounded-xl bg-sidebar-bg/55 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2 text-[14px] leading-relaxed">
          <span className="inline-flex shrink-0 items-center rounded-md bg-primary/10 px-2 py-1 text-[13px] font-semibold text-primary">
            <FontAwesomeIcon icon={["far", "file-lines"]} className="mr-1.5 text-[12px]" />
            {office.type}
          </span>
          {office.title && (
            <span className="font-medium text-text-base">
              {office.title}
            </span>
          )}
          {office.body && (
            <span className="whitespace-pre-wrap break-words text-text-base">
              {office.body}
            </span>
          )}
        </div>

        {office.meta.length > 0 && (
          <div className="mt-2 flex flex-wrap gap-1.5">
            {office.meta.map((item) => (
              <span
                key={`${item.label}-${item.value}`}
                className="inline-flex max-w-full items-center rounded-md bg-sidebar-bg/80 px-2 py-0.5 text-[12px] text-text-secondary"
              >
                <span className="mr-1 shrink-0">{item.label}</span>
                <span className="truncate text-text-base">{item.value}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      {office.prompt && (
        <div className="border-t border-border-theme/70 pt-2 text-[14px] leading-relaxed text-text-base">
          {office.prompt}
        </div>
      )}
    </div>
  );
}

function normalizeBrowserUrl(input: string): string {
  const trimmed = input.trim();
  if (!trimmed) return "";
  if (/^https?:\/\//i.test(trimmed)) return trimmed;
  if (/^(localhost|127\.0\.0\.1)(:\d+)?(\/.*)?$/i.test(trimmed)) return `http://${trimmed}`;
  return `https://${trimmed}`;
}

export function ChatView({
  messages,
  onSend,
  onFork,
  onRewind,
  onExport,
  onCopy,
  onRename,
  onOpenInNewWindow,
  onPin,
  onArchive,
  pinned = false,
  title = null,
  timeline = [],
  approval = null,
  approvalQueueCount = 0,
  onApprovalDecision,
  busy = false,
  onStop,
  planMode = false,
  activeProjectPath = null,
  projectMapOpenSignal = 0,
  contextUsage = null,
}: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const contextUsageFallbackTokens = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i -= 1) {
      const usage = messages[i]?.usage;
      if (usage?.promptTokens) return usage.promptTokens;
    }
    return 0;
  }, [messages]);
  const [isGitWorkbenchOpen, setIsGitWorkbenchOpen] = useState(false);
  const {
    loading: gitLoading,
    status: gitStatus,
    changes: gitChangesState,
    refresh: refreshGitStatus,
  } = useGitStatus(activeProjectPath);
  const [isRenameDialogOpen, setIsRenameDialogOpen] = useState(false);
  const [isBottomPanelOpen, setIsBottomPanelOpen] = useState(false);
  const [isRightSidebarOpen, setIsRightSidebarOpen] = useState(false);
  const [envMode, setEnvMode] = useState<"local" | "remote">(() =>
    localStorage.getItem("envMode") === "remote" ? "remote" : "local",
  );
  const [selectedConnectionId, setSelectedConnectionId] = useState<string | null>(() =>
    localStorage.getItem("ssh_connection_id"),
  );
  const [selectedConnection, setSelectedConnection] = useState<PluginConnectionSummary | null>(null);
  const [sidebarTabs, setSidebarTabs] = useState<PluginTab[]>([]);
  const [activeSidebarTabId, setActiveSidebarTabId] = useState<string>("new");
  const [bottomTabs, setBottomTabs] = useState<PluginTab[]>([]);
  const [activeBottomTabId, setActiveBottomTabId] = useState<string>("new");

  const [bottomPanelHeight, setBottomPanelHeight] = useState(280);
  const [isResizingBottom, setIsResizingBottom] = useState(false);
  const bottomPanelPresence = usePanelPresence(isBottomPanelOpen, 260);

  useEffect(() => {
    if (!isResizingBottom) return;
    const handleMouseMove = (e: MouseEvent) => {
      const newHeight = window.innerHeight - e.clientY;
      if (newHeight > 200 && newHeight < window.innerHeight - 100) {
        setBottomPanelHeight(newHeight);
      }
    };
    const handleMouseUp = () => setIsResizingBottom(false);

    document.addEventListener("mousemove", handleMouseMove);
    document.addEventListener("mouseup", handleMouseUp);
    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
    };
  }, [isResizingBottom]);

  const [mapStatus, setMapStatus] = useState<ProjectMapStatus | null>(null);

  const outputItems = useMemo(() => collectOutputItems(messages), [messages]);
  // Agent-driven changeset (`write_file` / `edit_file` / `multi_edit`).
  // Replaces the legacy `git diff --shortstat HEAD` source so the env
  // panel's "+N -M" reflects what THIS conversation actually wrote, even
  // when the workspace isn't a git repo. See `computeChatChanges` for
  // wire-format + undercounting notes.
  const chatChanges = useMemo(() => computeChatChanges(messages), [messages]);
  const gitWorkspaceAdditions = gitChangesState?.additions ?? gitStatus?.additions ?? 0;
  const gitWorkspaceDeletions = gitChangesState?.deletions ?? gitStatus?.deletions ?? 0;
  const gitWorkspaceFilesChanged =
    gitChangesState?.files.length ?? gitStatus?.files_changed ?? 0;
  const hasConversation = messages.length > 0;

  useEffect(() => {
    setIsGitWorkbenchOpen(false);
  }, [activeProjectPath]);

  // Auto-scroll the conversation to the bottom as messages/tokens stream in.
  const scrollRef = useRef<HTMLDivElement>(null);
  const composerFrameRef = useRef<HTMLDivElement>(null);
  const [composerBottomPadding, setComposerBottomPadding] = useState(176);
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, composerBottomPadding]);

  useEffect(() => {
    const frame = composerFrameRef.current;
    if (!frame) return;

    const updatePadding = () => {
      const height = Math.ceil(frame.getBoundingClientRect().height);
      setComposerBottomPadding(Math.max(176, height + 56));
    };

    updatePadding();
    const observer = new ResizeObserver(updatePadding);
    observer.observe(frame);
    return () => observer.disconnect();
  }, []);

  const handleOpenBottomPlugin = (c: PluginToolCard) => {
    const tabOptions =
      c.pluginAppId || c.pluginId
        ? { id: c.id ? `${c.id}-${Date.now()}` : undefined, title: c.title }
        : undefined;
    const newTab = createPluginTab(c.type, {
      activeProjectPath,
      envMode,
      selectedConnection,
      t,
    }, tabOptions);
    setBottomTabs([...bottomTabs, newTab]);
    setActiveBottomTabId(newTab.id);
  };

  const handleToggleBottomTerminalPanel = () => {
    if (isBottomPanelOpen) {
      setIsBottomPanelOpen(false);
    } else {
      setIsBottomPanelOpen(true);
      if (!bottomTabs.some((tab) => tab.type === "terminal")) {
        const terminalCard = PLUGIN_TOOL_CARDS.find((card) => card.type === "terminal");
        if (terminalCard) handleOpenBottomPlugin(terminalCard);
      } else {
        const terminalTab = bottomTabs.find((tab) => tab.type === "terminal");
        if (terminalTab) setActiveBottomTabId(terminalTab.id);
      }
    }
  };

  const handleOpenSidebarPlugin = (c: PluginToolCard) => {
    const tabOptions =
      c.pluginAppId || c.pluginId
        ? { id: c.id ? `${c.id}-${Date.now()}` : undefined, title: c.title }
        : undefined;
    const newTab = createPluginTab(c.type, {
      activeProjectPath,
      envMode,
      selectedConnection,
      t,
    }, tabOptions);
    setSidebarTabs((tabs) => [...tabs, newTab]);
    setActiveSidebarTabId(newTab.id);
  };

  const closeSidebarTab = (tabId: string) => {
    setSidebarTabs((tabs) => {
      const newTabs = tabs.filter((tab) => tab.id !== tabId);
      setActiveSidebarTabId((current) =>
        current === tabId ? (newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new") : current,
      );
      if (newTabs.length === 0) setIsRightSidebarOpen(false);
      return newTabs;
    });
  };

  const openProjectMapSidebar = useCallback(() => {
    setIsRightSidebarOpen(true);
    setSidebarTabs((tabs) => {
      const existingTab = tabs.find((tab) => tab.type === "project_map");
      setActiveSidebarTabId(existingTab?.id ?? PROJECT_MAP_TAB_ID);
      if (existingTab) return tabs;
      return [
        ...tabs,
        {
          id: PROJECT_MAP_TAB_ID,
          type: "project_map",
          title: t("chatView.tools.project_map", { defaultValue: "项目地图" }),
          icon: ["fas", "share-nodes"],
        },
      ];
    });
  }, [t]);

  useEffect(() => {
    if (projectMapOpenSignal > 0) openProjectMapSidebar();
  }, [openProjectMapSidebar, projectMapOpenSignal]);

  useEffect(() => {
    const onOpen = () => openProjectMapSidebar();
    window.addEventListener(PROJECT_MAP_OPEN_EVENT, onOpen);
    return () => window.removeEventListener(PROJECT_MAP_OPEN_EVENT, onOpen);
  }, [openProjectMapSidebar]);

  // Office-agent panels (file preview / recording) inject messages into the
  // active chat via this event - routed through the normal send path so the
  // turn respects the model's context budget and approval flow.
  useEffect(() => {
    const onSendToChat = (e: Event) => {
      const text = (e as CustomEvent<string>).detail;
      if (typeof text === "string" && text.trim()) onSend(text);
    };
    window.addEventListener(SEND_TO_CHAT_EVENT, onSendToChat);
    return () => window.removeEventListener(SEND_TO_CHAT_EVENT, onSendToChat);
  }, [onSend]);

  useEffect(() => {
    let cancelled = false;
    setMapStatus(null);
    if (!activeProjectPath) return;
    projectMapStatus(activeProjectPath)
      .then((next) => {
        if (!cancelled) setMapStatus(next);
      })
      .catch(() => {
        if (!cancelled) setMapStatus(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activeProjectPath]);

  const openUrlInSidebarBrowser = useCallback((rawUrl: string) => {
    const url = normalizeBrowserUrl(rawUrl);
    if (!url) return;
    window.open(url, "_blank", "noopener,noreferrer");
  }, []);

  const handleResend = useCallback(
    (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => {
      if (busy) return;
      onSend(text, [], skills, mentions);
    },
    [busy, onSend],
  );

  const submit = (
    attachments: ComposerAttachment[] = [],
    selectedSkills: ComposerSkillSelection[] = [],
    mentions: ComposerMention[] = [],
    displayText?: string,
  ) => {
    const t = value.trim();
    if (!t && attachments.length === 0 && selectedSkills.length === 0 && mentions.length === 0) return;
    onSend(t, attachments, selectedSkills, mentions, displayText);
    setValue("");
  };

  const handleRename = () => {
    setIsRenameDialogOpen(true);
  };

  const handleOpenAutomation = () => {
    window.dispatchEvent(new CustomEvent(OPEN_AUTOMATION_EVENT));
  };

  const isUserRewindEntry = useCallback(
    (entry: TimelineEntry) =>
      entry.kind === "message" &&
      entry.label.toLowerCase().includes("user") &&
      typeof entry.detail === "string" &&
      entry.detail.trim().length > 0,
    [],
  );

  const rewindEntries = useMemo(
    () => timeline.filter((entry) => isUserRewindEntry(entry)),
    [isUserRewindEntry, timeline],
  );

  const formatRewindTimestamp = useCallback((timestamp: number) => {
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) return "";
    return new Intl.DateTimeFormat("zh-CN", {
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: false,
    }).format(date);
  }, []);
  const activeBottomTab =
    bottomTabs.find((tab) => tab.id === activeBottomTabId) ?? null;

  return (
    <div className="w-full h-full min-w-0 overflow-hidden flex flex-col relative">
      <RenameSessionDialog
        open={isRenameDialogOpen}
        initialValue={title || t("chatView.chat")}
        onClose={() => setIsRenameDialogOpen(false)}
        onConfirm={async (nextTitle) => {
          await onRename?.(nextTitle);
          setIsRenameDialogOpen(false);
        }}
      />
      {/* Global Window Actions: fixed position in all states. */}
      <div className="absolute top-0.5 right-6 z-50 flex items-center gap-3 text-text-secondary pointer-events-auto">
        <button
          type="button"
          onClick={() => setIsRightSidebarOpen((v) => !v)}
          className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors ${
            isRightSidebarOpen ? "text-text-base" : "text-text-secondary hover:bg-hover-bg hover:text-text-base"
          }`}
          title={isRightSidebarOpen ? "收起侧栏" : "打开右侧栏"}
          aria-label={isRightSidebarOpen ? "收起侧栏" : "打开右侧栏"}
        >
          <SidebarRightIcon className="text-[15px]" />
        </button>
        <button
          type="button"
          onClick={handleToggleBottomTerminalPanel}
          className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors ${
            isBottomPanelOpen ? "text-text-base" : "text-text-secondary hover:bg-hover-bg hover:text-text-base"
          }`}
          title="打开底部终端"
          aria-label="打开底部终端"
        >
          <BottomPanelIcon className="text-[15px]" />
        </button>
      </div>

      {/* Top half: conversation flow & overlay */}
      <div className="relative flex flex-1 min-h-0 min-w-0 w-full overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="relative h-8 flex items-center pl-6 pr-6 justify-between flex-shrink-0 w-full">
          <div className="relative">
            <DropdownMenuTrigger>
              <AriaButton
                className="flex items-center text-sm font-medium text-text-base cursor-pointer px-2 py-1 -ml-2 rounded outline-none transition-colors hover:bg-hover-bg data-[pressed]:bg-hover-bg"
              >
                {title?.trim() || t("chatView.chat")}
                <FontAwesomeIcon icon={["fas", "ellipsis"]} className="ml-2 text-text-secondary" />
              </AriaButton>

              <DropdownMenu aria-label={t("chatView.chat")}>
                <DropdownMenuItem onAction={() => onPin?.()}>
                  <FontAwesomeIcon icon={["fas", "thumbtack"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{pinned ? t("sidebar.unpin") : t("chatView.pinChat")}</span>
                  <DropdownMenuShortcut>Ctrl+Alt+P</DropdownMenuShortcut>
                </DropdownMenuItem>
                <DropdownMenuItem onAction={handleRename}>
                  <FontAwesomeIcon icon={["fas", "pen"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.renameChat")}</span>
                  <DropdownMenuShortcut>Ctrl+Alt+R</DropdownMenuShortcut>
                </DropdownMenuItem>
                <DropdownMenuItem onAction={() => onArchive?.()}>
                  <FontAwesomeIcon icon={["fas", "box-archive"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.archiveChat")}</span>
                  <DropdownMenuShortcut>Ctrl+Shift+A</DropdownMenuShortcut>
                </DropdownMenuItem>

                <DropdownMenuSeparator />

                <DropdownMenuItem onAction={() => onCopy?.()}>
                  <FontAwesomeIcon icon={["far", "copy"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.copy")}</span>
                </DropdownMenuItem>
                <DropdownMenuItem onAction={() => onExport?.("json")}>
                  <FontAwesomeIcon icon={["fas", "file-export"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.exportJson")}</span>
                </DropdownMenuItem>
                <DropdownMenuItem onAction={() => onFork?.()}>
                  <FontAwesomeIcon icon={["fas", "code-branch"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.branch")}</span>
                </DropdownMenuItem>

                <DropdownMenuSub>
                  <DropdownMenuItem>
                    <FontAwesomeIcon icon={["fas", "clock-rotate-left"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                    <span>{t("chatView.rewind")}</span>
                    <FontAwesomeIcon icon={["fas", "chevron-right"]} className="ml-auto text-[10px] text-gray-400" />
                  </DropdownMenuItem>
                  <DropdownMenuSubContent aria-label={t("chatView.rewind")} className="max-h-72">
                    {rewindEntries.length === 0 ? (
                      <DropdownMenuItem isDisabled className="text-[12px] text-text-secondary">
                        {t("chatView.noRewindPoints")}
                      </DropdownMenuItem>
                    ) : (
                      rewindEntries.map((entry) => (
                        <DropdownMenuItem
                          key={entry.sequence}
                          className="!items-start !py-2.5 text-[12px]"
                          textValue={entry.detail ?? `#${entry.sequence}`}
                          onAction={() => {
                            // Claude Code rewind semantics: rewinding to a user
                            // message returns the session to the state BEFORE it
                            // was sent, and puts the prompt back in the composer
                            // for editing/resend (manual acceptance M-15).
                            onRewind?.(Math.max(0, entry.sequence - 1));
                            if (typeof entry.detail === "string" && entry.detail.trim()) {
                              setValue(entry.detail);
                            }
                          }}
                        >
                          <span className="text-gray-400 tabular-nums shrink-0 mr-2">#{entry.sequence}</span>
                          <div className="min-w-0 flex-1">
                            <div className="truncate text-[13px] text-text-base">{entry.detail}</div>
                            <div className="mt-0.5 text-[11px] text-text-secondary">
                              {formatRewindTimestamp(entry.timestamp)}
                            </div>
                          </div>
                        </DropdownMenuItem>
                      ))
                    )}
                  </DropdownMenuSubContent>
                </DropdownMenuSub>

                <DropdownMenuSeparator />

                <DropdownMenuItem onAction={handleOpenAutomation}>
                  <FontAwesomeIcon icon={["far", "clock"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.addAutomation")}</span>
                </DropdownMenuItem>

                <DropdownMenuSeparator />

                <DropdownMenuItem onAction={() => onOpenInNewWindow?.()}>
                  <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="w-4 mr-2.5 text-gray-500 group-data-[focused]/menu-item:text-text-base" />
                  <span>{t("chatView.openInNewWindow")}</span>
                </DropdownMenuItem>
              </DropdownMenu>
            </DropdownMenuTrigger>
          </div>
          <div className="flex items-center text-text-secondary">
            <div
              className="absolute top-1/2 z-10 flex -translate-y-1/2 items-center gap-3 text-text-secondary transition-[right] duration-300"
              style={{ right: isRightSidebarOpen ? 16 : 104 }}
            >
              {activeProjectPath && (
                <ProjectMapStatusBadge status={mapStatus} onClick={openProjectMapSidebar} />
              )}
              {hasConversation && (
                <EnvironmentInfoMenu
                  activeProjectPath={activeProjectPath}
                  gitStatus={gitStatus}
                  gitLoading={gitLoading}
                  gitWorkspaceAdditions={gitWorkspaceAdditions}
                  gitWorkspaceDeletions={gitWorkspaceDeletions}
                  gitWorkspaceFilesChanged={gitWorkspaceFilesChanged}
                  chatChanges={chatChanges}
                  outputItems={outputItems}
                  onOpenGitWorkbench={() => setIsGitWorkbenchOpen(true)}
                  onOpenUrl={openUrlInSidebarBrowser}
                  onEnvironmentChange={(mode, connectionId, connection) => {
                    setEnvMode(mode);
                    setSelectedConnectionId(connectionId);
                    setSelectedConnection(connection ?? null);
                  }}
                >
                <button
                  type="button"
                  className="flex h-7 w-7 items-center justify-center rounded-md text-text-secondary transition-colors hover:bg-hover-bg hover:text-text-base data-[state=open]:text-text-base"
                  title={t("chatView.environmentInfo")}
                  aria-label={t("chatView.environmentInfo")}
                >
                  <FontAwesomeIcon icon={["fas", "sliders"]} className="text-[15px]" />
                </button>
                </EnvironmentInfoMenu>
              )}
            </div>
          </div>
        </header>
          <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
            <div className="flex-1 flex flex-col relative min-w-0 min-h-0">
          

        <div
          ref={scrollRef}
          className="stable-scrollbar-gutter flex-1 min-h-0 overflow-y-auto px-6 py-4"
          style={{ paddingBottom: composerBottomPadding }}
        >
          {messages.length === 0 && (
            <div className="w-full max-w-4xl mx-auto text-text-secondary text-[15px] pl-2">
              {t("chatView.startConversation")}
            </div>
          )}
          <ChatTimeline
            messages={messages}
            busy={busy}
            onOpenUrl={openUrlInSidebarBrowser}
            onResend={handleResend}
            scrollContainerRef={scrollRef}
          />
        </div>

        <div className="absolute bottom-6 left-0 w-full px-6 flex justify-center">
          <div ref={composerFrameRef} className="w-full max-w-4xl relative">
            {approval && (
              <div
                className="absolute left-0 right-0 z-30"
                style={{ bottom: "calc(100% - 10px)" }}
              >
                <ApprovalDialog
                  request={approval}
                  queueCount={approvalQueueCount}
                  onApprove={(req) => onApprovalDecision?.(req, true)}
                  onReject={(req) => onApprovalDecision?.(req, false)}
                />
              </div>
            )}
            <Composer
              value={value}
              onChange={setValue}
              onSubmit={submit}
              placeholder={t("chatView.requestFollowUp")}
              busy={busy}
              onStop={onStop}
              planMode={planMode}
              activeProjectPath={activeProjectPath}
              contextUsage={contextUsage}
              contextUsageFallbackTokens={contextUsageFallbackTokens}
              textareaMaxHeight={300}
            />
          </div>
        </div>

            </div>
            
          </div>
        </div>
          <RightSidebarWorkbench
            open={isRightSidebarOpen}
            tabs={sidebarTabs}
            activeTabId={activeSidebarTabId}
            onSelectTab={setActiveSidebarTabId}
              onCloseTab={closeSidebarTab}
              onShowLauncher={() => setActiveSidebarTabId("new")}
              onSelectPlugin={handleOpenSidebarPlugin}
              renderContext={{ activeProjectPath, envMode, selectedConnectionId, onProjectMapStatusChange: setMapStatus }}
            />
      </div>

      {bottomPanelPresence.shouldRender && (
            <div
            className={`bottom-panel-workbench relative z-0 flex w-full min-w-0 flex-shrink-0 flex-col overflow-hidden border-t border-border-theme bg-bg-base ${
                bottomPanelPresence.isClosing ? "is-closing" : ""
              } ${isResizingBottom ? "is-resizing" : ""}`}
              style={{
                height: bottomPanelPresence.isVisible ? `${bottomPanelHeight}px` : "0px",
                minHeight: bottomPanelPresence.isVisible ? "200px" : "0px",
                maxHeight: '80vh',
                width: '100%',
              }}
            >
              <div
                className={`panel-resize-handle-row ${isResizingBottom ? "is-active" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setIsResizingBottom(true);
                }}
              />
              <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-bg-base">
                <div className="flex h-full min-w-0 flex-1 items-center overflow-x-auto text-[13px] text-text-secondary no-scrollbar">
                  {bottomTabs.map(tab => (
                    <div
                      key={tab.id}
                      onClick={() => setActiveBottomTabId(tab.id)}
                      className={`flex h-full max-w-[220px] flex-shrink-0 cursor-pointer items-center border-b-2 px-3 ${
                        activeBottomTabId === tab.id 
                          ? "border-text-base text-text-base" 
                          : "border-transparent hover:text-text-base"
                      }`}
                    >
                      <FontAwesomeIcon icon={tab.icon} className="mr-2 flex-shrink-0" />
                      <span className="min-w-0 truncate">{tab.title}</span>
                      <FontAwesomeIcon 
                        icon={["fas", "xmark"]} 
                        className="ml-3 flex-shrink-0 hover:text-red-500 text-[10px]"
                        onClick={(e) => {
                           e.stopPropagation();
                           const newTabs = bottomTabs.filter(t => t.id !== tab.id);
                           setBottomTabs(newTabs);
                           if (activeBottomTabId === tab.id) {
                             setActiveBottomTabId(newTabs.length > 0 ? newTabs[newTabs.length - 1].id : "new");
                           }
                        }}
                      />
                    </div>
                  ))}
                  <div
                    className={`flex h-full flex-shrink-0 cursor-pointer items-center px-3 ${activeBottomTabId === "new" ? "text-text-base" : "hover:text-text-base"}`}
                    onClick={() => setActiveBottomTabId("new")}
                  >
                    <FontAwesomeIcon icon={["fas", "plus"]} />
                  </div>
                </div>
                <div className="ml-3 flex flex-shrink-0 items-center space-x-3 text-text-secondary">
                  {activeBottomTab?.type === "files" && (
                    <>
                      <FontAwesomeIcon icon={["fas", "ellipsis"]} className="cursor-pointer hover:text-text-base" />
                      <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="cursor-pointer hover:text-text-base text-[13px]" />
                      <FontAwesomeIcon icon={["far", "copy"]} className="cursor-pointer hover:text-text-base" />
                    </>
                  )}
                  <FontAwesomeIcon icon={["fas", "xmark"]} className="cursor-pointer hover:text-text-base ml-2" onClick={() => setIsBottomPanelOpen(false)} />
                </div>
              </div>

              <div className="flex-1 overflow-hidden flex flex-col relative">
                {activeBottomTabId === "new" && (
                  <ToolLauncherPanel cards={PLUGIN_TOOL_CARDS} onSelect={handleOpenBottomPlugin} variant="bottom" />
                )}

                {activeBottomTabId !== "new" && activeBottomTab
                  ? renderPluginTab(activeBottomTab, {
                      activeProjectPath,
                      envMode,
                      selectedConnectionId,
                      onProjectMapStatusChange: setMapStatus,
                    })
                  : null}
              </div>
            </div>
      )}
        {isGitWorkbenchOpen && activeProjectPath && (
          <div
            className="absolute left-1/2 top-1/2 z-20 h-[min(620px,calc(100vh-96px))] w-[min(1040px,calc(100vw-48px))] -translate-x-1/2 -translate-y-1/2 overflow-hidden rounded-2xl border border-border-theme bg-bg-base shadow-[0_18px_46px_rgb(0,0,0,0.14)]"
          >
            <GitWorkbench
              projectPath={activeProjectPath}
              status={gitStatus}
              changes={gitChangesState}
              loading={gitLoading}
              onRefresh={refreshGitStatus}
              onClose={() => setIsGitWorkbenchOpen(false)}
            />
          </div>
        )}
    </div>
  );
}
