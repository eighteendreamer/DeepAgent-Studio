import { HoverInfo } from "../ui/HoverInfo";
import { useState } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { ComposerMention, ComposerSkillSelection } from "../../types";
import { MarkdownText } from "../MarkdownText";
import { ImagePreviewTrigger } from "../ImagePreviewTrigger";
import { useMessageEditMotion } from "../ui/useMessageEditMotion";
import { TintButton } from "../ui/TintButton";
import type { ChatBlock } from "./timelineTypes";

const USER_BUBBLE_READ =
  "absolute right-0 top-0 w-max overflow-hidden rounded-2xl rounded-tr-sm bg-gray-100 px-4 py-3";

const USER_SKILL_MARKER = "\uE000";
const USER_MENTION_MARKER = "\uE001";

function mentionIcon(mention: ComposerMention): "folder" | "file" | "bullseye" | "list-check" {
  if (mention.kind === "folder") return "folder";
  if (mention.kind === "file") return "file";
  if (mention.kind === "goal") return "bullseye";
  return "list-check";
}

function mentionLabel(mention: ComposerMention): string {
  if (mention.kind === "goal") return "目标";
  if (mention.kind === "plan_mode") return "计划模式";
  return mention.relPath || mention.name;
}

function mentionTitle(mention: ComposerMention): string | undefined {
  if (mention.kind === "goal") return mention.text;
  if (mention.kind === "plan_mode") return "开启计划模式";
  return mention.path;
}

function renderSkillChip(skill: ComposerSkillSelection, key: string) {
  return (
    <HoverInfo key={key} content={`${skill.name} (${skill.id})`}><span
      className="mx-0.5 inline-flex max-w-[220px] translate-y-[2px] items-center rounded-md border border-primary/20 bg-primary/10 px-1.5 py-0.5 text-[13px] font-medium leading-none text-primary"

    >
      <FontAwesomeIcon icon={["fas", "cube"]} className="mr-1 text-[11px]" />
      <span className="truncate">{skill.name || skill.id}</span>
    </span></HoverInfo>
  );
}

function renderMentionChip(mention: ComposerMention, key: string) {
  return (
    <HoverInfo key={key} content={mentionTitle(mention)}><span
      className="mx-0.5 inline-flex max-w-[260px] translate-y-[2px] items-center rounded-md border border-gray-300 bg-gray-50 px-1.5 py-0.5 text-[13px] font-medium leading-none text-text-base"

    >
      <FontAwesomeIcon icon={["fas", mentionIcon(mention)]} className="mr-1 text-[11px] text-text-secondary" />
      <span className="truncate">{mentionLabel(mention)}</span>
    </span></HoverInfo>
  );
}

function UserInlineContent({
  content,
  skills,
  mentions,
}: {
  content: string;
  skills: ComposerSkillSelection[];
  mentions: ComposerMention[];
}) {
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

function AttachmentPreviews({ block }: { block: Extract<ChatBlock, { kind: "user" }> }) {
  if (block.attachments.length === 0) return null;
  return (
    <div className="mb-2 flex max-w-[80%] flex-wrap justify-end gap-2 self-end">
      {block.attachments.map((attachment) => {
        const imageSrc =
          attachment.kind === "image"
            ? attachment.dataUrl || (attachment.localPath ? convertFileSrc(attachment.localPath) : "")
            : "";
        if (imageSrc) {
          return (
            <ImagePreviewTrigger
              key={attachment.id}
              src={imageSrc}
              alt={attachment.name}
              triggerClassName="max-h-20 max-w-[120px] rounded-xl border border-border-theme shadow-sm"
              imageClassName="max-h-20 max-w-[120px] object-contain"
            />
          );
        }
        return (
          <HoverInfo key={attachment.id} content={attachment.name}><span
            className="inline-flex max-w-[260px] items-center rounded-xl border border-border-theme bg-white px-3 py-2 text-[12px] text-text-secondary shadow-sm"

          >
            <FontAwesomeIcon icon={["fas", "paperclip"]} className="mr-2 text-[11px]" />
            <span className="truncate">{attachment.name}</span>
          </span></HoverInfo>
        );
      })}
    </div>
  );
}

export function UserMessageBubble({
  block,
  busy,
  onResend,
}: {
  block: Extract<ChatBlock, { kind: "user" }>;
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(block.text.replace(/[\uE000\uE001]/g, ""));
  const { slotRef, shellRef, compactRef, editorRef, textareaRef, actionsRef, editButtonRef } =
    useMessageEditMotion(editing, draft, block);
  const hasVisibleMessage = Boolean(block.text || block.selectedSkills.length > 0 || block.mentions.length > 0);

  const cancelEdit = () => {
    setDraft(block.text.replace(/[\uE000\uE001]/g, ""));
    setEditing(false);
  };

  const startEdit = () => {
    setDraft(block.text.replace(/[\uE000\uE001]/g, ""));
    setEditing(true);
  };

  const submit = () => {
    const text = draft.trim();
    if (!text || busy) return;
    setEditing(false);
    onResend(text, block.selectedSkills, block.mentions);
  };

  return (
    <div className="group flex w-full flex-col items-end">
      <AttachmentPreviews block={block} />
      <div ref={slotRef} data-user-message-layout className="relative w-full">
        <div ref={shellRef} data-message-edit-shell data-state={editing ? "open" : "closed"}
          className="morph-menu-shell absolute right-0 top-0 overflow-hidden">
          <div ref={compactRef} aria-hidden={editing}
            className={hasVisibleMessage ? `${USER_BUBBLE_READ} text-[15px] leading-relaxed text-text-base` : "absolute right-0 top-0 h-0 w-0 overflow-hidden"}>
            {hasVisibleMessage && <UserInlineContent content={block.text} skills={block.selectedSkills} mentions={block.mentions} />}
          </div>
          <div ref={editorRef} aria-hidden={!editing} className="absolute right-0 top-0 bg-gray-100 p-3.5" style={{ opacity: 0 }}>
              <textarea
                ref={textareaRef}
                aria-label="编辑消息"
                value={draft}
                rows={1}
                onChange={(event) => setDraft(event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.nativeEvent.isComposing) return;
                  if (event.key === "Escape") {
                    event.preventDefault();
                    cancelEdit();
                  }
                  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                    event.preventDefault();
                    submit();
                  }
                }}
                className="custom-scrollbar block min-h-12 max-h-36 w-full resize-none bg-transparent text-[15px] leading-6 text-text-base outline-none"
              />
              <div className="mt-3 flex items-center justify-between gap-2">
                <span className="text-[10px] text-text-secondary max-sm:hidden">Esc 取消</span>
                <div className="ml-auto flex items-center gap-2">
                <TintButton data-edit-action type="button" onClick={cancelEdit}>
                  取消
                </TintButton>
                <TintButton data-edit-action type="button" className="font-medium" disabled={!draft.trim() || busy} onClick={submit}>
                  重新发送
                </TintButton>
                </div>
              </div>
          </div>
        </div>
            <div ref={actionsRef} aria-hidden={editing}
              className="absolute bottom-0 right-0 flex h-6 items-center text-[11.5px] text-text-secondary">
              <button
                ref={editButtonRef}
                type="button"
                disabled={busy}
                tabIndex={editing ? -1 : 0}
                onClick={startEdit}
                className="rounded-md px-1.5 py-0.5 opacity-0 transition group-hover:opacity-90 focus-visible:opacity-100 hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
              >
                编辑
              </button>
            </div>
      </div>
    </div>
  );
}

export function AssistantMessageBubble({
  block,
  onOpenUrl,
}: {
  block: Extract<ChatBlock, { kind: "assistant" }>;
  onOpenUrl?: (url: string) => void;
}) {
  return (
    <div className="group/message flex min-w-0 max-w-full flex-col">
      <div className="min-w-0 max-w-full text-text-base">
        <MarkdownText text={block.text} tone={block.tone} onOpenUrl={onOpenUrl} />
      </div>
    </div>
  );
}
