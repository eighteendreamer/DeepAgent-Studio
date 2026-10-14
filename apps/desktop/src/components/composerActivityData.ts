import type { ChatMessage, SessionDetail, ToolCall } from "../types";
import type { ComposerTodo } from "./ComposerActivityBar";

/** Existing chats use their persisted path; names and stale details cannot identify a repository. */
export function resolveChatRepositoryPath(
  sessionId: string | null,
  detail: SessionDetail | null,
  selectedProjectPath: string | null,
): string | null {
  if (!sessionId) return selectedProjectPath;
  if (detail?.summary.id !== sessionId) return null;
  return detail.project_path?.trim() ? detail.project_path : null;
}

/** Ordered parts are authoritative; legacy tools only fill missing call IDs. */
export function conversationTools(messages: ChatMessage[]): ToolCall[] {
  return messages.flatMap((message) => {
    const ordered = (message.parts ?? []).flatMap((part) => part.kind === "tool" ? [part.tool] : []);
    const ids = new Set(ordered.map((tool) => tool.call_id));
    return [...(message.tools ?? []).filter((tool) => !ids.has(tool.call_id)), ...ordered];
  });
}

function parseSnapshot(value: unknown, callId: string): ComposerTodo[] | null {
  if (typeof value !== "object" || value === null) return null;
  const todos = (value as { todos?: unknown }).todos;
  if (!Array.isArray(todos)) return null;
  const items: ComposerTodo[] = [];
  for (const [index, raw] of todos.entries()) {
    if (!raw || typeof raw.content !== "string" || !raw.content.trim()) return null;
    const status = raw.status === "pending" ? "pending"
      : ["in_progress", "in-progress", "inprogress"].includes(raw.status) ? "in_progress"
      : ["completed", "complete", "done"].includes(raw.status) ? "completed" : null;
    if (!status) return null;
    items.push({
      id: `${callId}:${index}`,
      text: status === "in_progress" && typeof raw.active_form === "string" && raw.active_form.trim()
        ? raw.active_form.trim() : raw.content.trim(),
      status,
    });
  }
  return items;
}

export function collectComposerActivity(messages: ChatMessage[]) {
  const tools = conversationTools(messages);
  let todos: ComposerTodo[] = [];
  for (const tool of tools) {
    if (tool.status !== "ok" || !["todo_write", "task_list"].includes(tool.name)) continue;
    let snapshot = parseSnapshot(tool.output, tool.call_id);
    // Older recordings kept arguments but not the successful output.
    if (snapshot === null && tool.output == null && tool.name === "todo_write" && tool.args) {
      try { snapshot = parseSnapshot(JSON.parse(tool.args), tool.call_id); } catch { /* malformed legacy recording */ }
    }
    if (snapshot !== null) todos = snapshot;
  }
  return {
    todos,
    completionKey: JSON.stringify(tools.filter((tool) => tool.status !== "running")
      .map((tool) => [tool.call_id, tool.status])),
  };
}
