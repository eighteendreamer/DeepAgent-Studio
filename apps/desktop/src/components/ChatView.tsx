import { useState, useEffect, useMemo, useRef, useCallback } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { ChatMessage, MessagePart, ToolCall, TokenUsage, TimelineEntry, ApprovalRequest, ProjectMapStatus } from "../types";
import { Composer } from "./Composer";
import { ToolCallCard } from "./ToolCallCard";
import { ApprovalDialog } from "./ApprovalDialog";
import { RenameSessionDialog } from "./RenameSessionDialog";
import { MarkdownText } from "./MarkdownText";
import { EnvironmentInfoPanel } from "./EnvironmentInfoPanel";
import type { OutputItem } from "./EnvironmentInfoPanel";
import { ProjectMapStatusBadge } from "./project-map/ProjectMapPanel";
import { ToolLauncherPanel } from "./ToolLauncherPanel";
import { BottomPanelIcon, SidebarRightIcon } from "./icons";
import { message as toast } from "./message";
import { useTranslation } from "react-i18next";
import {
  getSessionUiPrefs,
  OPEN_AUTOMATION_EVENT,
  projectMapRefreshDeep,
  projectMapStatus,
  SEND_TO_CHAT_EVENT,
  setSessionEnvPanelAutoOpen,
} from "../api";
import { useGitStatus } from "../hooks/useGitStatus";
import { GitWorkbench } from "./git/GitWorkbench";
import {
  createPluginTab,
  PLUGIN_TOOL_CARDS,
  renderPluginTab,
  type PluginTab,
  type PluginToolCard,
} from "./plugins/pluginRegistry";
import { RightSidebarWorkbench } from "./RightSidebarWorkbench";

const PROJECT_MAP_OPEN_EVENT = "deepagent:open-project-map";
const PROJECT_MAP_TAB_ID = "project-map";

interface Props {
  /** Active session id for per-session UI persistence. */
  sessionId?: string | null;
  /** Active chat identity, including pending runs before a real session id exists. */
  sessionKey?: string | null;
  messages: ChatMessage[];
  onSend: (text: string) => void;
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
function UserTurn({
  message,
  busy,
  onResend,
}: {
  message: ChatMessage;
  busy: boolean;
  onResend: (text: string) => void;
}) {
  const office = parseOfficeContextMessage(message.content);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(message.content);

  const submitEdit = () => {
    const text = draft.trim();
    if (!text || busy) return;
    setEditing(false);
    onResend(text);
  };

  return (
    <div className="flex flex-col items-end mb-8 w-full max-w-4xl mx-auto group">
      {editing ? (
        <div className="w-full max-w-[80%] rounded-2xl rounded-tr-sm bg-gray-100 p-3">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            className="min-h-[120px] w-full resize-y rounded-xl border border-border-theme bg-white px-3 py-2 text-[14px] leading-relaxed text-text-base outline-none focus:border-primary/60"
            autoFocus
          />
          <div className="mt-2 flex justify-end gap-2">
            <button
              type="button"
              onClick={() => {
                setDraft(message.content);
                setEditing(false);
              }}
              className="rounded-lg px-3 py-1.5 text-[12px] text-text-secondary hover:bg-white"
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
        <div className="bg-gray-100 text-text-base px-4 py-3 rounded-2xl rounded-tr-sm text-[15px] max-w-[80%]">
          {office ? <OfficeContextBubble office={office} /> : message.content}
        </div>
      )}
      {!editing && (
        <div className="flex text-text-secondary mt-2 space-x-3 text-sm opacity-0 group-hover:opacity-100 transition-opacity w-full justify-end">
          <button
            type="button"
            title="复制"
            aria-label="复制"
            onClick={() => copyText(message.content)}
            className="hover:text-text-base"
          >
            <FontAwesomeIcon icon={["far", "copy"]} />
          </button>
          <button
            type="button"
            title="编辑后重发"
            aria-label="编辑后重发"
            onClick={() => setEditing(true)}
            disabled={busy}
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
      <div className="mb-2 rounded-xl bg-white/55 px-3 py-2">
        <div className="flex flex-wrap items-center gap-2 text-[14px] leading-relaxed">
          <span className="inline-flex shrink-0 items-center rounded-md bg-blue-50 px-2 py-1 text-[13px] font-semibold text-primary">
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
                className="inline-flex max-w-full items-center rounded-md bg-white/80 px-2 py-0.5 text-[12px] text-text-secondary"
              >
                <span className="mr-1 shrink-0">{item.label}</span>
                <span className="truncate text-text-base">{item.value}</span>
              </span>
            ))}
          </div>
        )}
      </div>

      {office.prompt && (
        <div className="border-t border-white/70 pt-2 text-[14px] leading-relaxed text-text-base">
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
  sessionId = null,
  sessionKey = null,
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
}: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [isOutputPanelOpen, setIsOutputPanelOpen] = useState(false);
  const [envPanelAutoOpenEnabled, setEnvPanelAutoOpenEnabled] = useState(true);
  const [envPanelPrefsLoaded, setEnvPanelPrefsLoaded] = useState(true);
  const [isGitWorkbenchOpen, setIsGitWorkbenchOpen] = useState(false);
  const {
    loading: gitLoading,
    status: gitStatus,
    changes: gitChangesState,
    refresh: refreshGitStatus,
  } = useGitStatus(activeProjectPath);
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const [isRenameDialogOpen, setIsRenameDialogOpen] = useState(false);
  const chatMenuRef = useRef<HTMLDivElement>(null);
  const [isProjectMapMenuOpen, setIsProjectMapMenuOpen] = useState(false);
  const projectMapMenuRef = useRef<HTMLDivElement>(null);
  const [isBottomPanelOpen, setIsBottomPanelOpen] = useState(false);
  const [isRightSidebarOpen, setIsRightSidebarOpen] = useState(false);
  const [sidebarTabs, setSidebarTabs] = useState<PluginTab[]>([]);
  const [activeSidebarTabId, setActiveSidebarTabId] = useState<string>("new");
  const [isRewindOpen, setIsRewindOpen] = useState(false);
  const rewindCloseTimerRef = useRef<number | null>(null);
  const [bottomTabs, setBottomTabs] = useState<PluginTab[]>([]);
  const [activeBottomTabId, setActiveBottomTabId] = useState<string>("new");

  const [bottomPanelHeight, setBottomPanelHeight] = useState(280);
  const [isResizingBottom, setIsResizingBottom] = useState(false);

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

  const activeProjectLabel = useMemo(() => {
    if (!activeProjectPath) return "";
    return activeProjectPath.split(/[\\/]/).filter(Boolean).pop() ?? activeProjectPath;
  }, [activeProjectPath]);
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
  const outputSignature = useMemo(
    () => outputItems.map((item) => `${item.kind}:${item.label}`).join("|"),
    [outputItems]
  );
  const lastAutoOpenedOutputRef = useRef<string>("");
  const previousSessionKeyRef = useRef<string | null>(null);
  const tempEnvPanelAutoOpenRef = useRef<Map<string, boolean>>(new Map());
  useEffect(() => {
    let cancelled = false;
    const previousSessionKey = previousSessionKeyRef.current;
    previousSessionKeyRef.current = sessionKey;
    lastAutoOpenedOutputRef.current = "";
    setIsOutputPanelOpen(false);

    if (!sessionKey) {
      setEnvPanelAutoOpenEnabled(true);
      setEnvPanelPrefsLoaded(true);
      return;
    }

    if (
      sessionId &&
      previousSessionKey &&
      previousSessionKey !== sessionKey &&
      tempEnvPanelAutoOpenRef.current.has(previousSessionKey)
    ) {
      const pendingAutoOpen =
        tempEnvPanelAutoOpenRef.current.get(previousSessionKey) ?? true;
      tempEnvPanelAutoOpenRef.current.delete(previousSessionKey);
      setEnvPanelAutoOpenEnabled(pendingAutoOpen);
      setEnvPanelPrefsLoaded(true);
      if (!pendingAutoOpen) {
        void setSessionEnvPanelAutoOpen(sessionId, false).catch((error) => {
          if (cancelled) return;
          console.error("set_session_env_panel_auto_open failed:", error);
        });
      }
      return;
    }

    if (!sessionId) {
      setEnvPanelAutoOpenEnabled(
        tempEnvPanelAutoOpenRef.current.get(sessionKey) ?? true,
      );
      setEnvPanelPrefsLoaded(true);
      return;
    }

    setEnvPanelPrefsLoaded(false);
    getSessionUiPrefs(sessionId)
      .then((prefs) => {
        if (cancelled) return;
        setEnvPanelAutoOpenEnabled(prefs.env_panel_auto_open);
        setEnvPanelPrefsLoaded(true);
      })
      .catch((error) => {
        if (cancelled) return;
        console.error("get_session_ui_prefs failed:", error);
        setEnvPanelAutoOpenEnabled(true);
        setEnvPanelPrefsLoaded(true);
      });

    return () => {
      cancelled = true;
    };
  }, [sessionId, sessionKey]);
  useEffect(() => {
    if (!hasConversation) {
      lastAutoOpenedOutputRef.current = "";
      setIsOutputPanelOpen(false);
      return;
    }
    if (!envPanelPrefsLoaded) return;
    if (!outputSignature) {
      lastAutoOpenedOutputRef.current = "";
      return;
    }
    if (lastAutoOpenedOutputRef.current === outputSignature) return;
    lastAutoOpenedOutputRef.current = outputSignature;
    if (!envPanelAutoOpenEnabled) return;
    setIsOutputPanelOpen(true);
  }, [envPanelAutoOpenEnabled, envPanelPrefsLoaded, hasConversation, outputSignature]);

  const toggleOutputPanel = useCallback(() => {
    setIsOutputPanelOpen((prev) => {
      const next = !prev;
      // Closing -> record the user's intent so future outputs respect it.
      // Opening -> user is engaging again, clear the override.
      if (!next) {
        setEnvPanelAutoOpenEnabled(false);
        if (sessionId) {
          void setSessionEnvPanelAutoOpen(sessionId, false).catch((error) => {
            console.error("set_session_env_panel_auto_open failed:", error);
          });
        } else if (sessionKey) {
          tempEnvPanelAutoOpenRef.current.set(sessionKey, false);
        }
      }
      return next;
    });
  }, [sessionId, sessionKey]);

  useEffect(() => {
    setIsGitWorkbenchOpen(false);
  }, [activeProjectPath]);

  // Auto-scroll the conversation to the bottom as messages/tokens stream in.
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages]);

  const handleOpenBottomPlugin = (c: PluginToolCard) => {
    const newTab = createPluginTab(c.type, {
      activeProjectPath,
      envMode: "local",
      t,
    });
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
    const newTab = createPluginTab(c.type, {
      activeProjectPath,
      envMode: "local",
      t,
    });
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
    if (!isProjectMapMenuOpen) return;
    const onMouseDown = (event: MouseEvent) => {
      if (projectMapMenuRef.current?.contains(event.target as Node)) return;
      setIsProjectMapMenuOpen(false);
    };
    document.addEventListener("mousedown", onMouseDown);
    return () => document.removeEventListener("mousedown", onMouseDown);
  }, [isProjectMapMenuOpen]);

  useEffect(() => {
    if (!isMenuOpen) {
      setIsRewindOpen(false);
      return;
    }
    const onMouseDown = (event: MouseEvent) => {
      if (chatMenuRef.current?.contains(event.target as Node)) return;
      setIsMenuOpen(false);
      setIsRewindOpen(false);
    };
    document.addEventListener("mousedown", onMouseDown);
    return () => document.removeEventListener("mousedown", onMouseDown);
  }, [isMenuOpen]);

  useEffect(() => {
    return () => {
      if (rewindCloseTimerRef.current !== null) {
        window.clearTimeout(rewindCloseTimerRef.current);
      }
    };
  }, []);

  const refreshProjectMap = async () => {
    if (!activeProjectPath) return;
    setMapStatus((current) => current ? { ...current, status: "updating" } : current);
    try {
      const result = await projectMapRefreshDeep(activeProjectPath);
      setMapStatus(result.status);
      openProjectMapSidebar();
    } catch {
      const next = await projectMapStatus(activeProjectPath).catch(() => null);
      setMapStatus(next);
    }
  };

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

  const openUrlInSidebarBrowser = (rawUrl: string) => {
    const url = normalizeBrowserUrl(rawUrl);
    if (!url) return;
    window.open(url, "_blank", "noopener,noreferrer");
  };

  const submit = () => {
    const t = value.trim();
    if (!t) return;
    onSend(t);
    setValue("");
  };

  const handleRename = () => {
    setIsRenameDialogOpen(true);
    setIsMenuOpen(false);
  };

  const handleOpenAutomation = () => {
    window.dispatchEvent(new CustomEvent(OPEN_AUTOMATION_EVENT));
    setIsMenuOpen(false);
  };

  const openRewindMenu = () => {
    if (rewindCloseTimerRef.current !== null) {
      window.clearTimeout(rewindCloseTimerRef.current);
      rewindCloseTimerRef.current = null;
    }
    setIsRewindOpen(true);
  };

  const scheduleCloseRewindMenu = () => {
    if (rewindCloseTimerRef.current !== null) {
      window.clearTimeout(rewindCloseTimerRef.current);
    }
    rewindCloseTimerRef.current = window.setTimeout(() => {
      setIsRewindOpen(false);
      rewindCloseTimerRef.current = null;
    }, 180);
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
            isRightSidebarOpen ? "text-text-base" : "text-text-secondary hover:bg-gray-100 hover:text-text-base"
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
            isBottomPanelOpen ? "text-text-base" : "text-text-secondary hover:bg-gray-100 hover:text-text-base"
          }`}
          title="打开底部终端"
          aria-label="打开底部终端"
        >
          <BottomPanelIcon className="text-[15px]" />
        </button>
      </div>

      {/* Top half: conversation flow & overlay */}
      <div className="flex flex-1 min-h-0 min-w-0 w-full overflow-hidden">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <header className="relative h-8 flex items-center pl-6 pr-6 justify-between flex-shrink-0 w-full">
          <div className="relative" ref={chatMenuRef}>
            <div
              className="flex items-center text-sm font-medium text-text-base cursor-pointer px-2 py-1 -ml-2 rounded hover:bg-gray-100 transition-colors"
              onClick={() => setIsMenuOpen(!isMenuOpen)}
            >
              {title?.trim() || t("chatView.chat")}
              <FontAwesomeIcon
                icon={["fas", "ellipsis"]}
                className="ml-2 text-text-secondary"
              />
            </div>
            
            {/* Dropdown Menu */}
            {isMenuOpen && (
              <div className="absolute top-10 left-0 w-60 bg-white border border-border-theme rounded-xl shadow-lg py-1.5 z-50 text-[13px] text-text-base font-normal">
                <div
                  className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onPin?.();
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "thumbtack"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{pinned ? t("sidebar.unpin") : t("chatView.pinChat")}</span>
                  </div>
                  <span className="text-gray-400 text-[11px] font-sans">Ctrl+Alt+P</span>
                </div>
                <div
                  className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={handleRename}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "pen"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.renameChat")}</span>
                  </div>
                  <span className="text-gray-400 text-[11px] font-sans">Ctrl+Alt+R</span>
                </div>
                <div
                  className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onArchive?.();
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "box-archive"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.archiveChat")}</span>
                  </div>
                  <span className="text-gray-400 text-[11px] font-sans">Ctrl+Shift+A</span>
                </div>
                
                <div className="w-full h-px bg-border-theme my-1.5"></div>
                
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onCopy?.();
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["far", "copy"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.copy")}</span>
                  </div>
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onExport?.("json");
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "file-export"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.exportJson")}</span>
                  </div>
                </div>
                <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onFork?.();
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "code-branch"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.branch")}</span>
                  </div>
                </div>
                <div
                  className="relative"
                  onMouseEnter={openRewindMenu}
                  onMouseLeave={scheduleCloseRewindMenu}
                >
                  <div className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                    onMouseEnter={openRewindMenu}
                    onClick={() => setIsRewindOpen((v) => !v)}
                  >
                    <div className="flex items-center">
                      <FontAwesomeIcon icon={["fas", "clock-rotate-left"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                      <span>{t("chatView.rewind")}</span>
                    </div>
                    <FontAwesomeIcon icon={["fas", "chevron-right"]} className="text-[10px] text-gray-400" />
                  </div>
                  {isRewindOpen && (
                    <div
                      className="absolute left-full top-0 ml-1 w-72 max-h-72 overflow-y-auto bg-white border border-border-theme rounded-xl shadow-lg py-1.5 z-50 custom-scrollbar"
                      onMouseEnter={openRewindMenu}
                      onMouseLeave={scheduleCloseRewindMenu}
                    >
                      {rewindEntries.length === 0 && (
                        <div className="px-4 py-2 text-[12px] text-text-secondary">{t("chatView.noRewindPoints")}</div>
                      )}
                      {rewindEntries.map((entry) => (
                        <div
                          key={entry.sequence}
                          className="px-4 py-2.5 hover:bg-gray-100 cursor-pointer text-[12px] text-text-base"
                          onClick={() => {
                            onRewind?.(entry.sequence);
                            setIsRewindOpen(false);
                            setIsMenuOpen(false);
                          }}
                          title={entry.detail ?? undefined}
                        >
                          <div className="flex items-start gap-2">
                            <span className="text-gray-400 tabular-nums shrink-0">#{entry.sequence}</span>
                            <div className="min-w-0 flex-1">
                              <div className="truncate text-[13px] text-text-base">
                                {entry.detail}
                              </div>
                              <div className="mt-0.5 text-[11px] text-text-secondary">
                                {formatRewindTimestamp(entry.timestamp)}
                              </div>
                            </div>
                          </div>
                        </div>
                      ))}
                    </div>
                  )}
                </div>

                <div className="w-full h-px bg-border-theme my-1.5"></div>

                <div
                  className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={handleOpenAutomation}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["far", "clock"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.addAutomation")}</span>
                  </div>
                </div>

                <div className="w-full h-px bg-border-theme my-1.5"></div>

                <div
                  className="flex items-center px-4 py-2 hover:bg-gray-100 cursor-pointer justify-between group"
                  onClick={() => {
                    onOpenInNewWindow?.();
                    setIsMenuOpen(false);
                  }}
                >
                  <div className="flex items-center">
                    <FontAwesomeIcon icon={["fas", "arrow-up-right-from-square"]} className="w-4 mr-2.5 text-gray-500 group-hover:text-text-base" />
                    <span>{t("chatView.openInNewWindow")}</span>
                  </div>
                </div>
              </div>
            )}
          </div>
          <div className="flex items-center text-text-secondary">
            <div
              className="absolute top-1/2 z-10 flex -translate-y-1/2 items-center gap-3 text-text-secondary transition-[right] duration-300"
              style={{ right: 104 }}
            >
              {activeProjectPath && (
                <div className="relative" ref={projectMapMenuRef}>
                <button
                  type="button"
                  className="h-7 max-w-[220px] flex items-center rounded-md px-2 text-[12px] text-text-secondary hover:bg-gray-100 hover:text-text-base transition-colors"
                  title={activeProjectPath}
                  onClick={() => setIsProjectMapMenuOpen((v) => !v)}
                >
                  <FontAwesomeIcon icon={["far", "folder"]} className="mr-1.5 text-[11px]" />
                  <span className="truncate">{activeProjectLabel}</span>
                  <FontAwesomeIcon icon={["fas", "chevron-down"]} className="ml-1.5 text-[9px]" />
                </button>
                {isProjectMapMenuOpen && (
                  <div className="absolute top-full right-0 mt-1 w-48 rounded-xl border border-border-theme bg-white py-1 shadow-[0_4px_24px_rgb(0,0,0,0.12)] z-50">
                    <button
                      type="button"
                      className="w-full flex items-center px-3 py-2 text-left text-[13px] text-text-base hover:bg-gray-50"
                      onClick={() => {
                        setIsProjectMapMenuOpen(false);
                        refreshProjectMap();
                      }}
                    >
                      <FontAwesomeIcon icon={["fas", "rotate-right"]} className="text-text-secondary mr-2.5 w-4" />
                      重新生成项目地图
                    </button>
                  </div>
                )}
              </div>
            )}
            {activeProjectPath && (
              <ProjectMapStatusBadge status={mapStatus} onClick={() => setIsProjectMapMenuOpen((v) => !v)} />
            )}
              {hasConversation && (
                <button
                  type="button"
                  className={`flex h-7 w-7 items-center justify-center rounded-md transition-colors ${
                    isOutputPanelOpen ? "text-text-base" : "text-text-secondary hover:bg-gray-100 hover:text-text-base"
                  }`}
                  onClick={toggleOutputPanel}
                  title="环境信息"
                  aria-label="环境信息"
                >
                  <FontAwesomeIcon icon={["fas", "sliders"]} className="text-[15px]" />
                </button>
              )}
            </div>
          </div>
        </header>
          <div className="flex min-h-0 min-w-0 flex-1 overflow-hidden">
            <div className="flex-1 flex flex-col relative min-w-0 min-h-0">
          

        <div ref={scrollRef} className="stable-scrollbar-gutter flex-1 min-h-0 overflow-y-auto px-6 py-4 pb-44">
          {messages.length === 0 && (
            <div className="w-full max-w-4xl mx-auto text-text-secondary text-[15px] pl-2">
              {t("chatView.startConversation")}
            </div>
          )}
          {messages.map((m, i) =>
            m.role === "user" ? (
              <UserTurn
                key={i}
                message={m}
                busy={busy}
                onResend={(text) => {
                  if (busy) return;
                  onSend(text);
                }}
              />
            ) : (
              <AssistantTurn
                key={i}
                message={m}
                busy={busy && i === messages.length - 1}
                onOpenUrl={openUrlInSidebarBrowser}
              />
            )
          )}
        </div>

        <div className="absolute bottom-6 left-0 w-full px-6 flex justify-center">
          <div className="w-full max-w-4xl relative">
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
            renderContext={{ activeProjectPath, envMode: "local", onProjectMapStatusChange: setMapStatus }}
          />
      </div>

      {isBottomPanelOpen && (
            <div
              className={`relative z-0 flex w-full min-w-0 flex-shrink-0 flex-col overflow-hidden border-t border-border-theme bg-white shadow-[0_-12px_30px_rgba(0,0,0,0.06)] ${isResizingBottom ? "" : "transition-[height] duration-300"}`}
              style={{ height: `${bottomPanelHeight}px`, minHeight: '200px', maxHeight: '80vh', width: '100%' }}
            >
              <div
                className={`panel-resize-handle-row ${isResizingBottom ? "is-active" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  setIsResizingBottom(true);
                }}
              />
              <div className="flex items-center justify-between border-b border-border-theme h-10 px-4 flex-shrink-0 bg-white">
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
                      envMode: "local",
                      onProjectMapStatusChange: setMapStatus,
                    })
                  : null}
              </div>
            </div>
      )}
      {/* Floating sticky note for context state */}
        {isOutputPanelOpen && (
          <EnvironmentInfoPanel
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
          />
        )}
        {isGitWorkbenchOpen && activeProjectPath && (
          <div
            className="absolute left-4 right-4 top-12 bottom-4 z-20 overflow-hidden rounded-2xl border border-border-theme bg-white shadow-[0_18px_46px_rgb(0,0,0,0.14)] md:left-6 md:right-6 md:top-16 md:bottom-6"
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

/**
 * Wraps one assistant turn in a coherent block. A long agent run produces many
 * reasoning/tool "process" steps before the final answer; once the model starts
 * summarizing (final answer text appears) or the run finishes, those process
 * steps collapse into ONE big dropdown so the answer stays prominent. While the
 * run is still working with no answer yet, the steps stay expanded for live
 * progress.
 */
function AssistantTurn({
  message: m,
  busy,
  onOpenUrl,
}: {
  message: ChatMessage;
  busy: boolean;
  onOpenUrl?: (url: string) => void;
}) {
  const { t } = useTranslation();
  const parts = m.parts ?? [];
  const hasLegacyVisibleContent =
    !!m.content?.trim() ||
    !!m.reasoning?.trim() ||
    (m.tools?.length ?? 0) > 0;

  if (!busy && parts.length === 0 && !hasLegacyVisibleContent) {
    return null;
  }

  // Split into process steps (reasoning + tools, plus any interleaved
  // intermediate text) and the final answer (the trailing run of text parts).
  let lastNonText = -1;
  for (let i = 0; i < parts.length; i++) {
    if (parts[i].kind !== "text") lastNonText = i;
  }
  const processParts = lastNonText >= 0 ? parts.slice(0, lastNonText + 1) : [];
  const answerParts = lastNonText >= 0 ? parts.slice(lastNonText + 1) : parts;

  const toolCount = processParts.filter((p) => p.kind === "tool").length;
  const hasProcess = processParts.length > 0;
  const hasAnswer = answerParts.some(
    (p) => p.kind === "text" && p.text.trim().length > 0
  );

  // Total wall-clock spent in tool calls this turn (sum of card durations).
  const totalToolMs = parts.reduce(
    (acc, p) => acc + (p.kind === "tool" ? p.tool.durationMs ?? 0 : 0),
    0
  );

  // Collapse the process once the answer has begun (summary phase) or the run
  // finished; keep it open while actively working with nothing summarized yet.
  const collapseProcess = hasAnswer || !busy;

  const renderPart = (part: MessagePart, pi: number, streamTail: boolean) => {
    if (part.kind === "tool") {
      return <ToolCallCard key={`tool-${part.tool.call_id}`} tool={part.tool} />;
    }
    if (part.kind === "reasoning") {
      return <ReasoningBlock key={`r-${pi}`} text={part.text} defaultOpen={streamTail} />;
    }
    return (
      <MarkdownText
        key={`t-${pi}`}
        text={part.text}
        tone={part.tone}
        className="mb-1"
        onOpenUrl={onOpenUrl}
      />
    );
  };

  return (
    <div className="mb-6 w-full max-w-4xl mx-auto">
      {/* Header: agent label + live status. */}
      <div className="flex items-center mb-2 pl-1">
        <div className="w-5 h-5 rounded-md bg-primary/10 text-primary flex items-center justify-center mr-2">
          <FontAwesomeIcon icon={["fas", "robot"]} className="text-[11px]" />
        </div>
        <span className="text-[12px] font-medium text-text-base">{t("chatView.assistant")}</span>
        {busy && (
          <span className="ml-2 flex items-center text-[11px] text-blue-500">
            <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-1 text-[10px]" />
            {t("chatView.working")}
          </span>
        )}
      </div>

      {parts.length > 0 ? (
        <>
          {/* Process steps: collapsed into one dropdown once summarizing. */}
          {hasProcess &&
            (collapseProcess ? (
              <ProcessSteps toolCount={toolCount} totalMs={totalToolMs}>
                {processParts.map((p, pi) => renderPart(p, pi, false))}
              </ProcessSteps>
            ) : (
              <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3 mb-2">
                {processParts.map((p, pi) =>
                  renderPart(p, pi, busy && pi === processParts.length - 1 && !hasAnswer)
                )}
              </div>
            ))}

          {/* Final answer: always prominent, outside the collapsible. */}
          {hasAnswer && (
            <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3">
              {answerParts.map((p, pi) => renderPart(p, processParts.length + pi, false))}
              {!busy && (
                <UsageFooter usage={m.usage} totalMs={m.runMs ?? totalToolMs} answer={m.content} />
              )}
            </div>
          )}

          {!busy && hasProcess && !hasAnswer && (
            <UsageFooter usage={m.usage} totalMs={m.runMs ?? totalToolMs} answer={m.content} />
          )}

          {/* Working placeholder before any step/answer exists. */}
          {busy && !hasProcess && !hasAnswer && (
            <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3">
              <div className="flex items-center text-text-secondary text-[14px]">
                <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2 text-[13px]" />
                {t("chatView.working")}
              </div>
            </div>
          )}
        </>
      ) : (
        // Legacy layout (replayed sessions without ordered parts).
        <div className="w-full rounded-xl border border-border-theme bg-white px-4 py-3">
          {m.tools && m.tools.length > 0 && (
            <ProcessSteps toolCount={m.tools.length} totalMs={0}>
              {m.tools.map((tool) => (
                <ToolCallCard key={tool.call_id} tool={tool} />
              ))}
            </ProcessSteps>
          )}
          {m.reasoning && <ReasoningBlock text={m.reasoning} />}
          {m.content ? (
            <MarkdownText text={m.content} tone={m.tone} onOpenUrl={onOpenUrl} />
          ) : (
            busy && (
              <div className="flex items-center text-text-secondary text-[14px]">
                <FontAwesomeIcon icon={["fas", "circle-notch"]} className="animate-spin mr-2 text-[13px]" />
                {t("chatView.working")}
              </div>
            )
          )}
          {!busy && m.content && (
            <UsageFooter usage={m.usage} totalMs={m.runMs ?? 0} answer={m.content} />
          )}
        </div>
      )}
    </div>
  );
}

/**
 * A big collapsible container that folds away a long run's process steps
 * (reasoning + tool calls), keeping the final answer prominent. Collapsed by
 * default; the header summarizes how many tools ran.
 */
function ProcessSteps({
  toolCount,
  totalMs,
  children,
}: {
  toolCount: number;
  totalMs: number;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <div className="w-full mb-2 border border-border-theme rounded-xl bg-gray-50/60 overflow-hidden">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center px-4 py-2.5 text-[13px] text-text-secondary hover:text-text-base transition-colors"
      >
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-down" : "chevron-right"]}
          className="mr-2 text-[11px]"
        />
        <FontAwesomeIcon icon={["fas", "list-check"]} className="mr-2 text-[12px]" />
        <span className="font-medium">
          {toolCount > 0
            ? t("chatView.processSteps", { count: toolCount })
            : t("chatView.processStepsNoTools")}
        </span>
        {totalMs > 0 && (
          <span className="ml-2 text-[11px] text-text-secondary tabular-nums">· {formatMs(totalMs)}</span>
        )}
        {!open && <span className="ml-2 text-[11px] text-text-secondary">{t("chatView.clickToExpand")}</span>}
      </button>
      {open && <div className="border-t border-border-theme px-4 py-3">{children}</div>}
    </div>
  );
}

/** Format a millisecond duration compactly (e.g. 850ms, 2.3s, 1m12s). */
function formatMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const rem = Math.round(s % 60);
  return `${m}m${rem}s`;
}

/** Compact number (1234 -> 1.2k). */
function formatTokens(n: number): string {
  if (n < 1000) return `${n}`;
  return `${(n / 1000).toFixed(n < 10000 ? 1 : 0)}k`;
}

const CNY_SYMBOL = "\uFFE5";

/** Format an RMB amount with adaptive precision (small spends keep 6 decimals). */
function formatCny(n: number): string {
  if (n <= 0) return `${CNY_SYMBOL}0`;
  if (n < 0.01) return `${CNY_SYMBOL}${n.toFixed(6)}`;
  if (n < 1) return `${CNY_SYMBOL}${n.toFixed(4)}`;
  return `${CNY_SYMBOL}${n.toFixed(2)}`;
}
/**
 * The footer shown under a finished assistant answer: a token/usage metaline
 * (total + input/output breakdown + cache hit) and total duration, plus a row
 * of action buttons (copy now; worktree and others wired in later). Gives the
 * answer breathing room from the composer and a home for per-turn actions.
 */
function UsageFooter({
  usage,
  totalMs: durationMs,
  answer,
}: {
  usage?: TokenUsage;
  totalMs: number;
  answer?: string;
}) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const onCopy = () => {
    if (!answer) return;
    navigator.clipboard?.writeText(answer).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      },
      () => {}
    );
  };

  const hasMetrics = !!usage || durationMs > 0;

  return (
    <div className="mt-3 pt-2.5 border-t border-border-theme">
      {/* Metrics line(s) */}
      {hasMetrics && (
        <div className="flex flex-col gap-0.5 text-[11px] text-text-secondary tabular-nums">
          {usage && (
            <div className="flex items-center flex-wrap gap-x-1.5">
              <span className="font-medium text-text-base">
                {formatTokens(usage.totalTokens)} tokens
              </span>
              <span className="text-text-secondary">
                ({formatTokens(usage.promptTokens)}
                <FontAwesomeIcon icon={["fas", "arrow-down"]} className="mx-0.5 text-[9px]" />
                {formatTokens(usage.completionTokens)}
                <FontAwesomeIcon icon={["fas", "arrow-up"]} className="ml-0.5 text-[9px]" />)
              </span>
              {usage.cacheHitTokens > 0 && (
                <span className="text-green-600 ml-1" title={t("chatView.tokensCacheHit")}>
                  <FontAwesomeIcon icon={["fas", "bolt"]} className="mr-0.5 text-[9px]" />
                  {t("chatView.tokensCacheHit")} {formatTokens(usage.cacheHitTokens)}
                </span>
              )}
              <span
                className="text-text-secondary ml-1"
                title={t("chatView.turnCost")}
              >
                <FontAwesomeIcon icon={["fas", "coins"]} className="mr-0.5 text-[9px]" />
                {typeof usage.costYuan === "number" ? formatCny(usage.costYuan) : `${CNY_SYMBOL}--`}
              </span>
            </div>
          )}
          {durationMs > 0 && (
            <div className="flex items-center">
              <FontAwesomeIcon icon={["far", "clock"]} className="mr-1 text-[9px]" />
              {t("chatView.totalDuration")}: {formatMs(durationMs)}
            </div>
          )}
        </div>
      )}

      {/* Actions row (copy now; worktree & more wired later). */}
      <div className="flex items-center gap-1 mt-1.5 -ml-1">
        <button
          onClick={onCopy}
          title={t("chatView.copyAnswer")}
          className="w-7 h-7 rounded-md flex items-center justify-center text-text-secondary hover:bg-gray-100 hover:text-text-base transition-colors"
        >
          <FontAwesomeIcon icon={copied ? ["fas", "check"] : ["far", "copy"]} className="text-[12px]" />
        </button>
        <button
          title={t("chatView.addWorktree")}
          className="w-7 h-7 rounded-md flex items-center justify-center text-text-secondary hover:bg-gray-100 hover:text-text-base transition-colors"
        >
          <FontAwesomeIcon icon={["fas", "code-branch"]} className="text-[12px]" />
        </button>
      </div>
    </div>
  );
}

/**
 * A collapsible block showing the model's full Thinking-Mode reasoning trace.
 * Auto-expands while it is the actively-streaming tail (so the user watches the
 * reasoning grow); collapsible afterwards to keep the answer prominent.
 */
function ReasoningBlock({ text, defaultOpen = false }: { text: string; defaultOpen?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(defaultOpen);
  // Follow the streaming state: open while streaming, collapse once it ends.
  const prevDefault = useRef(defaultOpen);
  useEffect(() => {
    if (prevDefault.current !== defaultOpen) {
      setOpen(defaultOpen);
      prevDefault.current = defaultOpen;
    }
  }, [defaultOpen]);
  return (
    <div className="w-full mb-3 border border-border-theme rounded-lg bg-gray-50/60 overflow-hidden">
      <button
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center px-3 py-2 text-[12px] text-text-secondary hover:text-text-base transition-colors"
      >
        <FontAwesomeIcon
          icon={["fas", open ? "chevron-down" : "chevron-right"]}
          className="mr-2 text-[10px]"
        />
        <FontAwesomeIcon icon={["fas", "lightbulb"]} className="mr-1.5 text-[11px]" />
        {t("chatView.reasoning")}
      </button>
      {open && (
        <pre className="px-3 pb-3 text-[12px] text-text-secondary whitespace-pre-wrap font-mono leading-relaxed border-t border-border-theme pt-2">
          {text}
        </pre>
      )}
    </div>
  );
}
