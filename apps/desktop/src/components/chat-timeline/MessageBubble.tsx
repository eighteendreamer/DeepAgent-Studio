import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { convertFileSrc } from "@tauri-apps/api/core";
import { AnimatePresence, LayoutGroup, motion, useReducedMotion } from "framer-motion";
import type { ComposerMention, ComposerSkillSelection } from "../../types";
import { MarkdownText } from "../MarkdownText";
import { morphSpringTransition } from "../ui/morphingMenuMotion";
import { TintButton } from "../ui/TintButton";
import { TurnFooter } from "./TurnFooter";
import type { ChatBlock } from "./timelineTypes";

const USER_BUBBLE_READ =
  "max-w-[80%] w-fit overflow-hidden rounded-2xl rounded-tr-sm bg-gray-100 px-4 py-3";
const USER_BUBBLE_EDIT =
  "w-full max-w-[80%] overflow-hidden rounded-2xl rounded-tr-sm bg-gray-100 px-4 py-3";

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
    <span
      key={key}
      className="mx-0.5 inline-flex max-w-[220px] translate-y-[2px] items-center rounded-md border border-primary/20 bg-primary/10 px-1.5 py-0.5 text-[13px] font-medium leading-none text-primary"
      title={`${skill.name} (${skill.id})`}
    >
      <FontAwesomeIcon icon={["fas", "cube"]} className="mr-1 text-[11px]" />
      <span className="truncate">{skill.name || skill.id}</span>
    </span>
  );
}

function renderMentionChip(mention: ComposerMention, key: string) {
  return (
    <span
      key={key}
      className="mx-0.5 inline-flex max-w-[260px] translate-y-[2px] items-center rounded-md border border-gray-300 bg-gray-50 px-1.5 py-0.5 text-[13px] font-medium leading-none text-text-base"
      title={mentionTitle(mention)}
    >
      <FontAwesomeIcon icon={["fas", mentionIcon(mention)]} className="mr-1 text-[11px] text-text-secondary" />
      <span className="truncate">{mentionLabel(mention)}</span>
    </span>
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
            <img
              key={attachment.id}
              src={imageSrc}
              alt={attachment.name}
              className="max-h-40 max-w-[240px] rounded-xl border border-border-theme object-contain shadow-sm"
            />
          );
        }
        return (
          <span
            key={attachment.id}
            className="inline-flex max-w-[260px] items-center rounded-xl border border-border-theme bg-white px-3 py-2 text-[12px] text-text-secondary shadow-sm"
            title={attachment.name}
          >
            <FontAwesomeIcon icon={["fas", "paperclip"]} className="mr-2 text-[11px]" />
            <span className="truncate">{attachment.name}</span>
          </span>
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
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const hasVisibleMessage = Boolean(block.text || block.selectedSkills.length > 0 || block.mentions.length > 0);
  const reduced = useReducedMotion();
  const spring = morphSpringTransition(reduced);
  const layoutId = `user-msg-bubble-${block.id}`;

  useEffect(() => {
    if (!editing) return;
    const textarea = textareaRef.current;
    if (!textarea) return;
    const focus = () => {
      textarea.focus();
      textarea.setSelectionRange(textarea.value.length, textarea.value.length);
      textarea.style.height = "auto";
      textarea.style.height = `${Math.min(textarea.scrollHeight, 360)}px`;
    };
    if (reduced) {
      focus();
      return;
    }
    const id = window.requestAnimationFrame(focus);
    return () => window.cancelAnimationFrame(id);
  }, [editing, reduced]);

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
      <LayoutGroup id={block.id}>
        <AnimatePresence mode="popLayout" initial={false}>
          {editing ? (
            <motion.div key="edit" layoutId={layoutId} transition={spring} className={USER_BUBBLE_EDIT}>
              <textarea
                ref={textareaRef}
                value={draft}
                rows={1}
                onChange={(event) => {
                  setDraft(event.currentTarget.value);
                  event.currentTarget.style.height = "auto";
                  event.currentTarget.style.height = `${Math.min(event.currentTarget.scrollHeight, 360)}px`;
                }}
                onKeyDown={(event) => {
                  if (event.key === "Escape") {
                    event.preventDefault();
                    cancelEdit();
                  }
                  if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                    event.preventDefault();
                    submit();
                  }
                }}
                className="block min-h-[1.6em] w-full resize-none bg-transparent text-[15px] leading-relaxed text-text-base outline-none"
              />
              <motion.div
                initial={reduced ? false : { opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={reduced ? undefined : { opacity: 0, y: 8 }}
                transition={
                  reduced
                    ? { duration: 0 }
                    : { delay: 0.06, type: "spring", stiffness: 320, damping: 28 }
                }
                className="mt-2 flex items-center justify-end gap-2"
              >
                <TintButton type="button" onClick={cancelEdit}>
                  取消
                </TintButton>
                <TintButton type="button" className="font-medium" disabled={!draft.trim() || busy} onClick={submit}>
                  重新发送
                </TintButton>
              </motion.div>
            </motion.div>
          ) : (
            hasVisibleMessage && (
              <motion.div
                key="read"
                layoutId={layoutId}
                transition={spring}
                className={`${USER_BUBBLE_READ} text-[15px] leading-relaxed text-text-base`}
              >
                <UserInlineContent content={block.text} skills={block.selectedSkills} mentions={block.mentions} />
              </motion.div>
            )
          )}
        </AnimatePresence>
      </LayoutGroup>
      <AnimatePresence initial={false}>
        {!editing && (
          <motion.div
            key="edit-actions"
            initial={reduced ? false : { opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={reduced ? undefined : { opacity: 0, height: 0 }}
            transition={reduced ? { duration: 0 } : { duration: 0.22, ease: [0.4, 0, 0.2, 1] }}
            className="mt-1 flex min-h-6 items-center gap-2 overflow-hidden text-[11.5px] text-text-secondary opacity-0 transition group-hover:opacity-90"
          >
            <button
              type="button"
              disabled={busy}
              onClick={startEdit}
              className="rounded-md px-1.5 py-0.5 transition hover:bg-gray-100 hover:text-text-base disabled:opacity-50"
            >
              编辑
            </button>
          </motion.div>
        )}
      </AnimatePresence>
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
      <TurnFooter usage={block.usage} totalMs={block.runMs} answer={block.text} />
    </div>
  );
}
