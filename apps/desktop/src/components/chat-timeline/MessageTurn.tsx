import { memo, useMemo, useState } from "react";
import type { ComposerMention, ComposerSkillSelection } from "../../types";
import { deriveTurnSections, groupProcessSections } from "./deriveTurnSections";
import { AssistantMessageBubble, UserMessageBubble } from "./MessageBubble";
import { ProcessSectionRow } from "./ProcessSectionRow";
import type { Turn } from "./timelineTypes";
import { WorkMetaRow } from "./WorkMetaRow";
import { CollapseBlurPanel } from "./CollapseBlurPanel";

function totalToolMs(turn: Turn): number {
  return turn.blocks.reduce((sum, block) => {
    if (block.kind !== "tool") return sum;
    return sum + (block.tool.durationMs ?? 0);
  }, 0);
}

function toolCount(turn: Turn): number {
  return turn.blocks.filter((block) => block.kind === "tool").length;
}

function hasProcessError(turn: Turn): boolean {
  return turn.blocks.some((block) => block.kind === "tool" && (block.tool.status === "error" || block.tool.status === "blocked"));
}

interface MessageTurnProps {
  turn: Turn;
  processing: boolean;
  busy: boolean;
  onResend: (text: string, skills?: ComposerSkillSelection[], mentions?: ComposerMention[]) => void;
  onOpenUrl?: (url: string) => void;
}

function MessageTurnComponent({
  turn,
  processing,
  busy,
  onResend,
  onOpenUrl,
}: MessageTurnProps) {
  const { processBlocks, assistantContentBlocks } = useMemo(
    () => deriveTurnSections(turn, processing),
    [turn, processing],
  );
  const sections = useMemo(() => groupProcessSections(processBlocks), [processBlocks]);
  const forceOpen = processing || hasProcessError(turn);
  const [expandedOverride, setExpandedOverride] = useState<boolean | null>(null);
  const expanded = expandedOverride ?? forceOpen;
  const tools = toolCount(turn);
  const totalMs = totalToolMs(turn);
  const hasProcess = processBlocks.length > 0;

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {turn.user && <UserMessageBubble block={turn.user} busy={busy} onResend={onResend} />}
      {hasProcess && (
        <div className="flex min-w-0 flex-col gap-1 pb-1">
          <WorkMetaRow
            processing={processing}
            stepCount={processBlocks.length}
            toolCount={tools}
            totalMs={totalMs}
            expanded={expanded}
            onToggle={() => setExpandedOverride((value) => !(value ?? forceOpen))}
          />
          <CollapseBlurPanel open={expanded && sections.length > 0}>
            <div className="flex min-w-0 flex-col gap-1">
              {sections.map((section) => (
                <ProcessSectionRow key={section.id} section={section} processing={processing} />
              ))}
            </div>
          </CollapseBlurPanel>
        </div>
      )}
      {assistantContentBlocks.map((block) => (
        <AssistantMessageBubble key={block.id} block={block} onOpenUrl={onOpenUrl} />
      ))}
      {processing && !hasProcess && assistantContentBlocks.length === 0 && (
        <div className="flex w-fit items-center gap-2 py-0.5 text-[14px] font-medium text-text-secondary">
          <span className="h-2 w-2 animate-pulse rounded-full bg-primary" />
          正在处理
        </div>
      )}
    </div>
  );
}

function sameTurnBlocks(previous: Turn, next: Turn): boolean {
  return previous.user === next.user &&
    previous.blocks.length === next.blocks.length &&
    previous.blocks.every((block, index) => block === next.blocks[index]);
}

export const MessageTurn = memo(
  MessageTurnComponent,
  (previous, next) =>
    previous.processing === next.processing &&
    previous.busy === next.busy &&
    previous.onResend === next.onResend &&
    previous.onOpenUrl === next.onOpenUrl &&
    sameTurnBlocks(previous.turn, next.turn),
);
