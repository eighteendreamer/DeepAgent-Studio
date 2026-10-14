import { memo, useMemo } from "react";
import type { ComposerMention, ComposerSkillSelection } from "../../types";
import { deriveTurnSections, groupProcessSections } from "./deriveTurnSections";
import { AssistantMessageBubble, UserMessageBubble } from "./MessageBubble";
import { ProcessSectionRow } from "./ProcessSectionRow";
import { SessionMarkerRow } from "./SessionMarkerRow";
import type { Turn } from "./timelineTypes";
import { TurnFooter } from "./TurnFooter";
import { WorkMetaRow } from "./WorkMetaRow";
import { CollapseBlurPanel } from "./CollapseBlurPanel";
import { ProcessDetailsContext, useTurnDisclosure } from "./ProcessDisclosureContext";

function totalToolMs(turn: Turn): number {
  return turn.blocks.reduce((sum, block) => {
    if (block.kind !== "tool") return sum;
    return sum + (block.tool.durationMs ?? 0);
  }, 0);
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
  const { processBlocks, assistantContentBlocks, markerBlocks } = useMemo(
    () => deriveTurnSections(turn, processing),
    [turn, processing],
  );
  const sections = useMemo(() => groupProcessSections(processBlocks), [processBlocks]);
  const phase = turn.activity?.phase ?? (processing ? "working" : "settled");
  const turnKey = turn.activity?.id ?? turn.user?.id ?? turn.blocks[0]?.id ?? "empty-turn";
  const epoch = phase === "settled" ? "settled" : `${turn.activity?.generation ?? 0}:${phase}`;
  const { snapshot, setOpen, setDetail } = useTurnDisclosure(turnKey, epoch, phase === "working");
  const expanded = snapshot.open;
  const detailsContext = useMemo(() => ({ snapshot, setDetail }), [snapshot, setDetail]);
  const totalMs = turn.runMs ?? totalToolMs(turn);
  const hasProcess = processBlocks.length > 0;

  return (
    <div className="flex min-w-0 flex-col gap-4" data-process-turn={turnKey}>
      {turn.user && <UserMessageBubble block={turn.user} busy={busy} onResend={onResend} />}
      {hasProcess && (
        <div className="flex min-w-0 flex-col gap-1 pb-1">
          <WorkMetaRow
            processing={processing}
            totalMs={totalMs}
            expanded={expanded}
            onToggle={() => setOpen(!expanded)}
          />
          <CollapseBlurPanel open={expanded && sections.length > 0}>
            <ProcessDetailsContext.Provider value={detailsContext}>
              <div className="flex min-w-0 flex-col gap-1">
                {sections.map((section) => (
                  <ProcessSectionRow key={section.id} section={section} processing={processing} />
                ))}
              </div>
            </ProcessDetailsContext.Provider>
          </CollapseBlurPanel>
        </div>
      )}
      {assistantContentBlocks.map((block) => (
        <AssistantMessageBubble key={block.id} block={block} onOpenUrl={onOpenUrl} />
      ))}
      {/* One metrics line per turn, rendered here rather than inside a text
          block: while the run continues, the answer text can be reclassified
          into the process timeline, and a block-level footer would blink. */}
      <TurnFooter
        usage={turn.usage}
        totalMs={turn.runMs}
        answer={assistantContentBlocks.map((block) => block.text).join("\n").trim()}
        editedFiles={turn.editedFiles}
      />
      {markerBlocks.map((block) => (
        <SessionMarkerRow key={block.id} block={block} />
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
    previous.activity === next.activity &&
    previous.usage === next.usage &&
    previous.runMs === next.runMs &&
    previous.editedFiles === next.editedFiles &&
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
