import type {
  ChatMessage,
  ComposerAttachment,
  ComposerMention,
  ComposerSkillSelection,
  MessagePart,
} from "../../types";
import type { ChatBlock } from "./timelineTypes";

const USER_SKILL_MARKER = "\uE000";
const USER_MENTION_MARKER = "\uE001";

function parseAttachmentAttrs(raw: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  const attrRegex = /([a-zA-Z0-9_-]+)="([^"]*)"/g;
  let match: RegExpExecArray | null;
  while ((match = attrRegex.exec(raw)) !== null) {
    attrs[match[1]] = match[2]
      .replace(/&quot;/g, '"')
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&amp;/g, "&");
  }
  return attrs;
}

function unescapeXmlText(value: string): string {
  return value
    .replace(/&quot;/g, '"')
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");
}

export function stripUserContext(content: string): string {
  return content
    .replace(/\n*<skill-context\b[^>]*>[\s\S]*?<\/skill-context>\s*/g, "")
    .replace(/\n*<attachments>[\s\S]*?<\/attachments>\s*/g, "")
    .replace(/\n*<context-items>[\s\S]*?<\/context-items>\s*/g, "")
    .trim();
}

export function stripUserMarkers(content: string): string {
  return content.replace(new RegExp(`[${USER_SKILL_MARKER}${USER_MENTION_MARKER}]`, "g"), "");
}

export function parseSkillContexts(content: string): ComposerSkillSelection[] {
  const skills: ComposerSkillSelection[] = [];
  const contextRegex = /<skill-context\b([^>]*)>/g;
  let match: RegExpExecArray | null;
  while ((match = contextRegex.exec(content)) !== null) {
    const attrs = parseAttachmentAttrs(match[1]);
    const id = attrs.id || attrs.name;
    if (id) skills.push({ id, name: attrs.name || id });
  }
  return skills;
}

export function parseMentionContexts(content: string): ComposerMention[] {
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

export function parseAttachmentContext(content: string): ComposerAttachment[] {
  const match = content.match(/<attachments>\s*([\s\S]*?)\s*<\/attachments>/);
  if (!match) return [];
  const attachments: ComposerAttachment[] = [];
  const itemRegex = /<attachment\b([^>]*)>([\s\S]*?)<\/attachment>/g;
  let block: RegExpExecArray | null;
  while ((block = itemRegex.exec(match[1])) !== null) {
    const attrs = parseAttachmentAttrs(block[1]);
    const kind = attrs.kind === "image" || attrs.kind === "file" || attrs.kind === "text" ? attrs.kind : "file";
    attachments.push({
      id: `parsed-${attrs.index ?? attachments.length + 1}-${attrs.name ?? "attachment"}`,
      kind,
      name: attrs.name || "attachment",
      mime: attrs.mime || "",
      size: Number(attrs.size ?? 0),
      source: "picker",
      localPath: attrs.path,
      extractedText: unescapeXmlText(block[2].trim()),
      status: "ready",
    });
  }
  return attachments;
}

function mapAssistantPart(part: MessagePart, messageIndex: number, partIndex: number, message: ChatMessage): ChatBlock | null {
  if (part.kind === "reasoning") {
    if (!part.text.trim()) return null;
    return { kind: "reasoning", id: `m${messageIndex}-p${partIndex}-reasoning`, text: part.text };
  }
  if (part.kind === "tool") {
    return { kind: "tool", id: `tool-${part.tool.call_id || `${messageIndex}-${partIndex}`}`, tool: part.tool };
  }
  if (!part.text.trim()) return null;
  return {
    kind: "assistant",
    id: `m${messageIndex}-p${partIndex}-assistant`,
    text: part.text,
    tone: part.tone ?? message.tone,
    usage: message.usage,
    runMs: message.runMs,
    source: message,
  };
}

export function chatMessagesToBlocks(messages: ChatMessage[]): ChatBlock[] {
  return messages.flatMap((message, messageIndex): ChatBlock[] => {
    if (message.role === "user") {
      const text = stripUserContext(message.content);
      return [
        {
          kind: "user",
          id: `m${messageIndex}-user`,
          text,
          attachments: message.attachments?.length ? message.attachments : parseAttachmentContext(message.content),
          selectedSkills: message.selectedSkills?.length ? message.selectedSkills : parseSkillContexts(message.content),
          mentions: message.mentions?.length ? message.mentions : parseMentionContexts(message.content),
          source: message,
        },
      ];
    }

    if (message.parts?.length) {
      const mapped = message.parts
        .map((part, partIndex) => mapAssistantPart(part, messageIndex, partIndex, message))
        .filter((block): block is ChatBlock => block !== null);
      if (mapped.length > 0) return mapped;
    }

    const blocks: ChatBlock[] = [];
    if (message.reasoning?.trim()) {
      blocks.push({ kind: "reasoning", id: `m${messageIndex}-reasoning`, text: message.reasoning });
    }
    for (const tool of message.tools ?? []) {
      blocks.push({ kind: "tool", id: `tool-${tool.call_id}`, tool });
    }
    if (message.content?.trim()) {
      blocks.push({
        kind: "assistant",
        id: `m${messageIndex}-assistant`,
        text: message.content,
        tone: message.tone,
        usage: message.usage,
        runMs: message.runMs,
        source: message,
      });
    }
    return blocks;
  });
}
